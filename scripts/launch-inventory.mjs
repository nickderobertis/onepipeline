#!/usr/bin/env node
// Measurement only (spike-launch-inventory, never lands): fold the launch trace and the
// test-profile records one hosted e2e job left behind into the inventory's tables.
//
//   node scripts/launch-inventory.mjs fold [--top N] LABEL=DIR [LABEL=DIR ...]
//
// Each DIR holds one job's `test-profile-<job>` artifact unzipped under `test-profile/`
// (`e2e/*.json`, the test-profile records; `launch/*.jsonl`, the launch trace the e2e
// harness switches on through `ONEPIPELINE_LAUNCH_TRACE`; `launch-host.json`, the
// runner's processor and minimal-start medians), optionally its log as `job.log`, and
// optionally the operating system's own record of process starts under `os/`
// (`strace -f` output as `*.strace`, or ETW process starts as `*.jsonl`, one
// `{pid, ppid, image, cmd}` per line).
//
// It prints, per job and in Markdown: the tier (tests, test-seconds, nextest summary);
// every program started, split into the engine's and its libraries' starts (the trace)
// and the tests' own (the test-profile records), with count, median and p90
// start-to-exit, exclusive seconds (a nested start's time counted once, in the innermost
// program) and shares; the same by call site and engine verb; each program's cheapest
// invocation; git invocations repeating an earlier one in the same CLI process; every
// wait on a child by site; the 20 tests and sites starting the most; the journeys that
// re-launch the CLI for read-only verbs; the trace's own cost; and, with `os/`, every
// start the OS saw that the trace and the records do not hold. Then, across jobs, the
// per-test and per-site start counts side by side.
//
// Exit 0 once printed; 1 with the reason and a `next:` line when an input cannot be
// read; 2 for a usage error.
import { execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
import { existsSync, readdirSync, readFileSync } from "node:fs";
import { cpus } from "node:os";
import { basename, join } from "node:path";
import { pathToFileURL } from "node:url";

export class Refusal extends Error {
  constructor(message, next, code = 1) {
    super(message);
    this.next = next;
    this.code = code;
  }
}

// biome-ignore lint/suspicious/noControlCharactersInRegex: a job log carries ANSI colour codes.
const ANSI = /\u001b\[[0-9;]*[A-Za-z]/g;

/** git subcommands that write nothing a later read of the same argv would see. */
export const GIT_READS = new Set([
  "rev-parse",
  "status",
  "log",
  "show",
  "cat-file",
  "for-each-ref",
  "ls-files",
  "ls-tree",
  "diff",
  "diff-index",
  "diff-tree",
  "diff-files",
  "merge-base",
  "rev-list",
  "show-ref",
  "describe",
  "name-rev",
  "check-ignore",
  "check-attr",
  "check-ref-format",
  "grep",
  "blame",
  "var",
  "ls-remote",
  "count-objects",
  "version",
  "--version",
  "help",
  "shortlog",
  "cherry",
  "range-diff",
  "whatchanged",
  "verify-commit",
  "verify-tag",
  "fsck",
]);

/**
 * Whether one git argv only reads. `config`, `branch`, `worktree`, `remote`, `symbolic-ref`,
 * `tag`, `stash` and `hash-object` read in some forms and write in others; any flag or
 * argument shape this does not recognise as a read is a write, so a repeat is never counted
 * across something that might have changed what it reads.
 */
export function gitReads(sub, argv) {
  if (!sub) return false;
  if (GIT_READS.has(sub)) return true;
  const rest = argv.slice(argv.indexOf(sub) + 1);
  const has = (...flags) => rest.some((a) => flags.includes(a));
  const operands = rest.filter((a) => !a.startsWith("-"));
  switch (sub) {
    case "config":
      return (
        has("--get", "--get-all", "--get-regexp", "--get-urlmatch", "--list", "-l") &&
        !has(
          "--add",
          "--unset",
          "--unset-all",
          "--replace-all",
          "--rename-section",
          "--remove-section",
          "--edit",
          "-e",
        )
      );
    case "branch":
      return (
        !has(
          "-d",
          "-D",
          "--delete",
          "-m",
          "-M",
          "--move",
          "-c",
          "-C",
          "--copy",
          "-f",
          "--force",
          "-u",
          "--set-upstream-to",
          "--unset-upstream",
          "--edit-description",
          "-t",
          "--track",
        ) &&
        (operands.length === 0 ||
          has("--list", "--contains", "--merged", "--no-merged", "--points-at"))
      );
    case "worktree":
      return rest[0] === "list";
    case "remote":
      return (
        rest.length === 0 ||
        ["get-url", "show"].includes(rest[0]) ||
        (rest.length === 1 && rest[0] === "-v")
      );
    case "symbolic-ref":
      return !has("-d", "--delete", "-m") && operands.length <= 1;
    case "tag":
      return (
        !has("-d", "--delete", "-a", "-s", "-f", "-m", "-F") &&
        (operands.length === 0 || has("--list", "-l", "--contains", "--points-at"))
      );
    case "stash":
      return ["list", "show"].includes(rest[0]);
    case "hash-object":
      return !has("-w");
    default:
      return false;
  }
}

/**
 * The engine's verbs that read and change nothing, from its `--help`: a journey re-launching
 * the CLI for one is re-reading. (`unpublished`, `supersessions` and `review` read unless
 * given their recording flags, which a re-launch for a read does not pass.)
 */
export const READ_VERBS = new Set([
  "plan",
  "template",
  "boundary",
  "channel",
  "next",
  "review",
  "runs",
  "status",
  "host",
  "monitor",
  "watch",
  "unwatched",
  "stop-guard",
  "unpublished",
  "results",
  "supersessions",
  "goals",
  "transcript",
  "agents",
  "telemetry",
  "help",
  "--version",
  "--help",
]);

const median = (xs) => quantile(xs, 0.5);
function quantile(xs, p) {
  if (!xs.length) return null;
  const s = [...xs].sort((a, b) => a - b);
  return s[Math.min(s.length - 1, Math.floor(p * s.length))];
}
const ms = (ns) => (ns == null ? "-" : (ns / 1e6).toFixed(1));
const sec = (ns) => (ns / 1e9).toFixed(1);
const pct = (x, of) => (of > 0 ? `${((100 * x) / of).toFixed(2)}%` : "-");

// llmlint: ignore[contracts_have_one_source_or_a_drift_gate] the producer is vendor/onevcs/src/launch_trace.rs on this same branch, and npm/test/golden/launch-inventory is that producer's own output recorded at this commit; a cross-language drift gate would be code for a reader that never lands: this branch is preserved as the measurement record and nothing on it merges.
/** The trace's record kinds, each with the fields a fold reads from it. */
const RECORD_FIELDS = {
  proc: ["pid", "t"],
  env: ["pid", "id"],
  start: ["pid", "t", "program", "site"],
  exit: ["pid"],
  end: ["pid", "t"],
  wait: ["pid", "t", "site"],
  sleeps: ["pid", "site", "count"],
};

/** Whether `r` is a trace record of a kind this fold reads, carrying its fields. */
export function validRecord(r) {
  const fields = r && r.v === 1 ? RECORD_FIELDS[r.kind] : null;
  return Boolean(fields) && fields.every((f) => r[f] != null) && Number.isFinite(r.pid);
}

/**
 * A JSONL file's valid records, counting into `skipped` each line that is not JSON (one a
 * process the runner ended was writing when it went) and each record of no known shape.
 */
function readJsonl(path, skipped) {
  const out = [];
  for (const line of readFileSync(path, "utf8").split("\n")) {
    if (!line.trim()) continue;
    let r;
    try {
      r = JSON.parse(line);
    } catch {
      skipped.malformed++;
      continue;
    }
    if (validRecord(r)) out.push(r);
    else skipped.invalid++;
  }
  return out;
}

/** A JSON document, or a refusal naming the file. */
function readJson(path) {
  try {
    return JSON.parse(readFileSync(path, "utf8"));
  } catch (error) {
    throw new Refusal(
      `${path} is not a JSON document: ${error.message}`,
      "re-download the job's test-profile artifact",
    );
  }
}

/** Every record a job's artifact holds. */
export function readJob(label, dir) {
  if (!existsSync(dir))
    throw new Refusal(
      `${label}: ${dir} does not exist`,
      "unzip the job's test-profile artifact there",
    );
  const root = existsSync(join(dir, "test-profile")) ? join(dir, "test-profile") : dir;
  const skipped = { malformed: 0, invalid: 0, records: 0, waits: 0 };
  const tests = [];
  const e2e = join(root, "e2e");
  if (existsSync(e2e)) {
    for (const f of readdirSync(e2e)) {
      if (!f.endsWith(".json")) continue;
      const r = readJson(join(e2e, f));
      // The test-profile record's own shape, schema 2 (crates/testfakes/src/profile.rs).
      if (
        r.schema === 2 &&
        typeof r.test === "string" &&
        Number.isFinite(r.wall_s) &&
        r.categories &&
        r.programs
      )
        tests.push(r);
      else skipped.records++;
    }
  }
  const trace = [];
  const harnessWaits = [];
  const launch = join(root, "launch");
  if (existsSync(launch)) {
    for (const f of readdirSync(launch)) {
      if (f.endsWith(".waits.json")) {
        const doc = readJson(join(launch, f));
        const sites = Array.isArray(doc.sites) ? doc.sites : [];
        const valid = sites.filter(
          (w) =>
            ["span", "poll", "sleep"].includes(w.kind) &&
            typeof w.site === "string" &&
            Number.isFinite(w.count) &&
            Number.isFinite(w.s),
        );
        skipped.waits += sites.length - valid.length + (Array.isArray(doc.sites) ? 0 : 1);
        for (const site of valid) harnessWaits.push({ ...site, test: doc.test });
        continue;
      }
      if (!f.endsWith(".jsonl")) continue;
      const file = basename(f, ".jsonl");
      for (const r of readJsonl(join(launch, f), skipped)) trace.push({ ...r, file });
    }
  }
  if (!tests.length && !trace.length)
    throw new Refusal(
      `${label}: ${dir} holds no e2e test-profile record and no launch trace`,
      "point LABEL=DIR at an unzipped test-profile artifact",
    );
  const hostFile = join(root, "launch-host.json");
  const host = existsSync(hostFile) ? readJson(hostFile) : null;
  if (host && !Array.isArray(host.spawn?.results))
    throw new Refusal(
      `${hostFile} holds no spawn results`,
      "regenerate it with 'node scripts/launch-inventory.mjs host'",
    );
  const logFile = join(dir, "job.log");
  const log = existsSync(logFile) ? readFileSync(logFile, "utf8") : null;
  const os = readOs(join(dir, "os"), skipped);
  return { label, tests, trace, harnessWaits, host, log, os, skipped };
}

/**
 * Each nextest run a job log holds, as `{ start, end, seconds, run, passed, line }`, and
 * which of them is the e2e tier: the largest, since the e2e binary's ~2,150 journeys are
 * the most tests any one nextest invocation in a test job runs.
 */
export function nextestRuns(log) {
  const lines = log.replace(ANSI, "").split(/\r?\n/);
  const runs = [];
  let start = null;
  for (const line of lines) {
    const t = Date.parse(/^(\S+Z)\s/.exec(line)?.[1] ?? "") / 1000;
    if (/Nextest run ID/.test(line)) start = t;
    const m = /Summary \[\s*([\d.]+)s\]\s+(\d+) tests? run: (\d+) passed/.exec(line);
    if (m)
      runs.push({
        start,
        end: t,
        seconds: Number(m[1]),
        run: Number(m[2]),
        passed: Number(m[3]),
        line: line.replace(/^\S+Z\s*/, "").trim(),
      });
  }
  return runs;
}

/** The e2e tier's nextest summary in a job log, or null. */
export function e2eSummary(log) {
  if (!log) return null;
  return nextestRuns(log).reduce((best, r) => (!best || r.run > best.run ? r : best), null);
}

/** The operating system's own process starts: `strace -f` output or ETW JSONL. */
export function readOs(dir, skipped = { malformed: 0, invalid: 0 }) {
  if (!existsSync(dir)) return null;
  const starts = [];
  for (const f of readdirSync(dir)) {
    const path = join(dir, f);
    if (f.endsWith(".strace")) starts.push(...parseStrace(readFileSync(path, "utf8"), f));
    else if (f.endsWith(".jsonl"))
      for (const line of readFileSync(path, "utf8").split("\n")) {
        if (!line.trim()) continue;
        let r;
        try {
          r = JSON.parse(line);
        } catch {
          skipped.malformed++;
          continue;
        }
        // An ETW process start: its pid, its parent's, and its image.
        if (!Number.isFinite(r.pid) || !Number.isFinite(r.ppid) || typeof r.image !== "string") {
          skipped.invalid++;
          continue;
        }
        starts.push({
          pid: r.pid,
          ppid: r.ppid,
          exe: stem(r.image),
          argv: splitCmd(r.cmd ?? ""),
          file: f,
        });
      }
  }
  return starts;
}

const stem = (path) =>
  basename(String(path).replace(/\\/g, "/"))
    .replace(/\.(exe|cmd|bat)$/i, "")
    .toLowerCase();
function splitCmd(cmd) {
  const out = [];
  for (const m of cmd.matchAll(/"([^"]*)"|(\S+)/g)) out.push(m[1] ?? m[2]);
  return out;
}

/**
 * Process starts out of `strace -f -e trace=execve,clone,clone3,fork,vfork` output: each
 * successful execve with the process (thread group) whose thread cloned it.
 */
export function parseStrace(text, file = "") {
  const parentOf = new Map();
  const groupOf = new Map();
  const execs = [];
  const unfinished = new Map();
  for (const line of text.split("\n")) {
    const m = /^(\d+)\s+(?:[\d:.]+\s+)?(.*)$/.exec(line);
    if (!m) continue;
    const tid = Number(m[1]);
    let rest = m[2];
    // A call another thread interrupted is printed in two halves: rejoin them.
    const cut = /^(.*?)\s*<unfinished \.\.\.>$/.exec(rest);
    if (cut) {
      unfinished.set(tid, cut[1]);
      continue;
    }
    const resumed = /^<\.\.\. (\w+) resumed>\s?(.*)$/.exec(rest);
    if (resumed) {
      const head = unfinished.get(tid);
      unfinished.delete(tid);
      if (head == null) continue;
      rest = `${head}${resumed[2]}`;
    }
    const clone = /^(clone3?|fork|vfork)\((.*)\)\s*=\s*(\d+)/.exec(rest);
    if (clone) {
      const child = Number(clone[3]);
      if (/CLONE_THREAD/.test(clone[2])) groupOf.set(child, groupOf.get(tid) ?? tid);
      else parentOf.set(child, groupOf.get(tid) ?? tid);
      continue;
    }
    const exec = /^execve\("([^"]*)",\s*\[(.*?)\](?:\.\.\.)?,.*\)\s*=\s*0/.exec(rest);
    if (exec) {
      const argv = [...exec[2].matchAll(/"((?:[^"\\]|\\.)*)"/g)].map((a) => a[1]);
      execs.push({ pid: groupOf.get(tid) ?? tid, exe: stem(exec[1]), argv, file });
    }
  }
  // An exec replaces the image a fork made: the first exec of a pid is the start, any
  // later one (a shell exec-ing its command) is the same process becoming another.
  const seen = new Set();
  const out = [];
  for (const e of execs) {
    out.push({ ...e, ppid: parentOf.get(e.pid) ?? null, reexec: seen.has(e.pid) });
    seen.add(e.pid);
  }
  return out;
}

