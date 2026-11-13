#!/usr/bin/env node
// Where a CI test job's wall clock goes, and what one process spawn costs on the host
// it runs on: the harness of the `spike-onepipeline-windows` measurement.
//
//   node scripts/windows-job-profile.mjs fold [--baseline LABEL] [--top N] LABEL=DIR [LABEL=DIR ...]
//   node scripts/windows-job-profile.mjs spawn [--runs N] [--json] [--program NAME=PATH ...]
//   node scripts/windows-job-profile.mjs outcomes BASELINE=LOG LABEL=LOG ...
//
// `fold` replays one hosted job per LABEL=DIR. DIR holds what that job left behind:
//   job.log          its log, as `gh api repos/<o>/<r>/actions/jobs/<id>/logs` serves it
//                    (or `gh run view --log`, whose two leading columns are dropped);
//   test-profile/    its `test-profile-<job>` artifact, unzipped (optional);
//   executions.json  from its `nx-cache-entries-<job>` artifact (optional).
// It prints, in Markdown, the job's wall clock divided so no minute is counted twice:
// the steps around the gate (setup, cache restore, bootstrap, cache save, the rest); the
// gate's Nx critical path, each task on it split into its build phase and its test phase;
// each test phase split by what its tests' profile records spent (each other CLI's spawns
// by name, git, build, fixtures, waits and polls, filesystem, unattributed), scaled from
// test-seconds to that phase's wall seconds; and the remainder, named unattributed. Then
// every Nx task with its result and minutes, each cargo build's elapsed time, each test
// binary's seconds, and the slowest tests with their split. With a `--baseline` label,
// each figure is set beside that job's and the difference is given.
//
// `spawn` starts each CLI the suite's tests start, `--runs` times (default 30), and prints
// the median, p10 and p90 wall milliseconds of one spawn-to-exit of its cheapest
// invocation (`--version`, or an empty script), with the path it resolved to. A start that
// exits non-zero still counts: it is the process start that is timed.
//
// `outcomes` reads each log's nextest result lines and says whether every test the first
// (baseline) log passed also ran and passed in each of the others.
//
// Exit 0 once the report is printed; 1 with the reason and a `next:` line on stderr when an
// input cannot be read; 2 for a usage error.
import { spawnSync } from "node:child_process";
import { existsSync, readdirSync, readFileSync, statSync } from "node:fs";
import { delimiter, join } from "node:path";
import { pathToFileURL } from "node:url";

/** A failure the command explains, with what to do about it. */
export class Refusal extends Error {
  constructor(message, next, code = 1) {
    super(message);
    this.next = next;
    this.code = code;
  }
}

