#!/usr/bin/env node
// Spike harness for `spike-onepipeline-test-cost` (throwaway; lands nowhere).
//
// One measurement, one command:
//
//   node spikes/test-cost/measure.mjs compare --side NAME=RUN[,RUN...] [--side ...]
//        [--cache DIR] [--tests SET=FILE ...] [--repo OWNER/NAME] [--json]
//
// For every hosted run named, it reads (through `gh`, read-only, cached under
// --cache so a second invocation makes no request) the run, its jobs, its log
// archive and its `test-profile-test-*` artifacts, and prints, per side and per
// platform job: the job's minutes, the e2e tier's nextest seconds and counts, the
// line coverage (Linux), the per-verb engine invocation counts and seconds out of
// the profile records, each cache restore's hit or miss and time, the evidence
// that nothing was saved, the run's GITHUB_SHA, head SHA, executed workflow blob
// and Nx task selection, and whether every side ran and passed the same e2e tests.
//
//   node spikes/test-cost/measure.mjs verbs DIR [DIR...]
//
// prints the same per-verb engine breakdown for local `target/test-profile/<bin>`
// record directories.
//
// `--tests SET=FILE` names a file of test names (one `module::test` per line,
// `#` comments allowed); the seconds those tests took are summed per side and
// platform, which is how the pyramid sample's replaced journeys and their
// in-process replacements are priced against each other.

import { execFileSync } from "node:child_process";
import { existsSync, mkdirSync, readdirSync, readFileSync, statSync, writeFileSync } from "node:fs";
import { join, resolve } from "node:path";

const JOBS = ["test-linux", "test-macos", "test-windows"];
const PLATFORM = { "test-linux": "linux", "test-macos": "macos", "test-windows": "windows" };

function fail(message) {
  process.stderr.write(`measure: ${message}\n`);
  process.exit(2);
}

function parseArgs(argv) {
  const [mode, ...rest] = argv;
  const options = { mode, sides: [], tests: [], cache: null, repo: "nickderobertis/onepipeline", json: false, dirs: [] };
  for (let i = 0; i < rest.length; i++) {
    const arg = rest[i];
    const value = () => {
      if (i + 1 >= rest.length) fail(`${arg} needs a value`);
      return rest[++i];
    };
    if (arg === "--side") {
      const [name, runs] = value().split("=");
      if (!name || !runs) fail("--side takes NAME=RUN[,RUN...]");
      options.sides.push({ name, runs: runs.split(",").filter(Boolean) });
    } else if (arg === "--tests") {
      const [name, file] = value().split("=");
      if (!name || !file) fail("--tests takes SET=FILE");
      options.tests.push({ name, names: readTestList(file) });
    } else if (arg === "--cache") options.cache = resolve(value());
    else if (arg === "--repo") options.repo = value();
    else if (arg === "--json") options.json = true;
    else if (arg.startsWith("--")) fail(`unknown option ${arg}`);
    else options.dirs.push(arg);
  }
  return options;
}