/** The trace's records, joined: starts with durations, processes, waits. */
export function joinTrace(trace) {
  const procs = new Map();
  const ends = new Map();
  const exits = new Map();
  const starts = [];
  const waits = [];
  const sleeps = [];
  const envs = new Map();
  const bySeq = new Map();
  const key = (file, pid) => `${file}\u0000${pid}`;
  for (const r of trace) {
    if (r.kind === "env") envs.set(`${key(r.file, r.pid)}\u0000${r.id}`, r.env);
  }
  for (const r of trace) {
    if (r.kind === "proc") procs.set(key(r.file, r.pid), r);
    else if (r.kind === "end") ends.set(key(r.file, r.pid), r.t);
    else if (r.kind === "exit" && r.seq != null)
      bySeq.set(`${key(r.file, r.pid)}\u0000${r.seq}`, r);
    else if (r.kind === "exit") exits.set(`${key(r.file, r.pid)}\u0000${r.child}`, r.t);
    else if (r.kind === "start") {
      if (typeof r.env === "number")
        r.env = envs.get(`${key(r.file, r.pid)}\u0000${r.env}`) ?? { unresolved: r.env };
      starts.push(r);
    } else if (r.kind === "wait") waits.push(r);
    else if (r.kind === "sleeps") sleeps.push(r);
  }
  // A start written at its spawn has its exit, and any wait folded into it, in an
  // `exit` record of the same seq.
  for (const s of starts) {
    const e = s.seq != null ? bySeq.get(`${key(s.file, s.pid)}\u0000${s.seq}`) : null;
    if (!e) continue;
    for (const f of ["exit_ns", "status", "wait_site", "wait_ns", "polls", "interval_ns"])
      if (e[f] != null) s[f] = e[f];
  }
  // A record names its process; the process names its verb and test.
  for (const r of [...starts, ...waits, ...sleeps]) {
    const p = procs.get(key(r.file, r.pid));
    r.verb ??= isTestBinary(p) ? "(test process)" : (p?.verb ?? "");
    r.test ??= p?.test ?? r.file;
  }
  // A start whose wait on it was folded into its record is that wait too.
  for (const s of starts) {
    if (s.wait_site) waits.push({ ...s, kind: "wait", site: s.wait_site });
  }
  for (const s of starts) {
    s.dur = s.exit_ns ?? null;
    if (s.dur == null && s.child != null) {
      const at =
        exits.get(`${key(s.file, s.pid)}\u0000${s.child}`) ?? ends.get(key(s.file, s.child));
      if (at != null && at >= s.t) s.dur = at - s.t;
    }
  }
  // The traced process each start began, where one exists: by pid when the site knew it,
  // otherwise the process of that program whose first record falls inside the start.
  const byFile = new Map();
  for (const p of procs.values()) {
    if (!byFile.has(p.file)) byFile.set(p.file, []);
    byFile.get(p.file).push(p);
  }
  const claimed = new Set();
  for (const s of [...starts].sort((a, b) => a.t - b.t)) {
    const candidates = byFile.get(s.file) ?? [];
    let child = null;
    if (s.child != null) child = candidates.find((p) => p.pid === s.child && p.t >= s.t) ?? null;
    else if (["onepipeline", "onevcs"].includes(s.program))
      child =
        candidates.find(
          (p) =>
            !claimed.has(p) &&
            p.exe === s.program &&
            p.t >= s.t &&
            (s.dur == null || p.t <= s.t + s.dur),
        ) ?? null;
    if (child) {
      claimed.add(child);
      s.childProc = child;
      child.parentStart = s;
      if (s.dur == null) {
        const end = ends.get(key(child.file, child.pid));
        if (end != null) s.dur = end - s.t;
      }
    }
  }
  const startsOf = new Map();
  for (const s of starts) {
    const k = key(s.file, s.pid);
    if (!startsOf.has(k)) startsOf.set(k, []);
    startsOf.get(k).push(s);
  }
  for (const s of starts) {
    const inner = s.childProc ? (startsOf.get(key(s.file, s.childProc.pid)) ?? []) : [];
    s.nested = union(
      inner.filter((c) => c.dur != null).map((c) => [c.t, c.t + c.dur]),
      s.t,
      s.t + (s.dur ?? 0),
    );
    s.self = s.dur == null ? null : Math.max(0, s.dur - s.nested);
  }
  return { procs: [...procs.values()], ends, starts, waits, sleeps, startsOf, key };
}