const TIMESTAMP = /(\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:\.\d+)?Z) ?(.*)$/;
// biome-ignore lint/suspicious/noControlCharactersInRegex: a job log carries ANSI colour codes.
const ANSI = /\u001b\[[0-9;]*[A-Za-z]/g;
const CATEGORIES = ["spawn", "git", "build", "fixture", "wait", "fs"];

/** A log's lines as `{ t, text }`, its timestamp in epoch seconds and ANSI codes stripped. */
export function parseLog(raw) {
  const lines = [];
  for (const line of raw.split(/\r?\n/)) {
    const match = TIMESTAMP.exec(line);
    if (!match) continue;
    lines.push({ t: Date.parse(match[1]) / 1000, text: match[2].replace(ANSI, "") });
  }
  return lines;
}

const STEP_START = /^##\[group\]Run (.*)$/;
const POST_START = /^Post job cleanup\.$/;

/**
 * The log cut into segments at each step's first line. A step's minutes are from its first
 * line to the next step's, so the segments tile the job and sum to its wall clock.
 */
export function segments(lines) {
  const cuts = [];
  let gateSeen = false;
  for (let i = 0; i < lines.length; i++) {
    const { text } = lines[i];
    const run = STEP_START.exec(text);
    if (run) {
      cuts.push({ index: i, name: run[1].trim(), post: false });
    } else if (POST_START.test(text)) {
      cuts.push({ index: i, name: "Post job cleanup", post: true });
    }
  }
  const out = [];
  const first = lines[0]?.t ?? 0;
  const last = lines.at(-1)?.t ?? first;
  if (cuts.length === 0 || cuts[0].index > 0) {
    out.push({ name: "Set up job", start: first, end: cuts[0] ? lines[cuts[0].index].t : last });
  }
  for (let c = 0; c < cuts.length; c++) {
    const from = cuts[c].index;
    const to = c + 1 < cuts.length ? cuts[c + 1].index : lines.length;
    const body = lines.slice(from, to).map((l) => l.text);
    const start = lines[from].t;
    const end = c + 1 < cuts.length ? lines[to].t : last;
    const kind = classify(cuts[c], body, gateSeen);
    if (kind === "gate") gateSeen = true;
    out.push({ name: cuts[c].name, start, end, kind, body });
  }
  out[0].kind ??= "setup";
  return out;
}

function classify(cut, body, gateSeen) {
  const all = body.join("\n");
  if (cut.post || gateSeen) {
    if (
      /cache saved with the key|Cache saved successfully|Saving cache path|Cache saved/i.test(all)
    )
      return "cache-save";
    return "after-gate";
  }
  if (/\bjust ci-test-|\bscripts\/ci-job\.sh\b/.test(cut.name)) return "gate";
  if (/^just bootstrap\b/.test(cut.name)) return "bootstrap";
  if (/rust-cache@|actions\/cache\/restore@|actions\/cache@/.test(cut.name)) return "cache-restore";
  return "setup";
}

const TASK_START =
  /^(?:##\[group\])?(?:\S+ )?> nx run (\S+?:\S+)(?:\s+\[(local cache|remote cache|existing outputs match the cache, left as is)\])?\s*$/;
const STARTING = /^\s*Starting (\d+) tests? across/;
const SUMMARY = /^\s*Summary \[\s*([\d.]+)s\] (\d+) tests? run/;
const FINISHED = /^\s*Finished `([^`]+)` profile .* in (?:(\d+)m )?([\d.]+)s\s*$/;

/** Each test phase the gate's log shows: the task, when it began, its tests' window. */
export function testPhases(lines) {
  const phases = [];
  let current = null;
  let lastTestTask = null;
  for (const { t, text } of lines) {
    const task = TASK_START.exec(text);
    if (task) {
      current = { task: task[1], start: t, replayed: Boolean(task[2]) };
      if (/:test(-quick)?$/.test(task[1])) lastTestTask = current;
      continue;
    }
    const starting = STARTING.exec(text);
    if (starting && lastTestTask) {
      lastTestTask.testsFrom = t;
      lastTestTask.tests = Number(starting[1]);
      continue;
    }
    const summary = SUMMARY.exec(text);
    if (summary && lastTestTask?.testsFrom !== undefined && lastTestTask.testsTo === undefined) {
      lastTestTask.testsTo = t;
      lastTestTask.nextestS = Number(summary[1]);
      phases.push(lastTestTask);
    }
  }
  void current;
  return phases.filter((p) => !p.replayed);
}

/** Every cargo build the log reports finishing: its profile, elapsed seconds and end. */
export function cargoBuilds(lines) {
  const out = [];
  for (const { t, text } of lines) {
    const m = FINISHED.exec(text);
    if (m) out.push({ profile: m[1], s: Number(m[2] ?? 0) * 60 + Number(m[3]), end: t });
  }
  return out;
}

/** The profile records under `dir`, keeping only the run that holds the most of them. */
export function loadRecords(dir) {
  if (!existsSync(dir)) return [];
  const all = [];
  const walk = (d) => {
    for (const name of readdirSync(d)) {
      const path = join(d, name);
      if (statSync(path).isDirectory()) walk(path);
      else if (name.endsWith(".json") && !name.startsWith(".")) {
        try {
          all.push(JSON.parse(readFileSync(path, "utf8")));
        } catch {
          // A record its writer was still staging when the artifact was cut.
        }
      }
    }
  };
  walk(dir);
  const counts = new Map();
  for (const r of all) counts.set(r.run_id, (counts.get(r.run_id) ?? 0) + 1);
  const run = [...counts.entries()].sort((a, b) => b[1] - a[1])[0]?.[0];
  const byTest = new Map();
  for (const r of all.filter((r) => r.run_id === run)) {
    const key = `${r.binary}::${r.test}`;
    const seen = byTest.get(key);
    if (!seen || (r.attempt ?? 0) >= (seen.attempt ?? 0)) byTest.set(key, r);
  }
  return [...byTest.values()];
}

/**
 * One record's seconds by part: each non-git program's spawns by name, then git, build,
 * fixture, wait, fs and unattributed. A program's seconds are spawn-to-exit of what the
 * test started, so they hold whatever that program itself did, its own children included.
 */
export function recordParts(r) {
  const parts = new Map();
  const add = (k, s) => parts.set(k, (parts.get(k) ?? 0) + s);
  const programs = Object.entries(r.programs ?? {}).filter(([name]) => name !== "git");
  const programS = programs.reduce((sum, [, v]) => sum + v.s, 0);
  const spawnS = r.categories.spawn.s;
  // Program seconds and the spawn category are one clock read two ways; the category is
  // what the record's wall adds up from, so programs are scaled onto it.
  const scale = programS > 0 ? spawnS / programS : 0;
  for (const [name, v] of programs) add(`spawn: ${name}`, v.s * scale);
  if (programS === 0 && spawnS > 0) add("spawn: (unnamed)", spawnS);
  for (const c of CATEGORIES.filter((c) => c !== "spawn")) add(c, r.categories[c].s);
  add("in-test unattributed", r.unattributed_s);
  return parts;
}

function sumParts(records) {
  const total = new Map();
  for (const r of records) {
    for (const [k, s] of recordParts(r)) total.set(k, (total.get(k) ?? 0) + s);
  }
  return total;
}

function readExecutions(dir) {
  const path = join(dir, "executions.json");
  if (!existsSync(path)) return null;
  const doc = JSON.parse(readFileSync(path, "utf8"));
  return doc.tasks.map((t) => ({
    task: t.task,
    result: t.result,
    start: Date.parse(t.started_at) / 1000,
    end: Date.parse(t.finished_at) / 1000,
  }));
}

/** The chain of tasks that ends last, each one the latest to finish before the next began. */
export function criticalPath(tasks, slackS = 2) {
  if (!tasks.length) return [];
  const sorted = [...tasks].sort((a, b) => a.end - b.end);
  const path = [sorted.at(-1)];
  for (;;) {
    const head = path[0];
    const before = sorted.filter(
      (t) => t !== head && t.end <= head.start + slackS && !path.includes(t),
    );
    if (!before.length) break;
    path.unshift(before.at(-1));
  }
  return path;
}

/** One job folded: its wall clock divided into parts that tile it, and the detail behind them. */
export function foldJob(label, dir) {
  const logPath = join(dir, "job.log");
  if (!existsSync(logPath)) {
    throw new Refusal(`${label}: ${logPath} does not exist`, `save the job's log as ${logPath}`);
  }
  const lines = parseLog(readFileSync(logPath, "utf8"));
  if (lines.length === 0) {
    throw new Refusal(`${label}: ${logPath} holds no timestamped line`, "save the raw job log");
  }
  const segs = segments(lines);
  const wall = lines.at(-1).t - lines[0].t;
  const gate = segs.find((s) => s.kind === "gate");
  const records = loadRecords(join(dir, "test-profile"));
  const executions = readExecutions(dir);
  const division = new Map();
  // A tier is `<project>:test` on the instrumented Linux job and `<project>:test-quick` on
  // the others; one name lets the two jobs' rows stand side by side.
  const tier = (k) => k.replace(/:test(-quick)?\b/g, " tests");
  const add = (k, s) => division.set(tier(k), (division.get(tier(k)) ?? 0) + s);
  for (const s of segs.filter((s) => s.kind !== "gate")) add(`step: ${s.kind}`, s.end - s.start);

  const phases = [];
  let path = [];
  if (gate) {
    const gateLines = lines.filter((l) => l.t >= gate.start && l.t <= gate.end);
    const found = testPhases(gateLines);
    const gateTasks = executions?.filter((t) => t.end >= gate.start - 1 && t.start <= gate.end + 1);
    path = gateTasks?.length ? criticalPath(gateTasks) : [];
    // Every test phase gets the records that started inside its window.
    for (const p of found) {
      const inside = records.filter(
        (r) => r.started_at >= p.testsFrom - 1 && r.started_at <= p.testsTo,
      );
      phases.push({ ...p, records: inside });
    }
    let covered = 0;
    const onPath = path.length
      ? path
      : found.map((p) => ({ task: p.task, start: p.start, end: p.testsTo }));
    for (const task of onPath) {
      const phase = phases.find((p) => p.task === task.task);
      const span = Math.max(0, task.end - task.start);
      covered += span;
      if (!phase) {
        add(`gate: ${task.task}${task.result === "remote_hit" ? " (replayed)" : ""}`, span);
        continue;
      }
      add(`gate: ${task.task} build, list and archive`, Math.max(0, phase.testsFrom - task.start));
      const testWall = Math.max(0, phase.testsTo - phase.testsFrom);
      const parts = sumParts(phase.records);
      const testS = [...parts.values()].reduce((a, b) => a + b, 0);
      if (testS > 0) {
        for (const [k, s] of parts) add(`tests: ${k}`, (testWall * s) / testS);
      } else {
        add(`tests: ${task.task} (no profile records)`, testWall);
      }
      add(`gate: ${task.task} after its tests`, Math.max(0, task.end - phase.testsTo));
    }
    add(
      "gate: between critical-path tasks (Nx scheduling)",
      Math.max(0, gate.end - gate.start - covered),
    );
  }
  const attributed = [...division.values()].reduce((a, b) => a + b, 0);
  return {
    label,
    wall,
    division,
    unattributed: wall - attributed,
    segs,
    phases,
    path,
    executions,
    builds: cargoBuilds(gate ? lines.filter((l) => l.t >= gate.start && l.t <= gate.end) : lines),
    records,
  };
}

