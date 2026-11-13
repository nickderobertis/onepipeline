#!/usr/bin/env node
// Where one hosted CI test job's wall clock goes, read back from what the run left.
// Spike tooling (spike-onepipeline-profile): nothing in the repository's own CI runs this.
//
//   node scripts/ci-time-profile.mjs job --run RUN_ID --job JOB --out FILE [--repo OWNER/NAME]
//        [--cache DIR] [--json FILE] [--min-remaining N]
//   node scripts/ci-time-profile.mjs history --since DATE --split DATE --until DATE --out FILE
//        [--compare-since DATE] [--base-ref REF] [--profiles CLASS] [--jobs a,b]
//        [--repo OWNER/NAME] [--cache DIR] [--json FILE] [--min-remaining N]
//
// `job` reads, through the `gh` login and read-only, the job's steps, its log, and the
// artifacts the run uploaded for it: `test-profile-<job>` (the per-test records
// `crates/testfakes/src/profile.rs` writes), `nx-cache-entries-<job>` (the Nx sidecar's
// per-task start and end) and, on a run whose workflow `scripts/ci-time-workflow.mjs`
// edited, `measure-<job>` (CPU and network samples, every rustc invocation's start and
// end, Nx's own task trace and graph, and the coverage-overhead partition). Its report is
// one breakdown of the job's wall clock that sums to it: the steps one after another;
// inside the gate step, each moment shared equally among the Nx tasks running then;
// inside a task, the moment is compile when a rustc is running and the task is not
// testing, a test category when its tests are running (each running test's share split
// by its record's categories), and the task's own phase otherwise. Nothing is counted
// twice, and what no source names is reported as `unattributed`.
//
// `history` lists the pull-request runs of ci.yml created from since to until (GitHub's
// range, both ends included; at most 1,000 runs, which is noted) and reports
// each job's successful durations and medians before and after `split`, counting only runs
// created at or after --compare-since (default: since), overall and by what each run's pull
// request changed: its head, fetched over git (no API request), diffed against its merge
// base with --base-ref (default origin/main; fetch it first). --profiles CLASS also reads each
// job of that class's test-profile artifact (e2e tests, e2e test-seconds, seconds per spawn
// of `onepipeline`).
//
// The report (Markdown) goes to --out and, with --json, the figures to that file; what is
// printed is one line naming the file and the headline figures. Every GitHub response is
// kept under --cache (default target/ci-time-profile), so a second run of the same
// measurement makes no request; no request is made while `gh api rate_limit` reports fewer
// than --min-remaining (default 2500) core requests.
// Exit 0 written; 1 refused, with the reason and a `next:` line on stderr (a source missing
// or malformed, a run not finished, the rate floor reached, a GitHub read failing); 2 a
// usage error.
import { execFileSync } from "node:child_process";
import {
  existsSync,
  mkdirSync,
  readdirSync,
  readFileSync,
  rmSync,
  statSync,
  writeFileSync,
} from "node:fs";
import { join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { CATEGORIES, recordProblem } from "./test-profile-report.mjs";

class Refusal extends Error {
  constructor(message, next) {
    super(message);
    this.next = next;
  }
}
const refuse = (message, next) => {
  throw new Refusal(message, next);
};
// A file the run left that is not the shape its writer gives it.
const check = (ok, what) => {
  if (!ok)
    refuse(
      `${what} is malformed`,
      "delete that run's directory under --cache and measure again; if it recurs, the writer changed",
    );
};
const finite = (...values) => values.every((v) => Number.isFinite(v));
// What the measurement could not read and went on without, each with what to do about it:
// reported in the report and counted on the printed line, never dropped silently.
const NOTES = [];
const note = (what, next) => NOTES.push(`${what} (next: ${next})`);

const FLAGS = {
  job: ["run", "job", "out", "repo", "cache", "json", "min-remaining"],
  history: [
    "since",
    "compare-since",
    "split",
    "until",
    "out",
    "jobs",
    "base-ref",
    "profiles",
    "repo",
    "cache",
    "json",
    "min-remaining",
  ],
};
const CLASSES = ["edits src/", "edits crates/ or Cargo files, not src/", "release", "other"];

function parse(argv) {
  const [command, ...rest] = argv;
  if (!Object.hasOwn(FLAGS, command ?? "")) usage("name a command: job or history");
  const opts = { command };
  for (let i = 0; i < rest.length; i += 2) {
    const flag = rest[i];
    const value = rest[i + 1];
    if (!flag?.startsWith("--") || value === undefined)
      usage(`cannot read ${flag ?? "the arguments"}`);
    const name = flag.slice(2);
    if (!FLAGS[command].includes(name)) usage(`${command} takes no ${flag}`);
    if (name in opts) usage(`${flag} is given twice`);
    opts[name] = value;
  }
  if (!opts.out) usage("--out names the file the report is written to");
  opts.repo ??= "nickderobertis/onepipeline";
  opts.cache ??= "target/ci-time-profile";
  const floor = opts["min-remaining"] ?? "2500";
  if (!/^\d+$/.test(floor) || Number(floor) > 5000)
    usage("--min-remaining is a whole number of requests, 0 to 5000");
  opts["min-remaining"] = Number(floor);
  if (!/^[\w.-]+\/[\w.-]+$/.test(opts.repo)) usage("--repo is OWNER/NAME");
  if (command === "job") {
    if (!/^\d+$/.test(opts.run ?? "")) usage("--run takes a run id");
    if (!/^[\w-]+$/.test(opts.job ?? "")) usage("--job takes a job name");
  } else {
    for (const key of ["since", "split", "until"])
      if (Number.isNaN(Date.parse(opts[key] ?? ""))) usage(`--${key} takes a date`);
    opts["compare-since"] ??= opts.since;
    if (Number.isNaN(Date.parse(opts["compare-since"]))) usage("--compare-since takes a date");
    const order = ["since", "compare-since", "split", "until"].map((k) => Date.parse(opts[k]));
    if (!order.every((t, i) => i === 0 || order[i - 1] <= t) || order[2] === order[3])
      usage("the dates run since <= compare-since <= split < until");
    opts.jobs = (opts.jobs ?? "test-linux,test-macos").split(",");
    if (!opts.jobs.every((j) => /^[\w-]+$/.test(j))) usage("--jobs is job names joined by commas");
    if (opts.profiles !== undefined && !CLASSES.includes(opts.profiles))
      usage(`--profiles is one of: ${CLASSES.join("; ")}`);
  }
  return opts;
}

function usage(message) {
  process.stderr.write(
    `ci-time-profile: ${message}\nusage: ci-time-profile.mjs job --run RUN_ID --job JOB --out FILE [--repo R] [--cache DIR] [--json FILE] [--min-remaining N]\n` +
      `       ci-time-profile.mjs history --since DATE --split DATE --until DATE --out FILE [--compare-since DATE] [--base-ref REF] [--profiles CLASS] [--jobs a,b] [--repo R] [--cache DIR] [--json FILE] [--min-remaining N]\n`,
  );
  process.exit(2);
}

class Gh {
  constructor(cache, floor) {
    this.cache = join(cache, "api");
    this.floor = floor;
    this.requests = 0;
    this.rateChecks = 0;
    this.cached = 0;
    mkdirSync(this.cache, { recursive: true });
  }
  path(endpoint, suffix) {
    return join(this.cache, endpoint.replace(/[^\w.-]+/g, "_") + suffix);
  }
  // `gh api rate_limit` itself is not counted against the allowance, so it is kept apart.
  guard() {
    this.rateChecks++;
    const remaining = Number(this.gh(["api", "rate_limit", "--jq", ".resources.core.remaining"]));
    if (!(remaining >= this.floor))
      refuse(
        `GitHub reports ${remaining} core requests remaining, under the floor of ${this.floor}; nothing more is read`,
        "wait for the allowance's reset (`gh api rate_limit` names it) and run the same command again; what was read is kept",
      );
  }
  gh(args, binary = false) {
    try {
      const out = execFileSync("gh", args, {
        encoding: binary ? "buffer" : "utf8",
        maxBuffer: 1 << 30,
        stdio: ["ignore", "pipe", "pipe"],
      });
      return binary ? out : out.trim();
    } catch (error) {
      refuse(
        `gh ${args.join(" ")} failed: ${String(error.stderr ?? error.message).trim()}`,
        "check `gh auth status` and the network, then run the same command again",
      );
    }
  }
  fetch(endpoint, { binary = false, keep = true } = {}) {
    const file = this.path(endpoint, binary ? ".bin" : ".txt");
    if (keep && existsSync(file)) {
      this.cached++;
      return readFileSync(file, binary ? undefined : "utf8");
    }
    this.guard();
    this.requests++;
    // A job log carries cargo's colour codes; the body goes to a file, never a terminal.
    const body = this.gh(["api", "--allow-escape-sequences", endpoint], binary);
    if (keep) writeFileSync(file, body);
    return body;
  }
  json(endpoint, opts) {
    try {
      return JSON.parse(this.fetch(endpoint, opts));
    } catch (error) {
      if (error instanceof Refusal) throw error;
      return check(false, `GitHub's answer to ${endpoint}`);
    }
  }
}

const ms = (iso) => Date.parse(iso);
const min = (seconds) => (seconds / 60).toFixed(1);
const pct = (part, whole) => (whole > 0 ? `${((100 * part) / whole).toFixed(1)}%` : "-");
const median = (values) => {
  const s = [...values].sort((a, b) => a - b);
  if (!s.length) return null;
  const mid = Math.floor(s.length / 2);
  return s.length % 2 ? s[mid] : (s[mid - 1] + s[mid]) / 2;
};
const table = (head, rows) =>
  [
    `| ${head.join(" | ")} |`,
    `| ${head.map(() => "---").join(" | ")} |`,
    ...rows.map((r) => `| ${r.join(" | ")} |`),
  ].join("\n");

function walk(dir, out = []) {
  if (!existsSync(dir)) return out;
  for (const name of readdirSync(dir)) {
    const p = join(dir, name);
    if (statSync(p).isDirectory()) walk(p, out);
    else out.push(p);
  }
  return out;
}

function unzipArtifact(gh, repo, artifact, into) {
  const done = join(into, ".complete");
  if (existsSync(done)) return into;
  mkdirSync(into, { recursive: true });
  const zip = gh.fetch(`repos/${repo}/actions/artifacts/${artifact.id}/zip`, {
    binary: true,
  });
  const file = join(into, "..", `${artifact.name}.zip`);
  writeFileSync(file, zip);
  execFileSync("unzip", ["-q", "-o", file, "-d", into]);
  writeFileSync(done, "");
  return into;
}

export function stepClass(name) {
  if (/measurement-only|^'?Measure:/.test(name)) return "measurement-only";
  if (/^Post /.test(name)) {
    if (/rust-cache|setup-uv|actions\/cache/.test(name)) return "cache save";
    return "setup and teardown";
  }
  if (/Swatinem\/rust-cache|Restore the bootstrap tools|Restore the visual docs/.test(name))
    return "cache restore";
  if (/Decide cache trust|op-nx\/github-cache|Wait for the cache recording proxy/.test(name))
    return "Nx cache sidecar";
  if (/Report cache entries|^Run actions\/upload-artifact/.test(name)) return "cache save";
  if (/\(affected projects\)$/.test(name)) return "gate (Nx tasks)";
  if (/^Does each failed test fail again/.test(name)) return "rerun of failed tests";
  if (/^Keep the /.test(name)) return "artifact upload";
  return "setup and teardown";
}

function parseLog(text) {
  const lines = [];
  // biome-ignore lint/suspicious/noControlCharactersInRegex: strips the ANSI colour codes cargo writes
  for (const raw of text.replace(/\u001b\[[0-9;]*[A-Za-z]/g, "").split("\n")) {
    const m = /^(\d{4}-\d\d-\d\dT\d\d:\d\d:\d\d(?:\.\d+)?Z) ?(.*)$/.exec(raw.replace(/\r$/, ""));
    if (m) lines.push({ t: ms(m[1]), text: m[2] });
  }
  return lines;
}

function logFacts(lines) {
  const facts = { github_sha: null, selection: null, cache: [], readOnly: [], nxReport: {} };
  for (let i = 0; i < lines.length; i++) {
    const { text } = lines[i];
    if (!facts.github_sha && /log -1 --format=%H|log -1 --format='%H'/.test(text)) {
      const next = lines[i + 1]?.text.trim();
      if (/^[0-9a-f]{40}$/.test(next ?? "")) facts.github_sha = next;
    }
    const sel = /ci-job: (\S+) runs (\d+) task\(s\): (.*)$/.exec(text);
    if (sel && !facts.selection)
      facts.selection = {
        job: sel[1],
        count: Number(sel[2]),
        tasks: sel[3].split(" ").filter(Boolean),
      };
    if (
      /Cache restored from key|No cache found|Cache not found for input keys|Cache hit for|Restored from cache key|Cache Size: |cache hit|Skipping cache save|Cache saved|Cache up-to-date/i.test(
        text,
      )
    )
      facts.cache.push({ t: lines[i].t, text: text.trim() });
    // Nx's own performance report at the end of the gate's invocation.
    const perf = /^\s*(Run duration|Cache|Critical path|Recoverable time):\s+(.*)$/.exec(text);
    if (perf) facts.nxReport[perf[1]] = perf[2].trim();
    if (/CACHE_READ_ONLY is set/.test(text))
      facts.readOnly.push({ t: lines[i].t, text: text.trim() });
  }
  return facts;
}

// Every nextest invocation the log shows: `Starting N tests across B binaries` paired with
// the next `Summary [ S s] N tests run`, and, where the profile prints a line per test (the
// cross-platform `ci` profile), each binary's summed test seconds. Records only cover the
// e2e harness's binary; this covers every tier.
function nextestRuns(lines) {
  const open = [];
  const runs = [];
  const binaries = {};
  for (const { t, text } of lines) {
    const start = /Starting (\d+) tests? across (\d+) binar(?:y|ies)/.exec(text);
    if (start) open.push({ t, tests: Number(start[1]), binaries: Number(start[2]) });
    const done = /Summary \[\s*([\d.]+)s\] (\d+) tests? run/.exec(text);
    if (done) {
      const i = open.findIndex((o) => o.tests === Number(done[2]));
      const o = i >= 0 ? open.splice(i, 1)[0] : { t: null, tests: Number(done[2]), binaries: null };
      runs.push({ ...o, end: t, s: Number(done[1]) });
    }
    const test =
      /^\s*(PASS|FAIL|FLAKY|TIMEOUT|LEAK|SIGKILL|SIGSEGV)\s*\[\s*([\d.]+)s\]\s*(?:\(\s*\d+\/\d+\))?\s+([\w-]+)(?:::([\w-]+))?\s/.exec(
        text,
      );
    if (test) {
      // `<crate>::<binary> <test>` for an integration binary, `<crate> <test>` for its lib.
      const binary = test[4] ?? test[3];
      binaries[binary] ??= { tests: 0, s: 0 };
      const b = binaries[binary];
      b.tests++;
      b.s += Number(test[2]);
    }
  }
  return { runs, binaries };
}

// The lines of one of the measurement's files, each parsed by `parse` (null for a line not
// in its writer's shape). The job can end in the middle of a write, so a last line with no
// newline after it may be cut short and is dropped; any other malformed line refuses.
function measureLines(file, parse) {
  const text = readFileSync(file, "utf8");
  const lines = text.split("\n");
  const cutShort = !text.endsWith("\n");
  if (!cutShort) lines.pop();
  const out = [];
  lines.forEach((line, i) => {
    const value = parse(line);
    if (value) out.push(value);
    else check(cutShort && i === lines.length - 1, `${file} line ${i + 1}`);
  });
  return out;
}

function sampleLine(line) {
  const [head, load, net] = line.split(" | ");
  if (net === undefined) return null;
  const parts = head.trim().split(/\s+/);
  const t = Number(parts[0]) * 1000;
  const [rx, tx] = net.trim().split(/\s+/).map(Number);
  if (!finite(t, rx, tx) || rx < 0 || tx < 0) return null;
  let cpu = null;
  if (parts[1] === "cpu") {
    // /proc/stat's first line: user nice system idle iowait ...
    const v = parts.slice(2).map(Number);
    if (v.length < 4 || !finite(...v) || v.some((x) => x < 0)) return null;
    cpu = { idle: v[3] + (v[4] || 0), total: v.reduce((a, b) => a + b, 0) };
  } else {
    // iostat: us sy id
    const [us, sy, id] = parts.slice(1, 4).map(Number);
    if (!finite(us, sy, id) || [us, sy, id].some((x) => x < 0 || x > 100)) return null;
    cpu = { busy: (us + sy) / 100 };
  }
  return { t, rx, tx, cpu, load: load.trim() };
}

function rustcLine(line) {
  const f = line.split("\t");
  if (f.length < 7) return null;
  const r = {
    start: Number(f[0]) * 1000,
    end: Number(f[1]) * 1000,
    crate: f[2],
    types: f[3],
    instrumented: f[4] === "1",
    outDir: f[5],
    code: Number(f[6]),
    compiler: f[7] ?? "",
  };
  const valid =
    finite(r.start, r.end) &&
    r.end >= r.start &&
    /^[01]$/.test(f[4]) &&
    Number.isInteger(r.code) &&
    /^\w+$/.test(r.crate);
  return valid ? r : null;
}

function readMeasure(dir) {
  if (!dir) return null;
  const files = walk(dir);
  const find = (name) => files.find((f) => f.endsWith(`/${name}`));
  const out = {
    samples: [],
    rustc: [],
    graph: null,
    overhead: null,
    host: null,
    nxProfile: null,
  };
  const host = find("host.txt");
  if (host) out.host = readFileSync(host, "utf8").trim().split("\n");
  const samples = find("samples.txt");
  if (samples) out.samples = measureLines(samples, sampleLine);
  const rustc = find("rustc.tsv");
  if (rustc) out.rustc = measureLines(rustc, rustcLine);
  const profile = find("nx-profile.json");
  if (profile) {
    let trace;
    try {
      trace = JSON.parse(readFileSync(profile, "utf8"));
    } catch {
      check(false, profile);
    }
    check(Array.isArray(trace), profile);
    // Nx's TaskProfilingLifeCycle: one complete event (`ph: "X"`) per task, in microseconds.
    out.nxProfile = trace.filter((e) => e?.ph === "X");
    check(
      out.nxProfile.every(
        (e) => typeof e.name === "string" && finite(e.ts, e.dur) && e.ts >= 0 && e.dur >= 0,
      ),
      profile,
    );
  }
  const graph = find("graph.json");
  if (graph) {
    // Nx prints its plan before the graph; the graph is the JSON object after it. A graph the
    // step could not write leaves no object, and the run has no dependency path.
    const text = readFileSync(graph, "utf8");
    if (text.includes("{")) {
      let parsed;
      try {
        parsed = JSON.parse(text.slice(text.indexOf("{")));
      } catch {
        check(false, graph);
      }
      const deps = parsed?.tasks?.dependencies;
      check(
        deps &&
          typeof deps === "object" &&
          !Array.isArray(deps) &&
          Object.values(deps).every(
            (d) => Array.isArray(d) && d.every((x) => typeof x === "string"),
          ),
        graph,
      );
      out.graph = parsed;
    }
  }
  const times = find("overhead/times.txt");
  if (times) {
    out.overhead = { runs: {}, records: {} };
    for (const run of measureLines(times, (line) => {
      const [label, start, end, status] = line.trim().split(/\s+/);
      const ok =
        ["instrumented", "plain-build", "plain"].includes(label) &&
        finite(Number(start), Number(end)) &&
        Number(end) >= Number(start) &&
        /^\d+$/.test(status ?? "");
      return ok ? { label, s: Number(end) - Number(start), status: Number(status) } : null;
    }))
      out.overhead.runs[run.label] = { s: run.s, status: run.status };
    for (const label of ["instrumented", "plain"]) {
      out.overhead.records[label] = readRecords(
        files.filter((f) => f.includes(`/overhead/${label}-records/`)),
      );
    }
  }
  return out;
}

// A rustc invocation's tree: what kind of build it is part of.
function tree(r) {
  if (/clippy-driver/.test(r.compiler)) return "clippy";
  // rust-toolchain.toml pins a patch release (1.97.1); the MSRV check's toolchain is the
  // bare minor (1.97) Cargo.toml's rust-version names.
  if (/\/toolchains\/\d+\.\d+-/.test(r.compiler)) return "msrv toolchain";
  if (r.instrumented || /\/target\/llvm-cov-target\//.test(r.outDir))
    return "instrumented (llvm-cov-target)";
  const inCheckout = /\/work\/[^/]+\/[^/]+\/target\/([^/]+)/.exec(r.outDir);
  if (!inCheckout) return "outside the checkout (cargo install)";
  if (inCheckout[1] === "debug") return "plain (target/debug)";
  if (inCheckout[1] === "release") return "release (target/release)";
  return `target/${inCheckout[1]}`;
}

// The workspace's own crates and targets, from the local clone's manifest, plus every crate
// this run instrumented (cargo-llvm-cov instruments the workspace's crates only). The rest
// of what rustc compiled is dependencies, which a warm cache would have held.
let workspaceCrates = null;
function workspaceNames(rustc) {
  if (!workspaceCrates) {
    workspaceCrates = new Set();
    try {
      const meta = JSON.parse(
        execFileSync("cargo", ["metadata", "--no-deps", "--format-version", "1", "--offline"], {
          encoding: "utf8",
          maxBuffer: 1 << 28,
          stdio: ["ignore", "pipe", "ignore"],
        }),
      );
      const shaped =
        Array.isArray(meta?.packages) &&
        meta.packages.every(
          (p) => Array.isArray(p?.targets) && p.targets.every((t) => typeof t?.name === "string"),
        );
      if (!shaped) throw new Error("cargo metadata printed no package and target list");
      for (const p of meta.packages)
        for (const t of p.targets) workspaceCrates.add(t.name.replace(/-/g, "_"));
    } catch (error) {
      // The instrumented crates alone then name the workspace, and a crate only ever built
      // plain counts as a dependency.
      note(
        `cargo metadata could not list the workspace's crates (${String(error.message).split("\n")[0]}), so "of which dependencies" counts every crate never built instrumented`,
        "run from a checkout of the repository with cargo on PATH",
      );
    }
  }
  const names = new Set(workspaceCrates);
  for (const r of rustc) if (r.instrumented) names.add(r.crate);
  return names;
}

function readRecords(files) {
  const records = [];
  for (const file of files) {
    if (!file.endsWith(".json")) continue;
    let record;
    let problem;
    try {
      record = JSON.parse(readFileSync(file, "utf8"));
      problem = recordProblem(record);
    } catch (error) {
      problem = `it is not JSON (${error.message})`;
    }
    if (problem) {
      note(
        `the test record ${file} was left out: ${problem}`,
        "that test's time counts as its task's; the recorder wrote it, so re-download the artifact (delete its directory under --cache) and check the recorder's schema",
      );
      continue;
    }
    records.push(record);
  }
  // A CI artifact holds one record file per test, and each tier is a nextest run of its own,
  // so every record is kept; a test seen twice keeps its later attempt.
  const kept = new Map();
  for (const r of records) {
    const key = `${r.binary}::${r.test}`;
    const prev = kept.get(key);
    if (!prev || (r.attempt ?? 0) > (prev.attempt ?? 0) || r.started_at > prev.started_at)
      kept.set(key, r);
  }
  return [...kept.values()].map((r) => ({
    ...r,
    start: r.started_at * 1000,
    end: r.started_at * 1000 + r.wall_s * 1000,
  }));
}

// A record's wall clock split into named parts that sum to it: spawn split by the program
// run, the other categories, and the recorder's own unattributed time.
function recordParts(r) {
  // A test that took no measurable time has no time to split.
  if (!(r.wall_s > 0)) return {};
  const parts = {};
  const spawnS = r.categories.spawn?.s ?? 0;
  const programs = Object.entries(r.programs ?? {}).filter(([p]) => p !== "git" && p !== "cargo");
  const programTotal = programs.reduce((a, [, v]) => a + v.s, 0);
  for (const c of CATEGORIES) {
    const s = r.categories[c]?.s ?? 0;
    if (c === "spawn" && programTotal > 0) {
      for (const [p, v] of programs) parts[`spawn: ${p}`] = (spawnS * v.s) / programTotal;
    } else if (c === "wait") {
      parts["wait (sleeps and polls)"] = s;
    } else if (c === "build") {
      parts["build (cargo run by a test)"] = s;
    } else parts[c === "spawn" ? "spawn: (unnamed)" : c] = s;
  }
  parts["in-test computation (recorder-unattributed)"] = r.unattributed_s;
  const total = Object.values(parts).reduce((a, b) => a + b, 0);
  const scale = total > 0 ? r.wall_s / total : 0;
  for (const k of Object.keys(parts)) parts[k] = (parts[k] * scale) / r.wall_s;
  return parts; // fractions of the record's wall clock
}

// One file of the Nx sidecar's report (`scripts/nx-remote-cache.mjs report`), or null when the
// run uploaded none. That script is an upstream copy whose own validator is not exported, so
// the fields read here are checked here.
function sidecarFile(dir, name, valid) {
  const file = dir && join(dir, name);
  if (!file || !existsSync(file)) return null;
  let doc;
  try {
    doc = JSON.parse(readFileSync(file, "utf8"));
  } catch {
    check(false, file);
  }
  check(doc && typeof doc === "object" && valid(doc), file);
  return doc;
}

/**
 * The artifacts a test job uploads that this reads: ci.yml's `test-profile-<job>` and
 * `nx-cache-entries-<job>`, and `scripts/ci-time-workflow.mjs`'s `measure-<job>`.
 */
export function artifactNames(job) {
  return {
    records: `test-profile-${job}`,
    sidecar: `nx-cache-entries-${job}`,
    measure: `measure-${job}`,
  };
}

function analyzeJob(opts) {
  const gh = new Gh(opts.cache, opts["min-remaining"]);
  const { repo } = opts;
  const runEndpoint = `repos/${repo}/actions/runs/${opts.run}`;
  const run = gh.json(runEndpoint);
  check(
    typeof run?.status === "string" &&
      /^[0-9a-f]{40}$/.test(run.head_sha ?? "") &&
      (run.head_branch === null || typeof run.head_branch === "string") &&
      (run.pull_requests === undefined ||
        (Array.isArray(run.pull_requests) &&
          run.pull_requests.every((pr) => Number.isInteger(pr?.number)))),
    `run ${opts.run}`,
  );
  if (run.status !== "completed") {
    // Not kept: the next measurement of this run has to see it finish.
    rmSync(gh.path(runEndpoint, ".txt"), { force: true });
    refuse(
      `run ${opts.run} is ${run.status}, not completed`,
      "measure it once it has finished: its artifacts are uploaded as its jobs end",
    );
  }
  const jobs = gh.json(`repos/${repo}/actions/runs/${opts.run}/jobs?per_page=100`);
  check(Array.isArray(jobs?.jobs), `run ${opts.run}'s jobs`);
  const job = jobs.jobs.find((j) => j?.name === opts.job);
  if (!job)
    refuse(
      `run ${opts.run} has no job ${opts.job}`,
      `name one of its jobs: ${jobs.jobs.map((j) => j?.name).join(", ")}`,
    );
  check(
    Number.isInteger(job.id) &&
      Number.isInteger(job.run_attempt) &&
      (job.runner_name === null || typeof job.runner_name === "string") &&
      (job.conclusion === null || typeof job.conclusion === "string") &&
      (job.labels === undefined ||
        (Array.isArray(job.labels) && job.labels.every((l) => typeof l === "string"))) &&
      !Number.isNaN(ms(job.started_at)) &&
      !Number.isNaN(ms(job.completed_at)) &&
      ms(job.completed_at) >= ms(job.started_at) &&
      Array.isArray(job.steps) &&
      job.steps.every(
        (st) =>
          typeof st?.name === "string" &&
          Number.isInteger(st.number) &&
          (st.conclusion === null || typeof st.conclusion === "string") &&
          !Number.isNaN(ms(st.started_at)) &&
          ms(st.completed_at) >= ms(st.started_at),
      ),
    `job ${opts.job} of run ${opts.run}`,
  );
  const lines = parseLog(gh.fetch(`repos/${repo}/actions/jobs/${job.id}/logs`));
  const facts = logFacts(lines);
  const artifacts = gh.json(
    `repos/${repo}/actions/runs/${opts.run}/artifacts?per_page=100`,
  )?.artifacts;
  check(
    Array.isArray(artifacts) &&
      artifacts.every(
        (x) =>
          Number.isInteger(x?.id) && typeof x.name === "string" && typeof x.expired === "boolean",
      ),
    `run ${opts.run}'s artifact list`,
  );
  const dir = join(opts.cache, "runs", String(opts.run), opts.job);
  const get = (name) => {
    const a = artifacts.find((x) => x.name === name && !x.expired);
    return a ? unzipArtifact(gh, repo, a, join(dir, name)) : null;
  };
  const names = artifactNames(opts.job);
  const profileDir = get(names.records);
  const nxDir = get(names.sidecar);
  const measureDir = get(names.measure);
  const executions = sidecarFile(nxDir, "executions.json", (doc) => {
    const tasksOk =
      Array.isArray(doc.tasks) &&
      doc.tasks.every(
        (t) =>
          typeof t?.task === "string" &&
          typeof t.result === "string" &&
          ms(t.finished_at) >= ms(t.started_at),
      );
    const attemptsOk =
      doc.store_attempts === undefined ||
      (Array.isArray(doc.store_attempts) &&
        doc.store_attempts.every(
          (a) =>
            typeof a?.status === "string" &&
            (a.http_status === null || Number.isInteger(a.http_status)),
        ));
    const cancelledOk =
      doc.cancelled_invocations === undefined ||
      (Array.isArray(doc.cancelled_invocations) &&
        doc.cancelled_invocations.every((c) => typeof c?.id === "string"));
    // This job's own report, not another job's or run's: a sidecar document names both.
    const identityOk =
      [1, 2].includes(doc.schema) && doc.job === opts.job && doc.run_id === Number(opts.run);
    return identityOk && tasksOk && attemptsOk && cancelledOk;
  });
  const manifest = sidecarFile(
    nxDir,
    "manifest.json",
    (doc) =>
      doc.schema === 1 &&
      doc.job === opts.job &&
      Array.isArray(doc.entries) &&
      doc.entries.every((e) => ["stored", "replayed"].includes(e?.source)) &&
      Number.isInteger(doc.stored_bytes) &&
      doc.stored_bytes >= 0,
  );
  const records = readRecords(walk(profileDir ?? ""));
  const measure = readMeasure(measureDir);

  const wall = (ms(job.completed_at) - ms(job.started_at)) / 1000;
  const result = {
    repo,
    run: Number(opts.run),
    job: opts.job,
    job_id: job.id,
    attempt: job.run_attempt,
    runner: job.runner_name,
    labels: job.labels,
    conclusion: job.conclusion,
    head_sha: run.head_sha,
    head_branch: run.head_branch,
    pull_requests: run.pull_requests?.map((p) => p.number) ?? [],
    github_sha: facts.github_sha,
    started_at: job.started_at,
    completed_at: job.completed_at,
    wall_s: wall,
    selection: facts.selection,
    nx_report: facts.nxReport,
    cache_lines: facts.cache,
    read_only_lines: facts.readOnly,
    requests: 0,
  };

  // Steps: sequential ones partition the job; a background step overlaps the next.
  const steps = job.steps.map((s) => ({
    name: s.name,
    number: s.number,
    conclusion: s.conclusion,
    start: ms(s.started_at),
    end: ms(s.completed_at),
    class: stepClass(s.name),
  }));
  steps.forEach((s, i) => {
    const next = steps[i + 1];
    s.background = Boolean(next && s.end > next.start + 1000 && s.end - s.start > 60_000);
  });
  const sequential = steps.filter((s) => !s.background && s.start && s.end);
  const stepSum = sequential.reduce((a, s) => a + (s.end - s.start) / 1000, 0);
  result.steps = steps.map((s) => ({
    name: s.name,
    class: s.class,
    background: s.background,
    s: (s.end - s.start) / 1000,
    conclusion: s.conclusion,
  }));
  result.between_steps_s = wall - stepSum;
  const byClass = {};
  for (const s of sequential) byClass[s.class] = (byClass[s.class] ?? 0) + (s.end - s.start) / 1000;
  result.step_classes = byClass;

  // The gate: Nx tasks shared fairly, each task's share split by what it was doing.
  const gate = sequential.find((s) => s.class === "gate (Nx tasks)");
  // Nx's own trace when the run kept one: it covers every task the gate's invocation ran,
  // failed and skipped ones too. Otherwise the sidecar's telemetry, which a failed gate's
  // invocation does not reach (its tasks are then only the bootstrap invocation's).
  const results = new Map((executions?.tasks ?? []).map((t) => [t.task, t.result]));
  const tasks = measure?.nxProfile?.length
    ? measure.nxProfile.map((e) => ({
        task: e.name,
        result: results.get(e.name) ?? e.args?.status ?? "-",
        start: e.ts / 1000,
        end: (e.ts + e.dur) / 1000,
      }))
    : (executions?.tasks ?? []).map((t) => ({
        task: t.task,
        result: t.result,
        remote_status: t.remote_status,
        start: ms(t.started_at),
        end: ms(t.finished_at),
      }));
  result.task_source = measure?.nxProfile?.length
    ? "NX_PROFILE trace"
    : executions?.cancelled_invocations
      ? "sidecar telemetry without the gate's invocation (a failed gate): tasks incomplete"
      : executions
        ? "sidecar telemetry"
        : "none";
  result.executions_available = Boolean(executions);
  result.store_attempts = executions?.store_attempts ?? null;
  result.stored = manifest
    ? {
        entries: manifest.entries.filter((e) => e.source === "stored").length,
        bytes: manifest.stored_bytes,
        replayed: manifest.entries.filter((e) => e.source === "replayed").length,
      }
    : null;

  // Which task ran each test binary: the test task whose window holds most of its records.
  const testTasks = tasks.filter((t) => /:(test|test-quick|smoke|release-compat)$/.test(t.task));
  const binaryTask = new Map();
  const binaries = [...new Set(records.map((r) => r.binary))];
  for (const b of binaries) {
    const counts = new Map();
    for (const r of records.filter((x) => x.binary === b))
      for (const t of testTasks)
        if (r.start >= t.start - 2000 && r.end <= t.end + 2000)
          counts.set(t, (counts.get(t) ?? 0) + 1);
    const best = [...counts.entries()].sort(
      (a, b2) => b2[1] - a[1] || a[0].end - a[0].start - (b2[0].end - b2[0].start),
    )[0];
    if (best) binaryTask.set(b, best[0]);
  }
  // Each record belongs to the task whose window holds it: its binary's main task when that
  // one does, else the shortest test task that does (a binary another task also runs, as the
  // version tasks run single e2e tests after the tier).
  const holds = (t, r) => r.start >= t.start - 2000 && r.end <= t.end + 2000;
  const owner = new Map();
  for (const r of records) {
    const main = binaryTask.get(r.binary);
    owner.set(
      r,
      main && holds(main, r)
        ? main
        : testTasks
            .filter((t) => holds(t, r))
            .sort((a, b2) => a.end - a.start - (b2.end - b2.start))[0],
    );
  }
  for (const t of tasks) {
    t.records = records.filter((r) => owner.get(r) === t);
    t.testStart = t.records.length ? Math.min(...t.records.map((r) => r.start)) : null;
    t.testEnd = t.records.length ? Math.max(...t.records.map((r) => r.end)) : null;
    t.coverage = /:test$/.test(t.task) && opts.job === "test-linux";
  }
  for (const r of records) r.parts = recordParts(r);
  const rustc = measure?.rustc ?? [];

  const categories = {};
  const add = (k, s) => {
    categories[k] = (categories[k] ?? 0) + s;
  };
  const taskShare = new Map(tasks.map((t) => [t.task, 0]));
  const taskCats = new Map(tasks.map((t) => [t.task, {}]));
  const binaryShare = new Map();
  const testShare = new Map();
  if (gate) {
    const tick = 1000;
    for (let t = gate.start; t < gate.end; t += tick) {
      const mid = t + tick / 2;
      const running = tasks.filter((x) => x.start <= mid && mid < x.end);
      if (!running.length) {
        const before = tasks.length && mid < Math.min(...tasks.map((x) => x.start));
        const after = tasks.length && mid >= Math.max(...tasks.map((x) => x.end));
        add(
          !tasks.length
            ? "gate without Nx task telemetry (unattributed)"
            : before
              ? "gate: Nx planning before the first task"
              : after
                ? "gate: after the last task (lockfile check, exit)"
                : "gate: no Nx task running (scheduling gap)",
          1,
        );
        continue;
      }
      const activeRustc = rustc.filter((r) => r.start <= mid && mid < r.end);
      const trees = [...new Set(activeRustc.map(tree))];
      const share = 1 / running.length;
      for (const x of running) {
        taskShare.set(x.task, taskShare.get(x.task) + share);
        const tc = taskCats.get(x.task);
        const put = (k, s) => {
          add(k, s);
          tc[k] = (tc[k] ?? 0) + s;
        };
        const inTests = x.records.filter((r) => r.start <= mid && mid < r.end);
        if (inTests.length) {
          for (const r of inTests) {
            const each = share / inTests.length;
            binaryShare.set(r.binary, (binaryShare.get(r.binary) ?? 0) + each);
            const key = `${r.binary}::${r.test}`;
            testShare.set(key, (testShare.get(key) ?? 0) + each);
            for (const [k, f] of Object.entries(r.parts)) put(`test: ${k}`, each * f);
          }
        } else if (trees.length && !(x.testStart && mid >= x.testEnd && x.coverage)) {
          for (const tr of trees) put(`compile: ${tr}`, share / trees.length);
        } else if (x.testStart && mid < x.testStart) {
          put("task: before its first test, no rustc running (cargo, nextest, archive)", share);
        } else if (x.testStart && mid >= x.testEnd) {
          put(
            x.coverage
              ? "task: after its last test (profdata merge, LCOV export)"
              : "task: after its last test",
            share,
          );
        } else if (x.testStart) {
          put("task: between tests (nextest scheduling)", share);
        } else {
          put(`task (no test records): ${x.task}`, share);
        }
      }
    }
  }
  result.gate_s = gate ? (gate.end - gate.start) / 1000 : 0;
  result.gate_categories = categories;
  result.tasks = tasks.map((t) => {
    const own = (t.end - t.start) / 1000;
    let overlap = 0;
    for (const o of tasks)
      if (o !== t)
        overlap += Math.max(0, Math.min(o.end, t.end) - Math.max(o.start, t.start)) / 1000;
    return {
      task: t.task,
      result: t.result,
      start_offset_s: gate ? (t.start - gate.start) / 1000 : null,
      s: own,
      avg_concurrency: own > 0 ? 1 + overlap / own : 1,
      share_s: taskShare.get(t.task),
      categories: taskCats.get(t.task),
      tests: t.records.length,
      test_seconds: t.records.reduce((a, r) => a + r.wall_s, 0),
    };
  });
  const nextest = nextestRuns(lines);
  result.nextest_runs = nextest.runs.map((n) => ({
    tests: n.tests,
    binaries: n.binaries,
    s: n.s,
    started_at: n.t ? new Date(n.t).toISOString() : null,
    tasks: n.t ? tasks.filter((x) => x.start <= n.t && n.t <= x.end).map((x) => x.task) : [],
  }));
  result.log_binaries = nextest.binaries;
  result.critical_path = criticalPath(tasks, measure?.graph, gate?.start ?? 0);
  result.concurrency_profile = concurrencyProfile(tasks, gate);

  const compile = {};
  const ours = workspaceNames(rustc);
  for (const r of rustc) {
    const k = tree(r);
    compile[k] ??= { invocations: 0, rustc_s: 0, wall_s: 0, dependency_s: 0, crates: {} };
    compile[k].invocations++;
    compile[k].rustc_s += (r.end - r.start) / 1000;
    if (!ours.has(r.crate)) compile[k].dependency_s += (r.end - r.start) / 1000;
    compile[k].crates[r.crate] = (compile[k].crates[r.crate] ?? 0) + (r.end - r.start) / 1000;
  }
  for (const k of Object.keys(compile))
    compile[k].wall_s = unionSeconds(rustc.filter((r) => tree(r) === k));
  result.compile = compile;
  result.compile_any_wall_s = unionSeconds(rustc);

  result.binaries = binaries
    .map((b) => {
      const rs = records.filter((r) => r.binary === b);
      return {
        binary: b,
        task: binaryTask.get(b)?.task ?? null,
        tier: b === "e2e" ? "e2e" : "other",
        tests: rs.length,
        test_seconds: rs.reduce((a, r) => a + r.wall_s, 0),
        elapsed_share_s: binaryShare.get(b) ?? 0,
      };
    })
    .sort((a, b) => b.test_seconds - a.test_seconds);
  result.slowest = [...records]
    .sort((a, b) => b.wall_s - a.wall_s)
    .slice(0, 20)
    .map((r) => ({
      test: `${r.binary}::${r.test}`,
      wall_s: r.wall_s,
      elapsed_share_s: testShare.get(`${r.binary}::${r.test}`) ?? 0,
      parts_s: Object.fromEntries(
        Object.entries(r.parts)
          .filter(([, f]) => f > 0.005)
          .map(([k, f]) => [k, f * r.wall_s]),
      ),
      polls: r.categories.wait?.polls ?? 0,
      sleeps: r.categories.wait?.count ?? 0,
    }));
  const programs = {};
  for (const r of records)
    for (const [p, v] of Object.entries(r.programs ?? {})) {
      programs[p] ??= { count: 0, s: 0 };
      programs[p].count += v.count;
      programs[p].s += v.s;
    }
  result.programs = programs;
  result.test_seconds = records.reduce((a, r) => a + r.wall_s, 0);
  result.tests = records.length;
  const totals = {};
  for (const r of records)
    for (const [k, f] of Object.entries(r.parts)) totals[k] = (totals[k] ?? 0) + f * r.wall_s;
  result.test_category_seconds = totals;

  if (measure?.samples.length) {
    result.samples = summarizeSamples(measure.samples, steps, tasks);
    if (result.samples.counter_resets)
      note(
        `${result.samples.counter_resets} sampling interval(s) saw a CPU or network counter fall, so they are left out of the network and CPU figures`,
        "nothing to fix in the run; the figures cover the remaining intervals",
      );
  }
  if (measure?.overhead) result.overhead = summarizeOverhead(measure.overhead, records);
  result.measure_host = measure?.host ?? null;
  result.requests = gh.requests;
  result.rate_checks = gh.rateChecks;
  result.cached_reads = gh.cached;
  return result;
}

function unionSeconds(intervals) {
  const s = intervals.map((r) => [r.start, r.end]).sort((a, b) => a[0] - b[0]);
  let total = 0;
  let cur = null;
  for (const [a, b] of s) {
    if (!cur || a > cur[1]) {
      if (cur) total += cur[1] - cur[0];
      cur = [a, b];
    } else cur[1] = Math.max(cur[1], b);
  }
  if (cur) total += cur[1] - cur[0];
  return total / 1000;
}

// From the task that ended last, back through the dependency (from the Nx graph the run
// wrote) that ended last before it started. A start later than that dependency's end is time
// Nx held the task back for something other than its dependencies: a free slot, or, for a
// `parallelism: false` task, a runner with nothing else running. With no graph there is no
// path: the concurrency profile is reported instead.
function criticalPath(tasks, graph, gateStart) {
  const deps = graph?.tasks?.dependencies;
  if (!tasks.length || !deps) return [];
  const byId = new Map(tasks.map((t) => [t.task, t]));
  let cur = [...tasks].sort((a, b) => b.end - a.end)[0];
  const path = [];
  const seen = new Set();
  while (cur && !seen.has(cur.task)) {
    seen.add(cur.task);
    const pred = (deps[cur.task] ?? [])
      .map((d) => byId.get(d))
      .filter((t) => t && t.end <= cur.start + 1000)
      .sort((a, b) => b.end - a.end)[0];
    path.unshift({
      task: cur.task,
      start: new Date(cur.start).toISOString(),
      end: new Date(cur.end).toISOString(),
      s: (cur.end - cur.start) / 1000,
      after: pred?.task ?? "(gate start)",
      after_dependencies_s: Math.max(0, (cur.start - (pred ? pred.end : gateStart)) / 1000),
    });
    cur = pred;
  }
  return path;
}

// Seconds of the gate during which exactly k Nx tasks were running.
function concurrencyProfile(tasks, gate) {
  const profile = {};
  if (!gate) return profile;
  for (let t = gate.start; t < gate.end; t += 1000) {
    const k = tasks.filter((x) => x.start <= t + 500 && t + 500 < x.end).length;
    profile[k] = (profile[k] ?? 0) + 1;
  }
  return profile;
}

function summarizeSamples(samples, steps, tasks) {
  const ticks = [];
  let resets = 0;
  for (let i = 1; i < samples.length; i++) {
    const a = samples[i - 1];
    const b = samples[i];
    const dt = (b.t - a.t) / 1000;
    if (dt <= 0) continue;
    // Counters only grow; one that falls was reset (an interface or counter restarted), so
    // that interval says nothing about its traffic or CPU, and is noted rather than counted.
    const reset =
      b.rx < a.rx || b.tx < a.tx || (a.cpu?.total !== undefined && b.cpu?.total < a.cpu.total);
    if (reset) {
      resets++;
      continue;
    }
    let busy = null;
    if (a.cpu?.total !== undefined && b.cpu?.total !== undefined) {
      const total = b.cpu.total - a.cpu.total;
      busy = total > 0 ? 1 - (b.cpu.idle - a.cpu.idle) / total : null;
    } else if (b.cpu?.busy !== undefined) busy = b.cpu.busy;
    ticks.push({ t: a.t, dt, bytes: b.rx - a.rx + (b.tx - a.tx), busy });
  }
  const ACTIVE = 256 * 1024; // bytes a second
  const where = (tick) => {
    const mid = tick.t + (tick.dt * 1000) / 2;
    const s = steps.find((x) => !x.background && x.start <= mid && mid < x.end);
    if (!s) return "between steps";
    if (s.class !== "gate (Nx tasks)") return `step: ${s.class}`;
    const running = tasks
      .filter((x) => x.start <= mid && mid < x.end)
      .map((x) => x.task.replace(/^[^:]+:/, (p) => p));
    return running.length ? `gate: ${running.sort().join(" + ")}` : "gate: no task";
  };
  const network = {};
  let activeS = 0;
  let bytes = 0;
  const cpu = { s: 0, busy_s: 0 };
  const byK = {};
  for (const tick of ticks) {
    bytes += tick.bytes;
    if (tick.busy !== null) {
      cpu.s += tick.dt;
      cpu.busy_s += tick.busy * tick.dt;
      const mid = tick.t + (tick.dt * 1000) / 2;
      const gate = steps.find(
        (x) => x.class === "gate (Nx tasks)" && x.start <= mid && mid < x.end,
      );
      if (gate) {
        const k = tasks.filter((x) => x.start <= mid && mid < x.end).length;
        byK[k] ??= { s: 0, busy_s: 0 };
        byK[k].s += tick.dt;
        byK[k].busy_s += tick.busy * tick.dt;
      }
    }
    const k = where(tick);
    network[k] ??= { bytes: 0, active_s: 0 };
    network[k].bytes += tick.bytes;
    if (tick.bytes / tick.dt >= ACTIVE) {
      network[k].active_s += tick.dt;
      activeS += tick.dt;
    }
  }
  return {
    sampled_s: ticks.reduce((a, t) => a + t.dt, 0),
    network_bytes: bytes,
    network_active_s: activeS,
    network_active_threshold_bytes_per_s: ACTIVE,
    network_by_place: network,
    cpu_busy_fraction: cpu.s ? cpu.busy_s / cpu.s : null,
    counter_resets: resets,
    cpu_busy_by_concurrency: Object.fromEntries(
      Object.entries(byK).map(([k, v]) => [
        k,
        { s: v.s, busy_fraction: v.s ? v.busy_s / v.s : null },
      ]),
    ),
  };
}

function summarizeOverhead(overhead, gateRecords) {
  const ins = new Map(overhead.records.instrumented.map((r) => [`${r.binary}::${r.test}`, r]));
  const pla = new Map(overhead.records.plain.map((r) => [`${r.binary}::${r.test}`, r]));
  const both = [...ins.keys()].filter((k) => pla.has(k));
  const sum = (m) => both.reduce((a, k) => a + m.get(k).wall_s, 0);
  void gateRecords;
  return {
    runs: overhead.runs,
    tests_in_both: both.length,
    instrumented_test_seconds: sum(ins),
    plain_test_seconds: sum(pla),
    ratio: sum(pla) > 0 ? sum(ins) / sum(pla) : null,
  };
}

function printJob(r) {
  const out = [];
  const w = r.wall_s;
  out.push(
    `ci-time-profile: ${r.repo} run ${r.run} job ${r.job} (id ${r.job_id}, attempt ${r.attempt}, ${r.conclusion}) on ${r.runner} [${(r.labels ?? []).join(",")}]`,
  );
  out.push(
    `head ${r.head_sha} (${r.head_branch}, PR ${r.pull_requests.join(",") || "-"}); GITHUB_SHA ${r.github_sha ?? "not in log"}`,
  );
  out.push(`wall clock ${min(w)} min (${r.started_at} .. ${r.completed_at})`);
  if (r.selection)
    out.push(`Nx selection: ${r.selection.count} task(s): ${r.selection.tasks.join(" ")}`);
  out.push(`Nx task times from: ${r.task_source}`);
  if (Object.keys(r.nx_report).length)
    out.push(
      `Nx's own report: ${Object.entries(r.nx_report)
        .map(([k, v]) => `${k} ${v}`)
        .join("; ")}`,
    );
  out.push("", "## Steps, one after another (sums to the wall clock)", "");
  const classes = Object.entries(r.step_classes).sort((a, b) => b[1] - a[1]);
  out.push(
    table(
      ["class", "min", "share"],
      [
        ...classes.map(([k, s]) => [k, min(s), pct(s, w)]),
        ["between steps (runner)", min(r.between_steps_s), pct(r.between_steps_s, w)],
        ["**job wall clock**", min(w), "100%"],
      ],
    ),
  );
  out.push("", "| step | class | s |", "| --- | --- | --- |");
  for (const s of r.steps)
    out.push(
      `| ${s.name.replace(/\|/g, "/")} | ${s.class}${s.background ? " (background, overlaps)" : ""} | ${s.s.toFixed(0)} |`,
    );
  if (r.cache_lines.length) {
    out.push("", "## Cache restore and save lines", "");
    for (const c of r.cache_lines)
      out.push(`- ${new Date(c.t).toISOString()} ${c.text.slice(0, 200)}`);
  }
  out.push("", "## Writes", "");
  out.push(
    r.stored
      ? `Nx sidecar manifest: stored ${r.stored.entries} entries (${r.stored.bytes} bytes), replayed ${r.stored.replayed}`
      : "Nx sidecar manifest: not uploaded",
  );
  if (r.store_attempts) {
    const by = {};
    for (const a of r.store_attempts)
      by[`${a.status}/${a.http_status}`] = (by[`${a.status}/${a.http_status}`] ?? 0) + 1;
    out.push(`Nx store attempts by status/http: ${JSON.stringify(by)}`);
  }
  for (const l of r.read_only_lines) out.push(`- ${l.text.slice(0, 160)}`);

  out.push(
    "",
    `## Inside the gate step (${min(r.gate_s)} min): each moment shared among the Nx tasks running`,
    "",
  );
  const cats = Object.entries(r.gate_categories).sort((a, b) => b[1] - a[1]);
  out.push(
    table(
      ["category", "min", "share of job"],
      cats.map(([k, s]) => [k, min(s), pct(s, w)]),
    ),
  );
  const unattributed =
    r.between_steps_s + cats.filter(([k]) => /unattributed/.test(k)).reduce((a, [, s]) => a + s, 0);
  out.push("", `unattributed: ${min(unattributed)} min (${pct(unattributed, w)} of the job)`);

  out.push("", "## Nx tasks", "");
  out.push(
    table(
      [
        "task",
        "result",
        "starts at (s)",
        "own min",
        "avg concurrency",
        "fair-share min",
        "tests",
        "test-seconds",
      ],
      [...r.tasks]
        .sort((a, b) => b.share_s - a.share_s)
        .map((t) => [
          t.task,
          t.result,
          t.start_offset_s?.toFixed(0) ?? "-",
          min(t.s),
          t.avg_concurrency.toFixed(2),
          min(t.share_s),
          t.tests,
          t.test_seconds.toFixed(0),
        ]),
    ),
  );
  out.push("", "### Concurrency: seconds of the gate with k Nx tasks running", "");
  out.push(
    table(
      ["k", "min", "share of gate"],
      Object.entries(r.concurrency_profile).map(([k, n]) => [k, min(n), pct(n, r.gate_s)]),
    ),
  );
  out.push("", "### Critical path (from the last task back, through the Nx graph)", "");
  if (r.critical_path.length) {
    out.push(
      table(
        ["task", "own min", "after", "started after its dependencies ended (min)"],
        r.critical_path.map((c) => [c.task, min(c.s), c.after, min(c.after_dependencies_s)]),
      ),
    );
    const own = r.critical_path.reduce((a, c) => a + c.s, 0);
    const waited = r.critical_path.reduce((a, c) => a + c.after_dependencies_s, 0);
    out.push(
      `critical path: ${min(own)} min running, ${min(waited)} min between a task's last dependency ending and the task starting`,
    );
  } else
    out.push("no Nx graph in this run's artifacts, so no dependency path; see the profile above");

  out.push("", "## Compile and link (every rustc invocation)", "");
  if (Object.keys(r.compile).length) {
    out.push(
      table(
        [
          "tree",
          "invocations",
          "rustc seconds (summed elapsed)",
          "of which dependencies (s)",
          "wall with one running (min)",
          "top crates (s)",
        ],
        Object.entries(r.compile).map(([k, c]) => [
          k,
          c.invocations,
          c.rustc_s.toFixed(0),
          c.dependency_s.toFixed(0),
          min(c.wall_s),
          Object.entries(c.crates)
            .sort((a, b) => b[1] - a[1])
            .slice(0, 5)
            .map(([n, s]) => `${n} ${s.toFixed(0)}`)
            .join(", "),
        ]),
      ),
    );
    out.push(`any rustc running: ${min(r.compile_any_wall_s)} min of wall clock`);
  } else out.push("no rustc timings (the run carried no measure artifact)");

  out.push(
    "",
    `## Tests: ${r.tests} records, ${(r.test_seconds / 60).toFixed(1)} test-minutes`,
    "",
  );
  out.push(
    table(
      ["category", "test-minutes", "share of test time"],
      Object.entries(r.test_category_seconds)
        .sort((a, b) => b[1] - a[1])
        .map(([k, s]) => [k, min(s), pct(s, r.test_seconds)]),
    ),
  );
  out.push("", "### Programs the tests ran (count, test-thread seconds)", "");
  out.push(
    table(
      ["program", "count", "seconds"],
      Object.entries(r.programs)
        .sort((a, b) => b[1].s - a[1].s)
        .map(([p, v]) => [p, v.count, v.s.toFixed(0)]),
    ),
  );
  out.push("", "### Test binaries", "");
  out.push(
    table(
      ["binary", "tier", "task", "tests", "test-seconds", "elapsed share (min)"],
      r.binaries.map((b) => [
        b.binary,
        b.tier,
        b.task ?? "-",
        b.tests,
        b.test_seconds.toFixed(0),
        min(b.elapsed_share_s),
      ]),
    ),
  );
  out.push("", "### Every nextest run in the log (all tiers, records or not)", "");
  out.push(
    table(
      ["Nx task running at its start", "tests", "binaries", "nextest elapsed s"],
      r.nextest_runs
        .filter((n) => n.s >= 1)
        .map((n) => [n.tasks.join(" + ") || "-", n.tests, n.binaries ?? "-", n.s.toFixed(0)]),
    ),
  );
  if (Object.keys(r.log_binaries).length) {
    out.push("", "### Test binaries from the log's per-test lines (summed test seconds)", "");
    out.push(
      table(
        ["binary", "tests", "test-seconds"],
        Object.entries(r.log_binaries)
          .sort((a, b) => b[1].s - a[1].s)
          .map(([b, v]) => [b, v.tests, v.s.toFixed(0)]),
      ),
    );
  }
  out.push("", "### The 20 slowest tests", "");
  out.push(
    table(
      ["test", "wall s", "elapsed share s", "split (s)"],
      r.slowest.map((t) => [
        t.test,
        t.wall_s.toFixed(1),
        t.elapsed_share_s.toFixed(1),
        `${Object.entries(t.parts_s)
          .sort((a, b) => b[1] - a[1])
          .map(([k, s]) => `${k} ${s.toFixed(1)}`)
          .join("; ")}; polls ${t.polls}, sleeps ${t.sleeps}`,
      ]),
    ),
  );
  if (r.samples) {
    const s = r.samples;
    out.push("", "## Network and CPU (sampler)", "");
    out.push(
      `sampled ${min(s.sampled_s)} min; non-loopback bytes ${(s.network_bytes / 1e6).toFixed(0)} MB; seconds at >= ${s.network_active_threshold_bytes_per_s / 1024} KiB/s: ${s.network_active_s.toFixed(0)}; CPU busy ${((s.cpu_busy_fraction ?? 0) * 100).toFixed(0)}%`,
    );
    out.push(
      `CPU busy in the gate by number of Nx tasks running: ${Object.entries(
        s.cpu_busy_by_concurrency,
      )
        .map(
          ([k, v]) => `k=${k} ${((v.busy_fraction ?? 0) * 100).toFixed(0)}% over ${min(v.s)} min`,
        )
        .join("; ")}`,
    );
    out.push(
      table(
        ["where", "MB", "active s"],
        Object.entries(s.network_by_place)
          .sort((a, b) => b[1].bytes - a[1].bytes)
          .slice(0, 25)
          .map(([k, v]) => [k, (v.bytes / 1e6).toFixed(1), v.active_s.toFixed(0)]),
      ),
    );
  }
  if (r.overhead) {
    const o = r.overhead;
    out.push(
      "",
      "## Coverage instrumentation runtime overhead (one e2e partition, run twice after the gate)",
      "",
    );
    out.push(
      `runs: ${Object.entries(o.runs)
        .map(([k, v]) => `${k} ${v.s}s exit ${v.status}`)
        .join(
          "; ",
        )}; tests in both ${o.tests_in_both}; test-seconds instrumented ${o.instrumented_test_seconds.toFixed(0)}, plain ${o.plain_test_seconds.toFixed(0)}, ratio ${o.ratio?.toFixed(3) ?? "-"}`,
    );
  }
  if (NOTES.length) out.push("", "## Notes", "", ...NOTES.map((n) => `- ${n}`));
  out.push(
    "",
    `GitHub requests this invocation: ${r.requests} (rate checks, which the allowance does not count: ${r.rate_checks}; read from cache: ${r.cached_reads})`,
  );
  return out.join("\n");
}

function history(opts) {
  const gh = new Gh(opts.cache, opts["min-remaining"]);
  const since = new Date(opts.since).toISOString();
  const until = new Date(opts.until).toISOString();
  const split = ms(opts.split);
  // The populations compared may start later than the listing does, so one cached listing
  // serves every split: runs created before --compare-since are listed and not counted.
  const compareSince = ms(opts["compare-since"] ?? opts.since);
  if (Number.isNaN(compareSince)) usage("--compare-since takes a date");
  const runs = [];
  for (let page = 1; page <= 10; page++) {
    const r = gh.json(
      `repos/${opts.repo}/actions/workflows/ci.yml/runs?event=pull_request&status=completed&per_page=100&page=${page}&created=${since.slice(0, 19)}..${until.slice(0, 19)}`,
    );
    check(
      Array.isArray(r?.workflow_runs) &&
        r.workflow_runs.every(
          (run) =>
            Number.isInteger(run?.id) &&
            Number.isInteger(run.run_attempt) &&
            !Number.isNaN(ms(run.created_at)) &&
            /^[0-9a-f]{40}$/.test(run.head_sha ?? "") &&
            (run.conclusion === null || typeof run.conclusion === "string"),
        ),
      `ci.yml's run listing, page ${page}`,
    );
    runs.push(...r.workflow_runs);
    if (r.workflow_runs.length < 100) break;
  }
  // Filtered by date, event and status, GitHub lists at most 1,000 runs (ten pages).
  if (runs.length >= 1000)
    note(
      `the listing holds ${runs.length} runs, GitHub's limit for a filtered listing, so runs past it are not counted`,
      "narrow --since and --until so each listing holds fewer than 1,000 runs",
    );
  const rows = [];
  for (const run of runs) {
    if (ms(run.created_at) < compareSince) continue;
    // A cancelled run's jobs were superseded by a later push, so none is a duration of record.
    if (run.conclusion === "cancelled" || run.conclusion === "skipped") continue;
    const jobs = gh.json(
      `repos/${opts.repo}/actions/runs/${run.id}/attempts/${run.run_attempt}/jobs?per_page=100`,
    );
    check(
      Array.isArray(jobs?.jobs) &&
        jobs.jobs.every(
          (j) =>
            typeof j?.name === "string" &&
            (j.conclusion !== "success" || ms(j.completed_at) >= ms(j.started_at)),
        ),
      `run ${run.id}'s jobs`,
    );
    for (const j of jobs.jobs) {
      if (!opts.jobs.includes(j.name) || j.conclusion !== "success") continue;
      rows.push({
        run: run.id,
        attempt: run.run_attempt,
        pr: run.pull_requests?.[0]?.number ?? null,
        head_sha: run.head_sha,
        created_at: run.created_at,
        job: j.name,
        minutes: (ms(j.completed_at) - ms(j.started_at)) / 60000,
        side: ms(run.created_at) < split ? "before" : "after",
      });
    }
  }
  // What each run's pull request changed, from the local clone: the head is fetched over git
  // (no API request) and diffed against its merge base with --base-ref.
  const baseRef = opts["base-ref"] ?? "origin/main";
  const classes = new Map();
  // Quiet: a missing commit is fetched, and git's own complaint about it is not the answer.
  const git = (...a) =>
    execFileSync("git", a, { encoding: "utf8", stdio: ["ignore", "pipe", "pipe"] }).trim();
  const why = (error) =>
    String(error.stderr || error.message)
      .trim()
      .split("\n")[0];
  for (const sha of new Set(rows.map((r) => r.head_sha))) {
    let cls;
    try {
      try {
        git("cat-file", "-e", `${sha}^{commit}`);
      } catch {
        git("fetch", "-q", "origin", sha);
      }
      const files = git("diff", "--name-only", git("merge-base", sha, baseRef), sha)
        .split("\n")
        .filter(Boolean);
      cls =
        files.length && files.every((f) => /^(Cargo\.(toml|lock)|CHANGELOG\.md)$/.test(f))
          ? "release"
          : files.some((f) => f.startsWith("src/"))
            ? "edits src/"
            : files.some((f) => /^(Cargo\.(toml|lock)|crates\/)/.test(f))
              ? "edits crates/ or Cargo files, not src/"
              : "other";
    } catch (error) {
      cls = "unknown";
      note(
        `head ${sha} could not be classed: ${why(error)}`,
        `run from a clone of ${opts.repo} that can fetch it, with ${baseRef} fetched`,
      );
    }
    classes.set(sha, cls);
  }
  for (const r of rows) r.class = classes.get(r.head_sha);
  // With --profiles CLASS, each counted job of that class also reads its test-profile
  // artifact: the e2e tier's size and cost, and what one spawn of `onepipeline` took, which
  // tells two runner machines behind one label apart.
  if (opts.profiles) {
    const listed = new Map();
    for (const r of rows.filter((x) => x.class === opts.profiles)) {
      if (!listed.has(r.run))
        listed.set(
          r.run,
          gh.json(`repos/${opts.repo}/actions/runs/${r.run}/artifacts?per_page=100`)?.artifacts,
        );
      check(
        Array.isArray(listed.get(r.run)) &&
          listed
            .get(r.run)
            .every(
              (x) =>
                Number.isInteger(x?.id) &&
                typeof x.name === "string" &&
                typeof x.expired === "boolean",
            ),
        `run ${r.run}'s artifact list`,
      );
      const a = listed.get(r.run).find((x) => x.name === `test-profile-${r.job}` && !x.expired);
      if (!a) {
        note(
          `run ${r.run} has no unexpired test-profile-${r.job} artifact, so its e2e figures are blank`,
          "artifacts expire after 30 days; read history inside that window",
        );
        continue;
      }
      const dir = unzipArtifact(
        gh,
        opts.repo,
        a,
        join(opts.cache, "runs", String(r.run), `attempt-${r.attempt}`, r.job, a.name),
      );
      const e2e = readRecords(walk(dir)).filter((x) => x.binary === "e2e");
      let count = 0;
      let seconds = 0;
      for (const x of e2e) {
        count += x.programs?.onepipeline?.count ?? 0;
        seconds += x.programs?.onepipeline?.s ?? 0;
      }
      r.e2e_tests = e2e.length;
      r.e2e_test_seconds = e2e.reduce((t, x) => t + x.wall_s, 0);
      r.onepipeline_spawn_s = count ? seconds / count : null;
    }
  }
  const summary = {};
  for (const job of opts.jobs)
    for (const cls of ["all", ...new Set(rows.map((r) => r.class))])
      for (const side of ["before", "after"]) {
        const v = rows
          .filter((r) => r.job === job && r.side === side && (cls === "all" || r.class === cls))
          .map((r) => r.minutes);
        summary[`${job}\t${cls}\t${side}`] = {
          n: v.length,
          median_min: median(v),
          min_min: v.length ? Math.min(...v) : null,
          max_min: v.length ? Math.max(...v) : null,
        };
      }
  return {
    runs: runs.length,
    rows,
    summary,
    requests: gh.requests,
    rate_checks: gh.rateChecks,
    cached_reads: gh.cached,
  };
}

function printHistory(h, opts) {
  const out = [
    `ci-time-profile history: ${opts.repo} ci.yml pull_request runs created ${opts.since} .. ${opts.until}, split at ${opts.split}: ${h.runs} runs`,
    "",
  ];
  out.push(
    table(
      ["job", "pull request changed", "side", "n", "median min", "min", "max"],
      Object.entries(h.summary).map(([k, v]) => {
        const [job, cls, side] = k.split("\t");
        return [
          job,
          cls,
          side,
          v.n,
          v.median_min?.toFixed(1) ?? "-",
          v.min_min?.toFixed(1) ?? "-",
          v.max_min?.toFixed(1) ?? "-",
        ];
      }),
    ),
  );
  out.push(
    "",
    table(
      [
        "run",
        "attempt",
        "head",
        "created",
        "pull request changed",
        "job",
        "min",
        "side",
        "e2e tests",
        "e2e test-s",
        "s per onepipeline spawn",
      ],
      h.rows.map((r) => [
        r.run,
        r.attempt,
        r.head_sha.slice(0, 8),
        r.created_at,
        r.class,
        r.job,
        r.minutes.toFixed(1),
        r.side,
        r.e2e_tests ?? "-",
        r.e2e_test_seconds?.toFixed(0) ?? "-",
        r.onepipeline_spawn_s?.toFixed(3) ?? "-",
      ]),
    ),
  );
  if (NOTES.length) out.push("", "## Notes", "", ...NOTES.map((n) => `- ${n}`));
  out.push(
    "",
    `GitHub requests this invocation: ${h.requests} (rate checks, which the allowance does not count: ${h.rate_checks}; read from cache: ${h.cached_reads})`,
  );
  return out.join("\n");
}

function write(file, text) {
  try {
    writeFileSync(file, text);
  } catch (error) {
    refuse(
      `cannot write ${file}: ${error.message}`,
      "name a file in a directory that exists and that you can write",
    );
  }
}

function main() {
  const opts = parse(process.argv.slice(2));
  const result = opts.command === "job" ? analyzeJob(opts) : history(opts);
  result.notes = NOTES;
  if (opts.json) write(opts.json, `${JSON.stringify(result, null, 2)}\n`);
  write(opts.out, `${opts.command === "job" ? printJob(result) : printHistory(result, opts)}\n`);
  const reads = `GitHub requests ${result.requests}, rate checks ${result.rate_checks}, read from cache ${result.cached_reads}`;
  const noted = NOTES.length
    ? `; ${NOTES.length} note(s) on what could not be read, in the report`
    : "";
  if (opts.command === "job") {
    const unattributed =
      result.between_steps_s +
      Object.entries(result.gate_categories)
        .filter(([k]) => /unattributed/.test(k))
        .reduce((a, [, v]) => a + v, 0);
    process.stdout.write(
      `ci-time-profile: run ${result.run} ${result.job}: wall ${min(result.wall_s)} min, gate ${min(result.gate_s)} min, unattributed ${pct(unattributed, result.wall_s)}; report in ${opts.out} (${reads})${noted}\n`,
    );
  } else {
    process.stdout.write(
      `ci-time-profile history: ${result.runs} runs listed, ${result.rows.length} successful jobs counted; report in ${opts.out} (${reads})${noted}\n`,
    );
  }
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  try {
    main();
  } catch (error) {
    if (error instanceof Refusal) {
      process.stderr.write(`ci-time-profile: ${error.message}\nnext: ${error.next}\n`);
    } else {
      process.stderr.write(
        `ci-time-profile: ${error.message}\nnext: this is a defect in the harness, not in the run; run the same command again with the run id, and report the message if it recurs\n`,
      );
    }
    process.exit(1);
  }
}