/** Nanoseconds of [from, to] covered by any interval. */
function union(intervals, from, to) {
  const xs = intervals
    .map(([a, b]) => [Math.max(a, from), Math.min(b, to)])
    .filter(([a, b]) => b > a)
    .sort((p, q) => p[0] - q[0]);
  let total = 0;
  let cur = null;
  for (const [a, b] of xs) {
    if (!cur || a > cur[1]) {
      if (cur) total += cur[1] - cur[0];
      cur = [a, b];
    } else cur[1] = Math.max(cur[1], b);
  }
  if (cur) total += cur[1] - cur[0];
  return total;
}

const programOf = (s) => (s.program === "git" && s.sub ? `git ${s.sub}` : s.program);
const siteOf = (s) => `${s.function} (${s.site.replace(/^.*?(src\/|vendor\/)/, "$1")})`;
const isTestBinary = (p) => Boolean(p) && /^[a-z0-9_]+-[0-9a-f]{16}$/.test(p.exe);

/** A job folded: every figure the tables print, as data. */
export function foldJob(job) {
  const t = joinTrace(job.trace);
  const testSeconds = job.tests.reduce((a, r) => a + (r.wall_s ?? 0), 0);
  const summary = e2eSummary(job.log);
  const wallS = summary?.seconds ?? null;
  const toWallMin = (ns) => (wallS && testSeconds ? ((ns / 1e9 / testSeconds) * wallS) / 60 : null);
  const procOf = (s) => t.procs.find((p) => p.file === s.file && p.pid === s.pid);
  const procIndex = new Map(t.procs.map((p) => [t.key(p.file, p.pid), p]));
  const startedBy = (s) => procIndex.get(t.key(s.file, s.pid));

  // Engine and library starts, from the trace.
  const engine = new Map();
  const bySite = new Map();
  for (const s of t.starts) {
    const p = startedBy(s);
    const origin = isTestBinary(p) ? "library, in the test process" : `${p?.exe ?? "?"} (traced)`;
    const prog = programOf(s);
    const add = (map, k) => {
      if (!map.has(k))
        map.set(k, { key: k, program: prog, origin, count: 0, durs: [], self: 0, untimed: 0 });
      const e = map.get(k);
      e.count++;
      if (s.dur == null) e.untimed++;
      else e.durs.push(s.dur);
      e.self += s.self ?? 0;
    };
    add(engine, `${origin}\u0000${prog}`);
    add(bySite, `${prog}\u0000${siteOf(s)}\u0000${s.verb || "-"}`);
  }

  // The tests' own starts, from the records, less what traced processes they began did
  // inside them: a top-level process's starts are subtracted from its program's time.
  const tests = new Map();
  const nestedInTest = new Map();
  for (const p of t.procs) {
    if (isTestBinary(p) || p.parentStart) continue;
    const inner = t.startsOf.get(t.key(p.file, p.pid)) ?? [];
    const end =
      t.ends.get(t.key(p.file, p.pid)) ?? Math.max(p.t, ...inner.map((c) => c.t + (c.dur ?? 0)));
    const k = `${p.file}\u0000${p.exe}`;
    nestedInTest.set(
      k,
      (nestedInTest.get(k) ?? 0) +
        union(
          inner.filter((c) => c.dur != null).map((c) => [c.t, c.t + c.dur]),
          p.t,
          end,
        ),
    );
  }
  for (const r of job.tests) {
    const file = r.test.replaceAll("::", "__");
    for (const [prog, v] of Object.entries(r.programs ?? {})) {
      if (!tests.has(prog)) tests.set(prog, { program: prog, count: 0, held: 0, self: 0 });
      const e = tests.get(prog);
      e.count += v.count;
      const held = v.s * 1e9;
      e.held += held;
      e.self += Math.max(0, held - (nestedInTest.get(`${file}\u0000${prog}`) ?? 0));
    }
  }

  // Each top-level engine run by verb, from its own process records.
  const verbs = new Map();
  for (const p of t.procs) {
    if (isTestBinary(p) || p.parentStart) continue;
    const k = `${p.exe} ${p.verb || "-"}`;
    if (!verbs.has(k)) verbs.set(k, { key: k, count: 0, lives: [] });
    const e = verbs.get(k);
    e.count++;
    const end = t.ends.get(t.key(p.file, p.pid));
    if (end != null) e.lives.push(end - p.t);
  }

  // Repeats: same argv, cwd and GIT_* in one process, no git write between.
  const repeats = new Map();
  let repeatCount = 0;
  let repeatNs = 0;
  for (const list of t.startsOf.values()) {
    const seen = new Set();
    for (const s of [...list].sort((a, b) => a.t - b.t)) {
      if (s.program !== "git") continue;
      if (!gitReads(s.sub, s.argv)) {
        seen.clear();
        continue;
      }
      const k = JSON.stringify([s.argv, s.cwd, s.env]);
      if (seen.has(k)) {
        repeatCount++;
        repeatNs += s.dur ?? 0;
        const rk = `${s.verb || "-"}\u0000${programOf(s)}\u0000${siteOf(s)}`;
        if (!repeats.has(rk))
          repeats.set(rk, {
            verb: s.verb || "-",
            program: programOf(s),
            site: siteOf(s),
            count: 0,
            ns: 0,
          });
        const e = repeats.get(rk);
        e.count++;
        e.ns += s.dur ?? 0;
      } else seen.add(k);
    }
  }

  // Waits on a child, by site.
  const waits = new Map();
  for (const w of t.waits) {
    const k = `${w.function} (${w.site.replace(/^.*?(src\/|vendor\/)/, "$1")})`;
    if (!waits.has(k))
      waits.set(k, { site: k, kind: "wait", count: 0, polls: 0, ns: 0, interval: w.interval_ns });
    const e = waits.get(k);
    e.count++;
    e.polls += w.polls ?? 0;
    e.ns += w.wait_ns ?? 0;
  }
  for (const w of t.sleeps) {
    const k = `${w.function} (${w.site.replace(/^.*?(src\/|vendor\/)/, "$1")})`;
    if (!waits.has(k))
      waits.set(k, { site: k, kind: "sleep", count: 0, polls: 0, ns: 0, interval: w.interval_ns });
    const e = waits.get(k);
    e.count += w.count;
    e.ns += w.slept_ns ?? 0;
  }
  // The harness's own waits, on the test process's threads, by site.
  const harness = new Map();
  for (const w of job.harnessWaits ?? []) {
    const k = `${w.kind}\u0000${w.site}`;
    if (!harness.has(k))
      harness.set(k, { kind: w.kind, site: w.site, count: 0, s: 0, interval: w.interval_s });
    const e = harness.get(k);
    e.count += w.count;
    e.s += w.s;
  }
  const profileWait = job.tests.reduce((a, r) => a + (r.categories?.wait?.s ?? 0), 0);
  const profileWaitCount = job.tests.reduce(
    (a, r) => a + (r.categories?.wait?.count ?? 0) + (r.categories?.wait?.polls ?? 0),
    0,
  );

  // Starts per test and per site.
  const perTest = new Map();
  for (const s of t.starts) perTest.set(s.test || s.file, (perTest.get(s.test || s.file) ?? 0) + 1);
  for (const r of job.tests)
    perTest.set(
      r.test,
      (perTest.get(r.test) ?? 0) + Object.values(r.programs ?? {}).reduce((a, v) => a + v.count, 0),
    );
  const perSite = new Map();
  for (const s of t.starts) perSite.set(siteOf(s), (perSite.get(siteOf(s)) ?? 0) + 1);

  // Journeys re-launching the CLI for read-only verbs.
  const relaunch = new Map();
  for (const p of t.procs) {
    if (p.exe !== "onepipeline" || p.parentStart || !READ_VERBS.has(p.verb)) continue;
    if (!relaunch.has(p.test)) relaunch.set(p.test, new Map());
    const m = relaunch.get(p.test);
    m.set(p.verb, (m.get(p.verb) ?? 0) + 1);
  }

  // The trace's own cost.
  let traceNs = 0;
  let records = 0;
  const lastWrite = new Map();
  for (const r of job.trace) {
    records++;
    traceNs += r.trace_ns ?? 0;
    const k = t.key(r.file, r.pid);
    lastWrite.set(k, Math.max(lastWrite.get(k) ?? 0, r.write_ns_before ?? 0));
  }
  const writeNs = [...lastWrite.values()].reduce((a, b) => a + b, 0);

  const names = job.tests.map((r) => r.test).sort();
  return {
    label: job.label,
    skipped: job.skipped ?? { malformed: 0, invalid: 0, records: 0, waits: 0 },
    os: job.tests[0]?.os ?? "?",
    host: job.host,
    summary,
    testCount: job.tests.length,
    unfinished: job.tests.filter((r) => r.ended === "unfinished").map((r) => r.test),
    testsDigest: createHash("sha256").update(names.join("\n")).digest("hex").slice(0, 16),
    names,
    testSeconds,
    wallS,
    toWallMin,
    engine: [...engine.values()],
    bySite: [...bySite.values()],
    tests: [...tests.values()],
    verbs: [...verbs.values()],
    repeats: [...repeats.values()],
    repeatCount,
    repeatNs,
    waits: [...waits.values()],
    harness: [...harness.values()],
    profileWait,
    profileWaitCount,
    perTest,
    perSite,
    relaunch,
    starts: t.starts,
    procs: t.procs,
    trace: { records, traceNs, writeNs, procs: lastWrite.size },
    coverage: job.os ? coverage(job.os, t, job.tests) : null,
    procOf,
  };
}