function readTestList(file) {
  return readFileSync(file, "utf8")
    .split("\n")
    .map((line) => line.replace(/#.*/, "").trim())
    .filter(Boolean);
}

// ---------------------------------------------------------------- records

function readRecords(dir) {
  const records = [];
  if (!existsSync(dir)) return records;
  for (const name of readdirSync(dir)) {
    const path = join(dir, name);
    if (statSync(path).isDirectory()) records.push(...readRecords(path));
    else if (name.endsWith(".json")) {
      try {
        const record = JSON.parse(readFileSync(path, "utf8"));
        if (record && record.schema && record.test) records.push(record);
      } catch {
        // A half-written record is not a measurement.
      }
    }
  }
  return records;
}

/// The run id most records share: a directory holding stray records of an
/// earlier run on the same runner keeps only this run's.
function ofOneRun(records) {
  const counts = new Map();
  for (const record of records) counts.set(record.run_id, (counts.get(record.run_id) ?? 0) + 1);
  const [run] = [...counts.entries()].sort((a, b) => b[1] - a[1])[0] ?? [];
  return records.filter((record) => record.run_id === run);
}

function summarizeRecords(records, binary = "e2e") {
  const own = ofOneRun(records.filter((record) => record.binary === binary));
  const verbs = new Map();
  const programs = new Map();
  let wall = 0;
  let wait = 0;
  let spawn = 0;
  const tests = new Map();
  for (const record of own) {
    wall += record.wall_s;
    wait += record.categories?.wait?.s ?? 0;
    spawn += record.categories?.spawn?.s ?? 0;
    tests.set(record.test, (tests.get(record.test) ?? 0) + record.wall_s);
    for (const [program, entry] of Object.entries(record.programs ?? {})) {
      const into = program.startsWith("onepipeline:") ? verbs : programs;
      const key = program.startsWith("onepipeline:") ? program.slice("onepipeline:".length) : program;
      const total = into.get(key) ?? { count: 0, s: 0 };
      total.count += entry.count;
      total.s += entry.s;
      into.set(key, total);
    }
  }
  return { records: own.length, wall, wait, spawn, tests, verbs, programs };
}

function verbTable(summary, top = 15) {
  const engine = summary.programs.get("onepipeline") ?? { count: 0, s: 0 };
  const rows = [...summary.verbs.entries()].sort((a, b) => b[1].s - a[1].s).slice(0, top);
  const lines = [
    `engine invocations: ${engine.count}, ${engine.s.toFixed(1)} s (${pct(engine.s, summary.wall)} of ${summary.wall.toFixed(1)} e2e test-s)`,
    "",
    "| verb | count | s | mean ms | share of engine s |",
    "| --- | ---: | ---: | ---: | ---: |",
  ];
  for (const [verb, total] of rows) {
    lines.push(`| ${verb} | ${total.count} | ${total.s.toFixed(1)} | ${((total.s / Math.max(total.count, 1)) * 1000).toFixed(0)} | ${pct(total.s, engine.s)} |`);
  }
  return lines.join("\n");
}

function pct(part, whole) {
  return whole > 0 ? `${((100 * part) / whole).toFixed(1)}%` : "–";
}

// ---------------------------------------------------------------- hosted runs

function gh(args, { binary = false } = {}) {
  return execFileSync("gh", args, { encoding: binary ? "buffer" : "utf8", maxBuffer: 1 << 30, stdio: ["ignore", "pipe", "pipe"] });
}

function cached(cache, name, produce) {
  const path = join(cache, name);
  if (!existsSync(path)) {
    mkdirSync(cache, { recursive: true });
    writeFileSync(path, produce());
  }
  return path;
}

function fetchRun(options, run) {
  const dir = join(options.cache, String(run));
  mkdirSync(dir, { recursive: true });
  const meta = JSON.parse(
    readFileSync(cached(dir, "run.json", () => gh(["api", `repos/${options.repo}/actions/runs/${run}`])), "utf8"),
  );
  if (meta.status !== "completed") return { run, meta, incomplete: true };
  const jobs = JSON.parse(
    readFileSync(cached(dir, "jobs.json", () => gh(["api", `repos/${options.repo}/actions/runs/${run}/jobs?per_page=50&filter=all`])), "utf8"),
  ).jobs;
  const logs = join(dir, "logs");
  if (!existsSync(logs)) {
    const zip = cached(dir, "logs.zip", () => gh(["api", `repos/${options.repo}/actions/runs/${run}/attempts/${meta.run_attempt}/logs`], { binary: true }));
    mkdirSync(logs, { recursive: true });
    execFileSync("unzip", ["-o", "-q", zip, "-d", logs]);
  }
  const artifacts = join(dir, "artifacts");
  if (!existsSync(join(artifacts, ".done"))) {
    mkdirSync(artifacts, { recursive: true });
    for (const job of JOBS) {
      try {
        gh(["run", "download", String(run), "--repo", options.repo, "-n", `test-profile-${job}`, "-D", join(artifacts, job)]);
      } catch {
        // A job that never reached its upload has no records; reported as such below.
      }
    }
    writeFileSync(join(artifacts, ".done"), "");
  }
  return { run, meta, jobs, logs, artifacts };
}

function jobLog(fetched, job) {
  const top = readdirSync(fetched.logs).find((name) => name.endsWith(`_${job}.txt`));
  if (!top) return null;
  return readFileSync(join(fetched.logs, top), "utf8").replace(/\x1b\[[0-9;]*m/g, "").replace(/\r/g, "");
}

const STAMP = /^(\d{4}-\d\d-\d\dT\d\d:\d\d:\d\d(?:\.\d+)?Z) ?/;
function at(line) {
  const match = STAMP.exec(line);
  return match ? Date.parse(match[1]) : null;
}

/// Each step as the log marks it: `##[group]Run X` opens one; the next opens the
/// next. Its seconds run to the next step's first line.
function steps(log) {
  const lines = log.split("\n");
  const found = [];
  for (let i = 0; i < lines.length; i++) {
    const match = /##\[group\]Run (.*)$/.exec(lines[i]);
    if (match && !/^\s/.test(lines[i].replace(STAMP, ""))) found.push({ name: match[1].trim(), start: at(lines[i]), line: i });
  }
  for (let i = 0; i < found.length; i++) {
    const end = i + 1 < found.length ? found[i + 1].start : at(lines[lines.length - 2] ?? "");
    found[i].seconds = end && found[i].start ? (end - found[i].start) / 1000 : null;
    found[i].text = lines.slice(found[i].line, i + 1 < found.length ? found[i + 1].line : lines.length).join("\n");
  }
  return found;
}

function cacheSteps(log) {
  const out = [];
  for (const step of steps(log)) {
    let kind = null;
    if (step.name.startsWith("Swatinem/rust-cache")) kind = "rust-cache";
    else if (step.name.startsWith("actions/cache/restore")) kind = "actions/cache/restore";
    else if (step.name.startsWith("astral-sh/setup-uv")) kind = "setup-uv";
    if (!kind) continue;
    const text = step.text;
    let outcome = "unknown";
    if (/full match: true/.test(text)) outcome = "hit (full)";
    else if (/full match: false/.test(text)) outcome = "hit (partial key)";
    else if (/Cache restored from key|uv cache restored|Cache restored successfully/.test(text)) outcome = "hit";
    else if (/No cache found|Cache not found|No GitHub Actions cache found/.test(text)) outcome = "miss";
    out.push({ kind, outcome, seconds: step.seconds });
  }
  return out;
}

/// What the post-job steps and the sidecar say about saving: every line naming a
/// save, so a reader sees the job's own words rather than this script's verdict.
function saveEvidence(log) {
  return log
    .split("\n")
    .filter((line) => /Post job cleanup|save-if|save-cache|Saving cache|Cache saved|not saving|Not saving|CACHE_READ_ONLY|read_only=|read-only/i.test(line))
    .filter((line) => !/# Subtest|ok \d+ -|not ok \d+ -/.test(line))
    .map((line) => line.replace(STAMP, "").trim())
    .filter((line, i, all) => all.indexOf(line) === i)
    .slice(0, 40);
}

function nextestTier(log, project) {
  const lines = log.split("\n");
  const start = lines.findIndex((line) => new RegExp(`> nx run ${project}:(test|test-quick)\\b`).test(line));
  if (start < 0) return null;
  for (let i = start; i < lines.length; i++) {
    const match = /Summary \[\s*([\d.]+)s\] (\d+) tests? run: (\d+) passed(?:.*?(\d+) failed)?/.exec(lines[i]);
    if (match) {
      return {
        seconds: Number(match[1]),
        run: Number(match[2]),
        passed: Number(match[3]),
        failed: Number(match[4] ?? 0),
        task_seconds: (at(lines[i]) - at(lines[start])) / 1000,
      };
    }
  }
  return null;
}

function headOf(log) {
  const match = /HEAD is now at ([0-9a-f]+) Merge ([0-9a-f]{40}) into ([0-9a-f]{40})/.exec(log);
  return match ? { merge: match[1], head: match[2], base: match[3] } : null;
}

function workflowBlob(sha) {
  try {
    return execFileSync("git", ["rev-parse", `${sha}:.github/workflows/ci.yml`], { encoding: "utf8", stdio: ["ignore", "pipe", "ignore"] }).trim();
  } catch {
    return "(commit not in this clone)";
  }
}

function measureRun(options, run) {
  const fetched = fetchRun(options, run);
  if (fetched.incomplete) return { run, incomplete: true, status: fetched.meta.status };
  const out = {
    run,
    attempt: fetched.meta.run_attempt,
    head_sha: fetched.meta.head_sha,
    head_branch: fetched.meta.head_branch,
    workflow_blob: workflowBlob(fetched.meta.head_sha),
    jobs: {},
  };
  for (const job of JOBS) {
    const meta = fetched.jobs.find((entry) => entry.name === job);
    const log = jobLog(fetched, job);
    if (!meta || !log) {
      out.jobs[job] = null;
      continue;
    }
    const head = headOf(log);
    const tasks = /ci-job: \S+ runs (\d+) task\(s\): (.*)$/m.exec(log);
    const coverage = /lines percent: ([\d.]+)/.exec(log);
    const records = summarizeRecords(readRecords(join(fetched.artifacts, job)));
    out.jobs[job] = {
      conclusion: meta.conclusion,
      minutes: (Date.parse(meta.completed_at) - Date.parse(meta.started_at)) / 60000,
      github_sha: head?.merge ?? null,
      merge_of: head ? `${head.head} into ${head.base}` : null,
      nx_tasks: tasks ? { count: Number(tasks[1]), list: tasks[2].trim() } : null,
      e2e: nextestTier(log, "onepipeline-e2e"),
      coverage_lines_percent: coverage ? Number(coverage[1]) : null,
      caches: cacheSteps(log),
      save_evidence: saveEvidence(log),
      records,
    };
  }
  return out;
}

function mean(values) {
  const real = values.filter((value) => typeof value === "number" && !Number.isNaN(value));
  return real.length ? real.reduce((a, b) => a + b, 0) / real.length : null;
}

function fmt(value, digits = 1) {
  return typeof value === "number" ? value.toFixed(digits) : "–";
}

function compare(options) {
  if (!options.cache) options.cache = resolve(process.env.ONEPIPELINE_NODE_SCRATCH_DIR ?? ".", "test-cost-runs");
  if (!options.sides.length) fail("name at least one --side NAME=RUN[,RUN...]");
  const sides = options.sides.map((side) => ({ ...side, measured: side.runs.map((run) => measureRun(options, run)) }));
  if (options.json) {
    process.stdout.write(`${JSON.stringify(sides, (key, value) => (value instanceof Map ? Object.fromEntries(value) : value), 2)}\n`);
    return;
  }
  const print = (line = "") => process.stdout.write(`${line}\n`);
  print("## Runs");
  print();
  print("| side | run | attempt | job | conclusion | GITHUB_SHA | head SHA | ci.yml blob | Nx tasks |");
  print("| --- | --- | --- | --- | --- | --- | --- | --- | ---: |");
  for (const side of sides) {
    for (const run of side.measured) {
      if (run.incomplete) {
        print(`| ${side.name} | ${run.run} | – | – | ${run.status} | – | – | – | – |`);
        continue;
      }
      for (const job of JOBS) {
        const j = run.jobs[job];
        print(`| ${side.name} | ${run.run} | ${run.attempt} | ${job} | ${j?.conclusion ?? "absent"} | ${j?.github_sha ?? "–"} | ${run.head_sha.slice(0, 12)} | ${run.workflow_blob.slice(0, 12)} | ${j?.nx_tasks?.count ?? "–"} |`);
      }
    }
  }
  print();
  print("## Minutes per platform (each run, then the side's mean)");
  print();
  print("| side | job | runs: job min | runs: e2e nextest min | runs: e2e tests run/passed/failed | runs: Linux line coverage % | mean job min | mean e2e min |");
  print("| --- | --- | --- | --- | --- | --- | ---: | ---: |");
  const means = {};
  for (const side of sides) {
    means[side.name] = {};
    for (const job of JOBS) {
      const js = side.measured.filter((run) => !run.incomplete).map((run) => run.jobs[job]).filter(Boolean);
      const jobMin = js.map((j) => j.minutes);
      const e2eMin = js.map((j) => (j.e2e ? j.e2e.seconds / 60 : null));
      means[side.name][job] = { job: mean(jobMin), e2e: mean(e2eMin) };
      print(`| ${side.name} | ${job} | ${jobMin.map((v) => fmt(v)).join(", ")} | ${e2eMin.map((v) => fmt(v)).join(", ")} | ${js.map((j) => (j.e2e ? `${j.e2e.run}/${j.e2e.passed}/${j.e2e.failed}` : "–")).join(", ")} | ${js.map((j) => fmt(j.coverage_lines_percent, 4)).join(", ")} | ${fmt(means[side.name][job].job)} | ${fmt(means[side.name][job].e2e)} |`);
    }
  }
  const [base, ...others] = sides;
  if (others.length) {
    print();
    print(`## Against \`${base.name}\` (mean minus mean; negative saves)`);
    print();
    print("| side | job | job min | e2e min | spread of the two sides' runs (job min) |");
    print("| --- | --- | ---: | ---: | --- |");
    for (const side of others) {
      for (const job of JOBS) {
        const a = means[base.name][job];
        const b = means[side.name][job];
        const spread = (s) => {
          const v = s.measured.filter((r) => !r.incomplete).map((r) => r.jobs[job]?.minutes).filter((x) => typeof x === "number");
          return v.length ? `${fmt(Math.min(...v))}–${fmt(Math.max(...v))}` : "–";
        };
        print(`| ${side.name} | ${job} | ${a.job != null && b.job != null ? fmt(b.job - a.job) : "–"} | ${a.e2e != null && b.e2e != null ? fmt(b.e2e - a.e2e) : "–"} | ${base.name} ${spread(base)}; ${side.name} ${spread(side)} |`);
      }
    }
  }
  print();
  print("## The same e2e tests on every run of every side (from the profile records)");
  print();
  for (const job of JOBS) {
    const sets = [];
    for (const side of sides) for (const run of side.measured) if (!run.incomplete && run.jobs[job]) sets.push({ label: `${side.name}/${run.run}`, tests: new Set(run.jobs[job].records.tests.keys()) });
    if (!sets.length) continue;
    const [first, ...rest] = sets;
    const differing = rest
      .map((set) => ({
        label: set.label,
        missing: [...first.tests].filter((t) => !set.tests.has(t)).length,
        extra: [...set.tests].filter((t) => !first.tests.has(t)).length,
      }))
      .filter((d) => d.missing || d.extra);
    print(`- ${job}: ${sets.map((s) => `${s.label} ${s.tests.size}`).join(", ")} records; ${differing.length ? `differs from ${first.label}: ${differing.map((d) => `${d.label} −${d.missing}/+${d.extra}`).join(", ")}` : `identical test-name sets`}`);
  }
  for (const set of options.tests) {
    print();
    print(`## Test set \`${set.name}\` (${set.names.length} tests): summed record seconds`);
    print();
    print("| side | run | job | tests found | s |");
    print("| --- | --- | --- | ---: | ---: |");
    for (const side of sides) {
      for (const run of side.measured) {
        if (run.incomplete) continue;
        for (const job of JOBS) {
          const j = run.jobs[job];
          if (!j) continue;
          let found = 0;
          let seconds = 0;
          for (const name of set.names) {
            if (j.records.tests.has(name)) {
              found++;
              seconds += j.records.tests.get(name);
            }
          }
          print(`| ${side.name} | ${run.run} | ${job} | ${found} | ${fmt(seconds)} |`);
        }
      }
    }
  }
  print();
  print("## Engine invocations per verb (first run of each side)");
  for (const side of sides) {
    const run = side.measured.find((r) => !r.incomplete);
    if (!run) continue;
    for (const job of JOBS) {
      const j = run.jobs[job];
      if (!j || !j.records.records) continue;
      print();
      print(`### ${side.name} ${run.run} ${job}`);
      print();
      print(j.records.verbs.size ? verbTable(j.records) : `(no per-verb entries in these records; engine total ${fmt(j.records.programs.get("onepipeline")?.s)} s)`);
    }
  }
  print();
  print("## Cache restores and saves");
  print();
  print("| side | run | job | restores (kind: outcome, s) | save evidence |");
  print("| --- | --- | --- | --- | --- |");
  for (const side of sides) {
    for (const run of side.measured) {
      if (run.incomplete) continue;
      for (const job of JOBS) {
        const j = run.jobs[job];
        if (!j) continue;
        print(`| ${side.name} | ${run.run} | ${job} | ${j.caches.map((c) => `${c.kind}: ${c.outcome}, ${fmt(c.seconds)}`).join("; ")} | ${j.save_evidence.join(" / ").replace(/\|/g, "\\|")} |`);
      }
    }
  }
}

function verbs(options) {
  if (!options.dirs.length) fail("name at least one record directory");
  for (const dir of options.dirs) {
    const records = readRecords(dir);
    const binary = records[0]?.binary ?? "e2e";
    const summary = summarizeRecords(records, binary);
    process.stdout.write(`## ${dir} (${summary.records} ${binary} records)\n\n${verbTable(summary, 30)}\n\n`);
  }
}

const options = parseArgs(process.argv.slice(2));
if (options.mode === "compare") compare(options);
else if (options.mode === "verbs") verbs(options);
else fail("usage: measure.mjs compare --side NAME=RUN[,RUN...] [...] | measure.mjs verbs DIR [DIR...]");