const min = (s) => (s / 60).toFixed(1);
const pct = (s, of) => (of > 0 ? `${((100 * s) / of).toFixed(1)}%` : "-");

/** The Markdown report over every folded job, against `baseline` when one is named. */
export function render(jobs, { baseline, top = 20 } = {}) {
  const out = [];
  const base = baseline ? jobs.find((j) => j.label === baseline) : null;
  if (baseline && !base) {
    throw new Refusal(`--baseline ${baseline} names no job`, "name one of the LABEL=DIR jobs", 2);
  }
  out.push("## Wall clock, divided\n");
  const keys = [...new Set(jobs.flatMap((j) => [...j.division.keys()]))].sort(
    (a, b) =>
      Math.max(...jobs.map((j) => j.division.get(b) ?? 0)) -
      Math.max(...jobs.map((j) => j.division.get(a) ?? 0)),
  );
  const header = ["part", ...jobs.flatMap((j) => [`${j.label} min`, `${j.label} share`])];
  const others = base ? jobs.filter((j) => j !== base) : [];
  for (const j of others) header.push(`${j.label} − ${base.label} min`);
  out.push(`| ${header.join(" | ")} |`);
  out.push(`| ${header.map((_, i) => (i ? "---:" : "---")).join(" | ")} |`);
  const row = (name, get) => {
    const cells = [name, ...jobs.flatMap((j) => [min(get(j)), pct(get(j), j.wall)])];
    for (const j of others) cells.push(min(get(j) - get(base)));
    out.push(`| ${cells.join(" | ")} |`);
  };
  // Parts under three seconds in every job are one row, so the table stays readable.
  const small = (k) => jobs.every((j) => (j.division.get(k) ?? 0) < 3);
  for (const k of keys.filter((k) => !small(k))) row(k, (j) => j.division.get(k) ?? 0);
  const rest = keys.filter(small);
  if (rest.length) {
    row(`other parts (${rest.length}, each under 3 s)`, (j) =>
      rest.reduce((a, k) => a + (j.division.get(k) ?? 0), 0),
    );
  }
  row("**unattributed**", (j) => j.unattributed);
  row("**job wall clock**", (j) => j.wall);

  for (const j of jobs) {
    out.push(`\n## ${j.label}\n`);
    out.push(
      `Job wall clock ${min(j.wall)} min; unattributed ${min(j.unattributed)} min (${pct(j.unattributed, j.wall)}).\n`,
    );
    out.push("### Steps\n\n| step | kind | min |\n| --- | --- | ---: |");
    for (const s of j.segs)
      out.push(
        `| ${s.name.slice(0, 80).replace(/\|/g, "\\|")} | ${s.kind} | ${min(s.end - s.start)} |`,
      );
    if (j.executions) {
      const t0 = j.executions.reduce((a, t) => Math.min(a, t.start), Number.POSITIVE_INFINITY);
      out.push(
        "\n### Nx tasks\n\n| task | result | starts at min | min | on critical path |\n| --- | --- | ---: | ---: | --- |",
      );
      for (const t of [...j.executions].sort((a, b) => a.start - b.start)) {
        out.push(
          `| ${t.task} | ${t.result} | ${min(t.start - t0)} | ${min(t.end - t.start)} | ${j.path.includes(t) ? "yes" : ""} |`,
        );
      }
    }
    if (j.phases.length) {
      out.push(
        "\n### Test phases\n\n| task | build, list, archive min | tests | test wall min | test-s | test-s / wall |\n| --- | ---: | ---: | ---: | ---: | ---: |",
      );
      for (const p of j.phases) {
        const testS = p.records.reduce((a, r) => a + r.wall_s, 0);
        const w = p.testsTo - p.testsFrom;
        out.push(
          `| ${p.task} | ${min(p.testsFrom - p.start)} | ${p.tests} | ${min(w)} | ${testS.toFixed(0)} | ${w > 0 ? (testS / w).toFixed(2) : "-"} |`,
        );
      }
    }
    if (j.builds.length) {
      out.push(
        "\n### Cargo builds in the gate\n\n| profile | elapsed min | ended at gate min |\n| --- | ---: | ---: |",
      );
      const g = j.segs.find((s) => s.kind === "gate");
      for (const b of j.builds)
        out.push(`| ${b.profile} | ${min(b.s)} | ${min(b.end - (g?.start ?? 0))} |`);
    }
    if (j.records.length) {
      const byBinary = new Map();
      for (const r of j.records) byBinary.set(r.binary, (byBinary.get(r.binary) ?? 0) + r.wall_s);
      out.push("\n### Test binaries\n\n| binary | tests | test-s |\n| --- | ---: | ---: |");
      for (const [b, s] of [...byBinary.entries()].sort((a, b) => b[1] - a[1])) {
        out.push(`| ${b} | ${j.records.filter((r) => r.binary === b).length} | ${s.toFixed(1)} |`);
      }
      const programs = new Map();
      for (const r of j.records) {
        for (const [name, v] of Object.entries(r.programs ?? {})) {
          const p = programs.get(name) ?? { count: 0, s: 0 };
          p.count += v.count;
          p.s += v.s;
          programs.set(name, p);
        }
      }
      out.push(
        "\n### Spawns by program (test-seconds, spawn to exit)\n\n| program | spawns | test-s | mean s per spawn |\n| --- | ---: | ---: | ---: |",
      );
      for (const [name, p] of [...programs.entries()]
        .sort((a, b) => b[1].s - a[1].s)
        .slice(0, 20)) {
        out.push(`| ${name} | ${p.count} | ${p.s.toFixed(1)} | ${(p.s / p.count).toFixed(3)} |`);
      }
      const waits = j.records.reduce(
        (a, r) => ({
          n: a.n + r.categories.wait.count,
          polls: a.polls + (r.categories.wait.polls ?? 0),
          s: a.s + r.categories.wait.s,
        }),
        { n: 0, polls: 0, s: 0 },
      );
      out.push(`\nWaits: ${waits.n} waits, ${waits.polls} polls, ${waits.s.toFixed(1)} test-s.`);
      out.push(`\n### The ${top} slowest tests\n`);
      out.push(`| test | wall s | ${[...CATEGORIES, "unattributed"].join(" | ")} | top program |`);
      out.push(
        `| --- | ---: | ${[...CATEGORIES, "unattributed"].map(() => "---:").join(" | ")} | --- |`,
      );
      for (const r of [...j.records].sort((a, b) => b.wall_s - a.wall_s).slice(0, top)) {
        const cats = CATEGORIES.map((c) => r.categories[c].s.toFixed(1));
        const topProgram = Object.entries(r.programs ?? {}).sort((a, b) => b[1].s - a[1].s)[0];
        const tp = topProgram
          ? `${topProgram[0]} ×${topProgram[1].count} ${topProgram[1].s.toFixed(1)}s`
          : "";
        out.push(
          `| ${r.binary}::${r.test} | ${r.wall_s.toFixed(1)} | ${cats.join(" | ")} | ${r.unattributed_s.toFixed(1)} | ${tp} |`,
        );
      }
    }
  }
  return `${out.join("\n")}\n`;
}