/** What the OS saw start that the trace and the records do not hold, by starter. */
export function coverage(os, t, records) {
  const procs = new Map(t.procs.map((p) => [p.pid, p]));
  const osPid = new Map(os.map((e) => [e.pid, e]));
  const expected = new Map();
  for (const s of t.starts) {
    const k = `${s.pid}\u0000${s.program.toLowerCase()}`;
    expected.set(k, (expected.get(k) ?? 0) + 1);
  }
  const rows = new Map();
  let total = 0;
  let traced = 0;
  for (const e of os) {
    if (e.reexec) continue;
    total++;
    if (/^[a-z0-9_]+-[0-9a-f]{16}$/.test(e.exe) && !procs.get(e.ppid)) {
      const k = `test binary (nextest's start)\u0000nextest\u0000${e.exe}`;
      rows.set(k, {
        kind: "test binary (nextest's start)",
        starter: "nextest",
        program: e.exe,
        count: (rows.get(k)?.count ?? 0) + 1,
      });
      continue;
    }
    const starter = procs.get(e.ppid);
    const parent = osPid.get(e.ppid);
    const starterName = starter ? starter.exe : (parent?.exe ?? "?");
    let kind;
    if (starter && !isTestBinary(starter)) {
      const k = `${e.ppid}\u0000${e.exe}`;
      if ((expected.get(k) ?? 0) > 0) {
        expected.set(k, expected.get(k) - 1);
        traced++;
        continue;
      }
      kind = "MISSED by the trace";
    } else if (starter && (expected.get(`${e.ppid}\u0000${e.exe}`) ?? 0) > 0) {
      // The engine's library, called in the test process, traced as it starts.
      expected.set(`${e.ppid}\u0000${e.exe}`, expected.get(`${e.ppid}\u0000${e.exe}`) - 1);
      traced++;
      continue;
    } else if (starter || /^e2e(-|$)/.test(starterName)) kind = "test process (records)";
    else if (["onepipeline", "onevcs"].includes(starterName))
      kind = "MISSED: an engine process with no trace";
    else kind = "uninstrumented descendant";
    const k = `${kind}\u0000${starterName}\u0000${e.exe}`;
    rows.set(k, {
      kind,
      starter: starterName,
      program: e.exe,
      count: (rows.get(k)?.count ?? 0) + 1,
    });
  }
  const unseen = new Map();
  for (const [k, n] of expected) {
    if (n <= 0) continue;
    const program = k.split("\u0000")[1];
    unseen.set(program, (unseen.get(program) ?? 0) + n);
  }
  const recordStarts = records.reduce(
    (a, r) => a + Object.values(r.programs ?? {}).reduce((b, v) => b + v.count, 0),
    0,
  );
  return {
    total,
    traced,
    rows: [...rows.values()].sort((a, b) => b.count - a.count),
    unseen: [...unseen].sort((a, b) => b[1] - a[1]),
    traceStarts: t.starts.length,
    recordStarts,
  };
}

function table(head, rows) {
  // A column is right-aligned when every cell in it is a figure.
  const figure = (c) => c == null || /^-?[\d.]+%?$|^-$|^blocking$/.test(String(c));
  const align = head.map((_, i) => rows.length > 0 && rows.every((r) => figure(r[i])));
  const out = [
    `| ${head.join(" | ")} |`,
    `| ${align.map((a) => (a ? "---:" : "---")).join(" | ")} |`,
  ];
  for (const r of rows)
    out.push(`| ${r.map((c) => String(c ?? "-").replaceAll("|", "\\|")).join(" | ")} |`);
  return out.join("\n");
}

export function render(folds, { top = 20 } = {}) {
  const out = [];
  for (const f of folds) {
    out.push(`## ${f.label} (${f.os})\n`);
    const h = f.host;
    if (h)
      out.push(
        `Runner: ${h.cpu_model ?? "?"}, ${h.cpus ?? "?"} logical CPUs${h.physical_cpus ? ` (${h.physical_cpus} physical)` : ""}.`,
      );
    out.push(
      `Tier: ${f.testCount} test-profile records (${f.unfinished.length} unfinished), ${(f.testSeconds / 60).toFixed(1)} test-minutes; ` +
        (f.summary
          ? `nextest \`${f.summary.line}\` = ${(f.summary.seconds / 60).toFixed(1)} min`
          : "no e2e nextest summary in job.log") +
        `; test-name set sha256/16 ${f.testsDigest}.\n`,
    );
    const k = f.skipped;
    out.push(
      `Input: ${f.trace.records} trace records read; skipped ${k.malformed} lines that are not JSON, ${k.invalid} records of no known shape, ${k.records} test-profile records not of schema 2 and ${k.waits} harness wait entries.\n`,
    );
    const ts = f.testSeconds * 1e9;
    const wm = (ns) => (f.toWallMin(ns) == null ? "-" : f.toWallMin(ns).toFixed(2));
    out.push("### Programs started\n");
    const rows = [];
    for (const e of f.engine.sort((a, b) => b.self - a.self))
      rows.push([
        "engine/libraries",
        e.origin,
        e.program,
        e.count,
        ms(median(e.durs)),
        ms(quantile(e.durs, 0.9)),
        sec(e.self),
        e.untimed,
        pct(e.self, ts),
        wm(e.self),
      ]);
    for (const e of f.tests.sort((a, b) => b.self - a.self))
      rows.push([
        "tests",
        "test process",
        e.program,
        e.count,
        "-",
        "-",
        sec(e.self),
        "-",
        pct(e.self, ts),
        wm(e.self),
      ]);
    out.push(
      table(
        [
          "from",
          "started by",
          "program",
          "count",
          "median ms",
          "p90 ms",
          "exclusive s",
          "untimed",
          "share of test-s",
          "wall min (est.)",
        ],
        rows,
      ),
    );
    const traceN = f.engine.reduce((a, e) => a + e.count, 0);
    const recN = f.tests.reduce((a, e) => a + e.count, 0);
    out.push(
      `\nRows account for ${traceN} traced starts (trace holds ${f.starts.length}) and ${recN} test-process starts (records hold ${recN}).\n`,
    );

    out.push(
      "### Engine runs the tests started, by verb (each process's own first-to-last record)\n",
    );
    out.push(
      table(
        ["program verb", "count", "median ms", "p90 ms", "total s"],
        f.verbs
          .sort((a, b) => b.count - a.count)
          .map((v) => [
            v.key,
            v.count,
            ms(median(v.lives)),
            ms(quantile(v.lives, 0.9)),
            sec(v.lives.reduce((a, b) => a + b, 0)),
          ]),
      ),
    );

    out.push("\n### By call site and engine verb\n");
    out.push(
      table(
        ["program", "site", "verb", "count", "median ms", "p90 ms", "exclusive s"],
        f.bySite
          .sort((a, b) => b.self - a.self)
          .map((e) => {
            const [prog, site, verb] = e.key.split("\u0000");
            return [
              prog,
              site,
              verb,
              e.count,
              ms(median(e.durs)),
              ms(quantile(e.durs, 0.9)),
              sec(e.self),
            ];
          }),
      ),
    );

    out.push("\n### Per-start cost\n");
    const cheapest = new Map();
    for (const e of f.bySite) {
      const [prog] = e.key.split("\u0000");
      const base = prog.split(" ")[0];
      const m = median(e.durs);
      if (m == null || e.durs.length < 3) continue;
      if (!cheapest.has(base) || m < cheapest.get(base).m) cheapest.set(base, { m, e, prog });
    }
    const spawn = new Map(
      (h?.spawn?.results ?? [])
        .filter((r) => r.median_ms != null)
        .map((r) => [r.name, r.median_ms]),
    );
    const startsOf = new Map();
    for (const e of f.engine)
      startsOf.set(e.program.split(" ")[0], (startsOf.get(e.program.split(" ")[0]) ?? 0) + e.count);
    for (const e of f.tests) startsOf.set(e.program, (startsOf.get(e.program) ?? 0) + e.count);
    out.push(
      table(
        [
          "program",
          "cheapest traced invocation",
          "its median ms",
          "minimal-start median ms (spawn step)",
          "starts",
          "start cost min of test-time (est.)",
        ],
        [...startsOf.keys()].sort().map((p) => {
          const c = cheapest.get(p);
          const floor = spawn.get(p) ?? c?.m / 1e6;
          const n = startsOf.get(p);
          return [
            p,
            c ? `${c.prog} @ ${c.e.key.split("\u0000")[1]}` : "-",
            c ? ms(c.m) : "-",
            spawn.has(p) ? spawn.get(p).toFixed(1) : "-",
            n,
            floor ? ((n * floor) / 1000 / 60).toFixed(2) : "-",
          ];
        }),
      ),
    );

    out.push("\n### git invocations repeating an earlier one in the same CLI process\n");
    out.push(`${f.repeatCount} repeats, ${sec(f.repeatNs)} s.\n`);
    out.push(
      table(
        ["verb", "program", "site", "count", "s"],
        f.repeats
          .sort((a, b) => b.ns - a.ns)
          .map((r) => [r.verb, r.program, r.site, r.count, sec(r.ns)]),
      ),
    );

    out.push("\n### Waits and polls on a child, by site\n");
    out.push(
      "Engine and library waits run inside the processes the tests start, so they overlap the tests' `spawn` time rather than their `wait`; the last column is the ratio to the records' `wait` all the same.\n",
    );
    out.push(
      table(
        [
          "site",
          "kind",
          "count",
          "polls",
          "interval ms",
          "total s",
          "share of test-s",
          "÷ test-profile wait",
        ],
        f.waits
          .sort((a, b) => b.ns - a.ns)
          .map((w) => [
            w.site,
            w.kind,
            w.count,
            w.kind === "wait" ? w.polls : "-",
            w.interval == null || w.interval === 0 ? "blocking" : ms(w.interval),
            sec(w.ns),
            pct(w.ns, ts),
            pct(w.ns / 1e9, f.profileWait),
          ]),
      ),
    );
    out.push(
      `\nTest-profile \`wait\` over the tier: ${f.profileWait.toFixed(1)} s over ${f.profileWaitCount} sleeps and polls, on the test threads.\n`,
    );
    out.push(
      "### The e2e harness's waits, by site (`profile::polled` and `profile::thread::sleep`, every thread)\n",
    );
    out.push(
      "A poll's seconds include the sleeps inside it, which are also listed at their own site.\n",
    );
    out.push(
      table(
        ["kind", "site", "count", "interval ms", "s", "share of test-profile wait"],
        f.harness
          .sort((a, b) => b.s - a.s)
          .map((w) => [
            w.kind,
            w.site.replace(/^.*?(tests\/|crates\/)/, "$1"),
            w.count,
            w.interval == null ? "-" : (w.interval * 1000).toFixed(1),
            w.s.toFixed(1),
            pct(w.s, f.profileWait),
          ]),
      ),
    );
    const polls = f.harness.filter((w) => w.kind === "poll").reduce((a, w) => a + w.s, 0);
    const bareSleeps = f.harness.filter((w) => w.kind === "sleep").reduce((a, w) => a + w.s, 0);
    out.push(
      `\nHarness polls ${polls.toFixed(1)} s and sleeps ${bareSleeps.toFixed(1)} s (the two overlap where a poll sleeps), against the records' ${f.profileWait.toFixed(1)} s of test-thread \`wait\`.\n`,
    );

    out.push(`### The ${top} tests starting the most processes\n`);
    out.push(table(["test", "starts"], [...f.perTest].sort((a, b) => b[1] - a[1]).slice(0, top)));
    out.push(`\n### The ${top} call sites starting the most processes\n`);
    out.push(table(["site", "starts"], [...f.perSite].sort((a, b) => b[1] - a[1]).slice(0, top)));

    out.push("\n### Journeys re-launching the CLI for read-only verbs\n");
    const rl = [...f.relaunch]
      .map(([test, m]) => [
        test,
        [...m.values()].reduce((a, b) => a + b, 0),
        [...m].map(([v, n]) => `${v}×${n}`).join(", "),
      ])
      .filter((r) => r[1] > 1);
    out.push(
      table(["test", "read-only launches", "verbs"], rl.sort((a, b) => b[1] - a[1]).slice(0, top)),
    );
    out.push(
      `\n${rl.length} journeys launch the CLI more than once for read-only verbs (top ${Math.min(top, rl.length)} shown).\n`,
    );

    out.push("### Trace cost\n");
    const per = f.trace.records ? (f.trace.traceNs + f.trace.writeNs) / f.trace.records : 0;
    out.push(
      `${f.trace.records} records from ${f.trace.procs} processes: ${sec(f.trace.traceNs)} s building them and ${sec(f.trace.writeNs)} s writing them, ${(per / 1000).toFixed(1)} µs per record (each process's last write is not in its own total).\n`,
    );

    if (f.coverage) {
      const c = f.coverage;
      out.push("### Coverage against the operating system's own record\n");
      out.push(
        `${c.total} process starts observed; ${c.traced} matched a traced start (the trace holds ${c.traceStarts}; the records hold ${c.recordStarts} test-process starts).\n`,
      );
      out.push(
        table(
          ["attributed to", "starter", "program", "count"],
          c.rows.map((r) => [r.kind, r.starter, r.program, r.count]),
        ),
      );
      out.push(
        `\nTraced starts the OS record has no exec for (a spawn that failed, or a process outside the OS record): ${c.unseen.map(([p, n]) => `${p} ${n}`).join(", ") || "none"}.`,
      );
      out.push("");
    }
  }
  if (folds.length > 1) {
    out.push("## Across jobs\n");
    const labels = folds.map((f) => `${f.label} (${f.os})`);
    const lead = folds.find((f) => f.os === "windows") ?? folds[0];
    for (const [what, field] of [
      ["tests", "perTest"],
      ["call sites", "perSite"],
    ]) {
      out.push(`### The ${top} ${what} starting the most on ${lead.label}, beside the others\n`);
      out.push(
        table(
          [what === "tests" ? "test" : "site", ...labels],
          [...lead[field]]
            .sort((a, b) => b[1] - a[1])
            .slice(0, top)
            .map(([k]) => [k, ...folds.map((f) => f[field].get(k) ?? 0)]),
        ),
      );
      out.push("");
    }
    const sets = folds.map((f) => new Set(f.names));
    const all = new Set(folds.flatMap((f) => f.names));
    const missing = [...all].filter((n) => sets.some((s) => !s.has(n)));
    out.push(
      `Test-name sets: ${folds.map((f) => `${f.label} ${f.names.length} (${f.testsDigest})`).join(", ")}; ${missing.length} names not in every job${missing.length ? `: ${missing.slice(0, 20).join(", ")}${missing.length > 20 ? ", ..." : ""}` : ""}.\n`,
    );
  }
  return `${out.join("\n")}\n`;
}