function resolveOnPath(name) {
  const exts = process.platform === "win32" ? (process.env.PATHEXT ?? ".EXE").split(";") : [""];
  const found = [];
  for (const dir of (process.env.PATH ?? "").split(delimiter)) {
    for (const ext of exts) {
      const candidate = join(dir, name + ext.toLowerCase());
      if (dir && existsSync(candidate) && !found.includes(candidate)) found.push(candidate);
    }
  }
  return found;
}

const DEFAULT_PROGRAMS = [
  ["git", ["--version"]],
  ["onevcs", ["--version"]],
  ["onetaskgraph", ["--version"]],
  ["node", ["-e", "0"]],
  ["python3", ["-c", "pass"]],
  ["python", ["-c", "pass"]],
  ["bash", ["-c", ":"]],
  ["sh", ["-c", ":"]],
  ["just", ["--version"]],
  ["cargo", ["--version"]],
];

/** Each program's spawn-to-exit milliseconds over `runs` starts of its cheapest invocation. */
export function measureSpawns({ runs = 30, programs = [] } = {}) {
  const exe = process.platform === "win32" ? ".exe" : "";
  const targets = [];
  // The crate's own binary and the doubles the e2e harness builds beside it.
  for (const name of ["onepipeline", "onevcs", "oneagentgraph", "oneharness", "fake-gh"]) {
    const local = join(process.cwd(), "target", "debug", `${name}${exe}`);
    if (existsSync(local)) targets.push({ name, path: local, args: ["--version"] });
  }
  for (const [name, args] of DEFAULT_PROGRAMS) {
    const [path] = resolveOnPath(name);
    const taken = targets.some((t) => t.name === name);
    if (path) targets.push({ name: taken ? `${name} (PATH)` : name, path, args });
  }
  for (const spec of programs) {
    const at = spec.indexOf("=");
    if (at < 1)
      throw new Refusal(`--program ${spec} is not NAME=PATH`, "pass --program NAME=PATH", 2);
    targets.push({ name: spec.slice(0, at), path: spec.slice(at + 1), args: ["--version"] });
  }
  const results = [];
  for (const target of targets) {
    if (!existsSync(target.path)) {
      results.push({ ...target, error: "not found" });
      continue;
    }
    const ms = [];
    let error;
    for (let i = 0; i < runs + 2; i++) {
      const t = process.hrtime.bigint();
      const r = spawnSync(target.path, target.args, { stdio: "ignore" });
      const elapsed = Number(process.hrtime.bigint() - t) / 1e6;
      // A start that exits non-zero was still one process start, which is what is timed.
      if (r.error) {
        error = r.error.message;
        break;
      }
      // The first two starts warm the file cache and are not counted.
      if (i >= 2) ms.push(elapsed);
    }
    if (error) {
      results.push({ ...target, error });
      continue;
    }
    ms.sort((a, b) => a - b);
    const q = (p) => ms[Math.min(ms.length - 1, Math.floor(p * ms.length))];
    results.push({ ...target, runs: ms.length, median_ms: q(0.5), p10_ms: q(0.1), p90_ms: q(0.9) });
  }
  const gitCopies = resolveOnPath("git");
  return { platform: process.platform, arch: process.arch, runs, results, git_on_path: gitCopies };
}