/** Per test, each program's traced start count, where two jobs differ. */
export function diffCounts(a, b) {
  const count = (f) => {
    const m = new Map();
    for (const s of f.starts) {
      const k = `${s.test}\u0000${programOf(s)}`;
      m.set(k, (m.get(k) ?? 0) + 1);
    }
    return m;
  };
  const ca = count(a);
  const cb = count(b);
  const keys = new Set([...ca.keys(), ...cb.keys()]);
  const rows = [];
  for (const k of keys) {
    const [test, prog] = k.split("\u0000");
    const x = ca.get(k) ?? 0;
    const y = cb.get(k) ?? 0;
    if (x !== y) rows.push([test, prog, x, y, y - x]);
  }
  return rows.sort((p, q) => Math.abs(q[4]) - Math.abs(p[4]));
}

/**
 * One hosted job's own facts, from its log: the commit it checked out, the Nx selection,
 * each cache step's restore and time, what it saved, and where the e2e tier ran in it.
 */
export function jobFacts(log) {
  const lines = log
    .replace(ANSI, "")
    .split(/\r?\n/)
    .map((line) => {
      const m = /^(\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:\.\d+)?Z) ?(.*)$/.exec(line);
      return m ? { t: Date.parse(m[1]) / 1000, text: m[2] } : null;
    })
    .filter(Boolean);
  const facts = {
    checkout: null,
    selection: null,
    steps: [],
    cacheSaves: [],
    nxStored: null,
    e2e: null,
  };
  const steps = [];
  for (let i = 0; i < lines.length; i++) {
    const { t, text } = lines[i];
    const step = /^##\[group\]Run (.*)$/.exec(text);
    if (step) steps.push({ name: step[1].trim(), start: t, lines: [] });
    else if (steps.length) steps.at(-1).lines.push(text);
    const head = /^HEAD is now at (\w+) Merge (\w+) into (\w+)/.exec(text);
    if (head) facts.checkout = { merge: head[1], head: head[2], base: head[3] };
    const sel = /^ci-job: (\S+) runs (\d+) task\(s\): (.*)$/.exec(text);
    if (sel)
      facts.selection = { job: sel[1], count: Number(sel[2]), tasks: sel[3].trim().split(/\s+/) };
    const stored = /^Nx cache: stored (\d+) entries, (\d+) bytes; replayed (\d+) entries/.exec(
      text,
    );
    if (stored)
      facts.nxStored = {
        entries: Number(stored[1]),
        bytes: Number(stored[2]),
        replayed: Number(stored[3]),
      };
    if (/^Saving cache path|^Cache saved|tar --posix -cf cache\.tzst/.test(text))
      facts.cacheSaves.push(text.slice(0, 160));
  }
  const end = lines.at(-1)?.t ?? 0;
  steps.forEach((s, i) => {
    s.end = steps[i + 1]?.start ?? end;
  });
  for (const s of steps) {
    if (
      !/rust-cache|actions\/cache\/restore|setup-uv|start-cache-server|setup-node|setup-python/.test(
        s.name,
      )
    )
      continue;
    const said = s.lines
      .filter((l) =>
        /Cache hit|Cache restored|full match|No cache found|Cache not found|cache miss|not found|Restored from/i.test(
          l,
        ),
      )
      .map((l) => l.trim());
    const hit = said.some((l) => /full match: true|Cache hit for:|Cache restored from key/i.test(l))
      ? "hit"
      : said.some((l) => /restore-key|full match: false/i.test(l))
        ? "partial (restore key)"
        : said.length
          ? "miss"
          : "no restore";
    facts.steps.push({
      name: s.name.slice(0, 60),
      seconds: s.end - s.start,
      hit,
      said: said.slice(0, 3),
    });
  }
  // The e2e tier: the largest nextest run.
  facts.e2e = e2eSummary(log);
  const gate = steps.find((s) => /just ci-test-/.test(s.name));
  facts.job = { start: lines[0]?.t ?? 0, end, gateStart: gate?.start ?? null };
  return facts;
}