function renderSpawns(doc) {
  const out = [
    `Spawn cost on ${doc.platform}/${doc.arch}, ${doc.runs} starts each after two warm-ups.\n`,
  ];
  out.push("| program | invocation | median ms | p10 ms | p90 ms | path |");
  out.push("| --- | --- | ---: | ---: | ---: | --- |");
  for (const r of doc.results) {
    const inv = r.args.join(" ");
    if (r.error) out.push(`| ${r.name} | ${inv} | ${r.error} | | | ${r.path} |`);
    else
      out.push(
        `| ${r.name} | ${inv} | ${r.median_ms.toFixed(1)} | ${r.p10_ms.toFixed(1)} | ${r.p90_ms.toFixed(1)} | ${r.path} |`,
      );
  }
  out.push(`\ngit on PATH, in order: ${doc.git_on_path.join(", ") || "none"}`);
  return `${out.join("\n")}\n`;
}

const OUTCOME =
  /^\s*(PASS|FAIL|SIGSEGV|SIGABRT|TIMEOUT|LEAK-FAIL|ABORT)\s+\[\s*[\d.]+s\]\s+(?:\(\s*\d+\/\d+\)\s+)?(\S+)\s+(\S+)/;

/** Each test a job's log reports finishing, by `binary test`, with its last status. */
export function outcomes(raw) {
  const seen = new Map();
  for (const { text } of parseLog(raw)) {
    const m = OUTCOME.exec(text);
    if (m) seen.set(`${m[2]} ${m[3]}`, m[1]);
  }
  return seen;
}