function renderFacts(label, f) {
  const min = (s) => (s / 60).toFixed(1);
  const out = [`## ${label}\n`];
  if (f.checkout)
    out.push(
      `Checked out ${f.checkout.merge}, merging head ${f.checkout.head} into base ${f.checkout.base}.`,
    );
  out.push(
    `Job ${min(f.job.end - f.job.start)} min; gate step from ${f.job.gateStart ? min(f.job.gateStart - f.job.start) : "-"} min.`,
  );
  if (f.e2e)
    out.push(
      `e2e tier: nextest summary ${min(f.e2e.seconds)} min, ${f.e2e.run} run, ${f.e2e.passed} passed; it started ${min(f.e2e.start - f.job.start)} min into the job (${f.job.gateStart ? min(f.e2e.start - f.job.gateStart) : "-"} min into the gate step, the compile and the tasks before it) and ended ${min(f.job.end - f.e2e.end)} min before the job did.`,
    );
  if (f.selection)
    out.push(`Nx selection: ${f.selection.count} tasks: ${f.selection.tasks.join(" ")}.`);
  out.push(
    `Nx remote cache: ${f.nxStored ? `stored ${f.nxStored.entries} entries (${f.nxStored.bytes} bytes), replayed ${f.nxStored.replayed}` : "no report line"}. Actions-cache saves in the log: ${f.cacheSaves.length}${f.cacheSaves.length ? ` (${f.cacheSaves.join("; ")})` : ""}.\n`,
  );
  out.push(
    table(
      ["cache step", "s", "restore", "what it said"],
      f.steps.map((s) => [s.name, s.seconds.toFixed(1), s.hit, s.said.join(" / ") || "-"]),
    ),
  );
  return `${out.join("\n")}\n`;
}