/** Whether every test the first log passed also ran and passed in each other log. */
export function compareOutcomes(logs) {
  const [first, ...rest] = logs.map(({ label, path }) => {
    if (!existsSync(path))
      throw new Refusal(`${label}: ${path} does not exist`, "save the job's log there");
    return { label, seen: outcomes(readFileSync(path, "utf8")) };
  });
  const count = (seen, status) => [...seen.values()].filter((s) => s === status).length;
  const out = [
    "| job | tests reported | passed | not passed | baseline passes missing or not passed |",
  ];
  out.push("| --- | ---: | ---: | ---: | --- |");
  const basePassed = [...first.seen].filter(([, s]) => s === "PASS").map(([k]) => k);
  for (const job of [first, ...rest]) {
    const lost = job === first ? [] : basePassed.filter((k) => job.seen.get(k) !== "PASS");
    const cell =
      job === first ? "(baseline)" : lost.length ? `${lost.length}: ${lost.join(", ")}` : "none";
    out.push(
      `| ${job.label} | ${job.seen.size} | ${count(job.seen, "PASS")} | ${job.seen.size - count(job.seen, "PASS")} | ${cell} |`,
    );
  }
  const failed = [...first.seen].filter(([, s]) => s !== "PASS").map(([k, s]) => `${k} (${s})`);
  out.push(`\nNot passed in ${first.label}: ${failed.length ? failed.join(", ") : "none"}.`);
  return `${out.join("\n")}\n`;
}