/** The runner: its processor, as each OS names it, and each CLI's minimal start. */
export async function host({ runs = 30 } = {}) {
  if (!Number.isSafeInteger(runs) || runs < 1) throw usage("--runs takes a positive whole number");
  const { measureSpawns } = await import("./windows-job-profile.mjs");
  const unanswered = [];
  const ask = (cmd, args) => {
    try {
      return execFileSync(cmd, args, {
        encoding: "utf8",
        stdio: ["ignore", "pipe", "pipe"],
      }).trim();
    } catch (error) {
      unanswered.push(`${cmd} ${args.join(" ")}: ${String(error.stderr || error.message).trim()}`);
      return null;
    }
  };
  let model = cpus()[0]?.model ?? null;
  let physical = null;
  if (process.platform === "linux" && existsSync("/proc/cpuinfo"))
    model = /model name\s*:\s*(.*)/.exec(readFileSync("/proc/cpuinfo", "utf8"))?.[1] ?? model;
  if (process.platform === "darwin") {
    model = ask("sysctl", ["-n", "machdep.cpu.brand_string"]) ?? model;
    physical = Number(ask("sysctl", ["-n", "hw.physicalcpu"])) || null;
  }
  const exe = process.platform === "win32" ? ".exe" : "";
  const programs = [];
  for (const dir of ["llvm-cov-target/debug"]) {
    const path = join(process.cwd(), "target", dir, `onepipeline${exe}`);
    if (existsSync(path)) programs.push(`onepipeline (instrumented)=${path}`);
  }
  return {
    platform: process.platform,
    cpu_model: model,
    cpus: cpus().length,
    physical_cpus: physical,
    unanswered,
    runner: {
      name: process.env.RUNNER_NAME ?? null,
      image: process.env.ImageOS ?? null,
      version: process.env.ImageVersion ?? null,
    },
    spawn: measureSpawns({ runs, programs }),
  };
}

function usage(message) {
  return new Refusal(
    message,
    "run 'node scripts/launch-inventory.mjs fold LABEL=DIR ...' or '... diff A=DIR B=DIR'",
    2,
  );
}

export async function main(argv) {
  const [verb, ...rest] = argv;
  if (verb === "host") {
    const at = rest.indexOf("--runs");
    return `${JSON.stringify(await host({ runs: at < 0 ? 30 : Number(rest[at + 1]) }), null, 2)}\n`;
  }
  if (verb === "run") {
    if (!rest.length) throw usage("name at least one LABEL=LOG job log");
    return rest
      .map((arg) => {
        const at = arg.indexOf("=");
        if (at < 1) throw usage(`'${arg}' is not LABEL=LOG`);
        const path = arg.slice(at + 1);
        if (!existsSync(path))
          throw new Refusal(
            `${arg.slice(0, at)}: ${path} does not exist`,
            "save the job's log there (gh api .../actions/jobs/<id>/logs)",
          );
        return renderFacts(arg.slice(0, at), jobFacts(readFileSync(path, "utf8")));
      })
      .join("\n");
  }
  let top = 20;
  const jobs = [];
  for (let i = 0; i < rest.length; i++) {
    const arg = rest[i];
    if (arg === "--top") {
      top = Number(rest[++i]);
      if (!Number.isSafeInteger(top) || top < 1) throw usage("--top takes a positive whole number");
    } else if (/^[^=]+=.+/.test(arg)) {
      const at = arg.indexOf("=");
      jobs.push(foldJob(readJob(arg.slice(0, at), arg.slice(at + 1))));
    } else throw usage(`unknown argument '${arg}'`);
  }
  // llmlint: ignore-block[tool_output_is_signal] fold, diff, run and host are report commands: the multi-line Markdown or JSON they print on success is the whole of what they are for, and they never lands: this branch is preserved as the measurement record and nothing on it merges.
  if (verb === "fold") {
    if (!jobs.length) throw usage("name at least one LABEL=DIR job");
    return render(jobs, { top });
  }
  if (verb === "diff") {
    if (jobs.length !== 2) throw usage("diff takes exactly two LABEL=DIR jobs");
    const rows = diffCounts(jobs[0], jobs[1]);
    return `${table(["test", "program", jobs[0].label, jobs[1].label, "difference"], rows)}\n\n${rows.length} (test, program) pairs differ.\n`;
  }
  // llmlint: ignore-end[tool_output_is_signal]
  throw usage(verb ? `unknown verb '${verb}'` : "name a verb: fold, diff, run or host");
}

if (import.meta.url === pathToFileURL(process.argv[1] ?? "").href) {
  try {
    process.stdout.write(await main(process.argv.slice(2)));
  } catch (error) {
    const refusal =
      error instanceof Refusal
        ? error
        : new Refusal(
            `could not fold the input: ${error.message}`,
            "check that each LABEL=DIR is one job's unzipped test-profile artifact, and report the input if it is",
          );
    process.stderr.write(`launch-inventory: ${refusal.message}\nnext: ${refusal.next}\n`);
    process.exitCode = refusal.code;
  }
}