function usage(message) {
  return new Refusal(
    message,
    "run 'node scripts/windows-job-profile.mjs fold LABEL=DIR ...' or '... spawn [--runs N]'",
    2,
  );
}

export function main(argv) {
  const [verb, ...rest] = argv;
  if (verb === "fold") {
    let baseline;
    let top = 20;
    const jobs = [];
    for (let i = 0; i < rest.length; i++) {
      const arg = rest[i];
      if (arg === "--baseline") baseline = rest[++i];
      else if (arg === "--top") {
        top = Number(rest[++i]);
        if (!Number.isSafeInteger(top) || top < 1)
          throw usage("--top takes a positive whole number");
      } else if (/^[^=]+=.+/.test(arg)) {
        const at = arg.indexOf("=");
        jobs.push(foldJob(arg.slice(0, at), arg.slice(at + 1)));
      } else throw usage(`unknown argument '${arg}'`);
    }
    if (!jobs.length) throw usage("name at least one LABEL=DIR job");
    return render(jobs, { baseline, top });
  }
  if (verb === "spawn") {
    let runs = 30;
    let json = false;
    const programs = [];
    for (let i = 0; i < rest.length; i++) {
      const arg = rest[i];
      if (arg === "--runs") runs = Number(rest[++i]);
      else if (arg === "--json") json = true;
      else if (arg === "--program") programs.push(rest[++i] ?? "");
      else throw usage(`unknown argument '${arg}'`);
    }
    if (!Number.isSafeInteger(runs) || runs < 1)
      throw usage("--runs takes a positive whole number");
    const doc = measureSpawns({ runs, programs });
    return json ? `${JSON.stringify(doc, null, 2)}\n` : renderSpawns(doc);
  }
  if (verb === "outcomes") {
    const logs = rest.map((arg) => {
      const at = arg.indexOf("=");
      if (at < 1) throw usage(`'${arg}' is not LABEL=LOG`);
      return { label: arg.slice(0, at), path: arg.slice(at + 1) };
    });
    if (logs.length < 1) throw usage("name at least one LABEL=LOG, the baseline first");
    return compareOutcomes(logs);
  }
  throw usage(verb ? `unknown verb '${verb}'` : "name a verb: fold, spawn or outcomes");
}

if (import.meta.url === pathToFileURL(process.argv[1] ?? "").href) {
  try {
    process.stdout.write(main(process.argv.slice(2)));
  } catch (error) {
    if (!(error instanceof Refusal)) throw error;
    process.stderr.write(`windows-job-profile: ${error.message}\nnext: ${error.next}\n`);
    process.exitCode = error.code;
  }
}
