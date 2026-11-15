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

// llmlint: ignore-block[contracts_have_one_source_or_a_drift_gate] git's read/write semantics are git's, held nowhere this repository could gate against; a form this does not recognise is counted as a write, which can only under-count repeats, and this never lands: this branch is preserved as the measurement record and nothing on it merges.
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
]);

/**
 * Whether one git argv only reads. `config`, `branch`, `worktree`, `remote`, `symbolic-ref`,
 * `tag`, `stash` and `hash-object` read in some forms and write in others; any flag or
 * argument shape this does not recognise as a read is a write, so a repeat is never counted
 * across something that might have changed what it reads.
 */
export function gitReads(sub, argv) {
  if (!sub) return false;
  // `--output=<file>` makes even a diff or a log a write to the filesystem.
  if (argv.some((a) => a === "--output" || a.startsWith("--output=") || a === "-o")) return false;
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

// llmlint: ignore-end[contracts_have_one_source_or_a_drift_gate]
/**
 * The engine's verbs that read and change nothing, from its `--help`: a journey re-launching
 * the CLI for one is re-reading. `review`, `unpublished` and `supersessions` read unless
 * given one of `RECORDING_FLAGS`, so a launch carrying one is not counted as a read.
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
]);

/** Every other verb `--help` lists: each changes a run, a branch, or the host. */
export const WRITE_VERBS = new Set([
  "human-items",
  "scratch",
  "start",
  "adopt",
  "readjust",
  "reply",
  "surface",
  "decline-ruling",
  "attest",
  "stop",
  "label",
  "shutdown",
  "ask",
  "flow",
  "publish-branch",
  "repo-recover",
  "integrate",
]);

// llmlint: ignore[contracts_have_one_source_or_a_drift_gate] which of a verb's flags record is the engine's behaviour, stated in no machine-readable form a gate could compare; a flag missing here can only count a recording launch as a read, which over-states re-reads in a measurement table that never lands: this branch is preserved as the measurement record and nothing on it merges.
/** The flags that make a reading verb record something. */
export const RECORDING_FLAGS = ["--record", "--acknowledge", "--decide", "--reply", "--post"];

/** Whether one engine launch, by its argv, only reads. */
export function launchReads(verb, argv) {
  return READ_VERBS.has(verb) && !(argv ?? []).some((a) => RECORDING_FLAGS.includes(a));
}

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

const isTime = (x) => Number.isFinite(x) && x >= 0;
/** A count or an id: a whole number, not negative. */
const isCount = (x) => Number.isSafeInteger(x) && x >= 0;

/** A GIT_* set as a child was given it: each name a GIT_ variable, each value a string or unset. */
const gitEnv = (env) =>
  Boolean(env) &&
  typeof env === "object" &&
  !Array.isArray(env) &&
  Object.entries(env).every(
    ([k, v]) => k.startsWith("GIT_") && (v === null || typeof v === "string"),
  );
const isTimeOrNull = (x) => x == null || isTime(x);

/** Whether `r` is a trace record of a kind this fold reads, its fields of their types. */
export function validRecord(r) {
  const fields =
    r && typeof r === "object" && r.v === 1 && Object.hasOwn(RECORD_FIELDS, r.kind)
      ? RECORD_FIELDS[r.kind]
      : null;
  if (!fields?.every((f) => r[f] != null) || !isCount(r.pid)) return false;
  if (r.child != null && !isCount(r.child)) return false;
  if (r.polls != null && !isCount(r.polls)) return false;
  if (r.wait_site != null && (!isTime(r.wait_ns) || !isCount(r.polls))) return false;
  if (r.t != null && !isTime(r.t)) return false;
  const scalar = (x) => x == null || typeof x === "string" || Number.isFinite(x);
  if (
    !scalar(r.id) ||
    !scalar(r.seq) ||
    (r.env != null && typeof r.env !== "object" && !scalar(r.env))
  )
    return false;
  if (r.site != null && typeof r.site !== "string") return false;
  if (["start", "wait", "sleeps"].includes(r.kind) && typeof r.function !== "string") return false;
  for (const f of ["function", "wait_site", "verb", "test"])
    if (r[f] != null && typeof r[f] !== "string") return false;
  if (
    !["child", "interval_ns", "polls", "write_ns_before", "trace_ns"].every((f) =>
      isTimeOrNull(r[f]),
    )
  )
    return false;
  if (r.kind === "env") return gitEnv(r.env);
  if (r.kind === "start" && r.env != null && typeof r.env === "object" && !gitEnv(r.env))
    return false;
  if (r.kind === "proc")
    return (
      typeof r.exe === "string" &&
      (r.verb == null || typeof r.verb === "string") &&
      (r.test == null || typeof r.test === "string") &&
      (r.argv == null || (Array.isArray(r.argv) && r.argv.every((a) => typeof a === "string")))
    );
  if (r.kind === "start")
    return (
      typeof r.program === "string" &&
      (r.cwd == null || typeof r.cwd === "string") &&
      (r.sub == null || typeof r.sub === "string") &&
      Array.isArray(r.argv) &&
      r.argv.every((a) => typeof a === "string") &&
      ["spawn_ns", "exit_ns", "wait_ns", "trace_ns"].every((f) => isTimeOrNull(r[f]))
    );
  if (r.kind === "exit") return ["exit_ns", "t", "wait_ns"].every((f) => isTimeOrNull(r[f]));
  if (r.kind === "wait") return isTime(r.wait_ns) && isCount(r.polls);
  if (r.kind === "sleeps") return isCount(r.count) && isTime(r.slept_ns);
  return true;
}

/** The six categories every schema-2 test-profile record carries, in profile.rs's order. */
const CATEGORIES = ["spawn", "git", "build", "fixture", "wait", "fs"];

// llmlint: ignore[contracts_have_one_source_or_a_drift_gate] the gate is a chain the rule does not see: tests/e2e/profiling.rs holds every record crates/testfakes/src/profile.rs writes to tests/golden/test-profile-record-v2.json's shape, and npm/test/launch-inventory.test.mjs holds this check to that same golden.
/** Whether `r` is a test-profile record of schema 2 (crates/testfakes/src/profile.rs). */
export function validProfile(r) {
  return (
    Boolean(r) &&
    typeof r === "object" &&
    r.schema === 2 &&
    typeof r.test === "string" &&
    typeof r.os === "string" &&
    ["test-thread", "process-exit", "unfinished"].includes(r.ended) &&
    isTime(r.wall_s) &&
    (r.categories?.wait?.polls == null || isCount(r.categories.wait.polls)) &&
    Boolean(r.categories) &&
    typeof r.categories === "object" &&
    !Array.isArray(r.categories) &&
    CATEGORIES.every(
      (name) =>
        r.categories[name] && isTime(r.categories[name].s) && isCount(r.categories[name].count),
    ) &&
    Boolean(r.programs) &&
    typeof r.programs === "object" &&
    !Array.isArray(r.programs) &&
    Object.values(r.programs).every((v) => v && isCount(v.count) && isTime(v.s))
  );
}

// llmlint: ignore[contracts_have_one_source_or_a_drift_gate] the waits document is written by crates/testfakes/src/profile.rs's wait_sites on this branch, and the fixture is its own output recorded at this commit; a cross-language gate would be code for a reader that never lands: this branch is preserved as the measurement record and nothing on it merges.
/** Whether `w` is one site of a harness waits document. */
const validWait = (w) =>
  Boolean(w) &&
  typeof w === "object" &&
  ["span", "poll", "sleep"].includes(w.kind) &&
  typeof w.site === "string" &&
  isCount(w.count) &&
  isTime(w.s) &&
  (w.interval_s == null || isTime(w.interval_s));

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

const VERSION_ONLY_TEST = "surface::version_reports_the_crate_version";

/** Every record a job's artifact holds. */
export function readJob(label, dir) {
  if (!existsSync(dir))
    throw new Refusal(
      `${label}: ${dir} does not exist`,
      "unzip the job's test-profile artifact there",
    );
  const root = existsSync(join(dir, "test-profile")) ? join(dir, "test-profile") : dir;
  const skipped = { malformed: 0, invalid: 0, records: 0, waits: 0, otherBinaries: 0 };
  const tests = [];
  const e2e = join(root, "e2e");
  if (existsSync(e2e)) {
    for (const f of readdirSync(e2e)) {
      if (!f.endsWith(".json")) continue;
      const r = readJson(join(e2e, f));
      if (!validProfile(r)) skipped.records++;
      // The justfile's e2e-tier excludes this test; onepipeline-version-e2e runs it
      // separately from the same binary. The command test gates this name to that filter.
      else if (r.test !== VERSION_ONLY_TEST) tests.push(r);
    }
  }
  let trace = [];
  const harnessWaits = [];
  const launch = join(root, "launch");
  // The note, smoke and other binaries that compile the e2e harness write into the same
  // directory: with e2e records present, only the files of the tests they name are the
  // e2e tier's.
  const e2eTests = new Set(tests.map((r) => r.test.replaceAll("::", "__")));
  const ours = (file) =>
    file !== VERSION_ONLY_TEST.replaceAll("::", "__") &&
    (e2eTests.size === 0 || e2eTests.has(file));
  if (existsSync(launch)) {
    for (const f of readdirSync(launch)) {
      if (f.endsWith(".waits.json")) {
        if (!ours(basename(f, ".waits.json"))) {
          skipped.otherBinaries++;
          continue;
        }
        const doc = readJson(join(launch, f));
        const sites =
          doc && typeof doc.test === "string" && Array.isArray(doc.sites) ? doc.sites : [];
        const valid = sites.filter(validWait);
        skipped.waits += sites.length - valid.length + (sites === doc?.sites ? 0 : 1);
        for (const site of valid) harnessWaits.push({ ...site, test: doc.test });
        continue;
      }
      if (!f.endsWith(".jsonl")) continue;
      const file = basename(f, ".jsonl");
      if (!ours(file)) {
        skipped.otherBinaries++;
        continue;
      }
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
  const result = (x) =>
    x &&
    typeof x === "object" &&
    typeof x.name === "string" &&
    (x.error != null ? x.median_ms == null : isTime(x.median_ms));
  const facts = (h) =>
    (h.cpu_model == null || typeof h.cpu_model === "string") &&
    isTime(h.cpus) &&
    (h.physical_cpus == null || isTime(h.physical_cpus));
  if (
    host !== null &&
    !(
      host &&
      typeof host === "object" &&
      facts(host) &&
      Array.isArray(host.spawn?.results) &&
      host.spawn.results.every(result)
    )
  )
    throw new Refusal(
      `${hostFile} holds no spawn results`,
      "regenerate it with 'node scripts/launch-inventory.mjs host'",
    );
  const logFile = join(dir, "job.log");
  const log = existsSync(logFile) ? readFileSync(logFile, "utf8") : null;
  const completed = largestRun(log);
  const matching = tests.filter((r) => r.run_id === completed?.id).length;
  const scope =
    completed?.start != null && completed.run >= tests.length && matching > tests.length / 2
      ? { ...completed, records: 0, starts: 0, os: 0, otherProfiles: tests.length - matching }
      : null;
  // GitHub timestamps received nextest output, rather than the trace's clock read.
  // One second of slack keeps boundary events while excluding earlier/later tiers.
  const inside = (t) => t >= (scope.start - 1) * 1e9 && t <= (scope.end + 1) * 1e9;
  if (scope) {
    const active = new Map();
    const kept = trace.filter((r) => {
      const key = `${r.file}\u0000${r.pid}`;
      if (r.t != null) active.set(key, inside(r.t));
      // Untimed env/exit records belong to the preceding record of their writer.
      const keep = active.get(key) === true;
      if (!keep) {
        scope.records++;
        if (r.kind === "start") scope.starts++;
      }
      return keep;
    });
    trace = kept;
  }
  const observed = readOs(
    existsSync(join(dir, "os")) ? join(dir, "os") : join(root, "os"),
    skipped,
  );
  const os =
    observed?.filter((e) => {
      const keep = !scope || ((e.test == null || ours(e.test)) && (e.t == null || inside(e.t)));
      if (!keep && scope) scope.os++;
      return keep;
    }) ?? null;
  return { label, tests, trace, harnessWaits, host, log, os, skipped, scope };
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
  let id = null;
  for (const line of lines) {
    const t = Date.parse(/^(\S+Z)\s/.exec(line)?.[1] ?? "") / 1000;
    if (!Number.isFinite(t)) continue;
    const nextest = /Nextest run ID\s+(\S+)/.exec(line);
    if (nextest) {
      start = t;
      id = nextest[1];
    }
    const m = /Summary \[\s*([\d.]+)s\]\s+(\d+) tests? run: (\d+) passed/.exec(line);
    if (m)
      runs.push({
        id,
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

/**
 * The largest nextest run a job log holds, or null. In this repository's test jobs that is
 * the e2e tier, whose ~2,150 journeys no other tier approaches, and the reports print it as
 * the largest run, naming that tier only as what it is in these jobs.
 */
export function largestRun(log) {
  if (!log) return null;
  return nextestRuns(log).reduce((best, r) => (!best || r.run > best.run ? r : best), null);
}

/** The operating system's own process starts: `strace -f` output or ETW JSONL. */
export function readOs(dir, skipped = { malformed: 0, invalid: 0 }) {
  if (!existsSync(dir)) return null;
  const starts = [];
  for (const f of readdirSync(dir)) {
    const path = join(dir, f);
    if (f.endsWith(".strace")) {
      // `<test>.<runner pid>.strace`: its starts are that test's alone.
      const test = f.replace(/\.\d+\.strace$/, "");
      for (const e of parseStrace(readFileSync(path, "utf8"), f)) starts.push({ ...e, test });
    } else if (f.endsWith(".jsonl"))
      for (const line of readFileSync(path, "utf8").split("\n")) {
        if (!line.trim()) continue;
        let r;
        try {
          r = JSON.parse(line);
        } catch {
          skipped.malformed++;
          continue;
        }
        if (
          !r ||
          typeof r !== "object" ||
          !isCount(r.pid) ||
          !isCount(r.ppid) ||
          typeof r.image !== "string" ||
          (r.cmd != null && typeof r.cmd !== "string")
        ) {
          skipped.invalid++;
          continue;
        }
        starts.push({
          pid: r.pid,
          ppid: r.ppid,
          exe: stem(r.image),
          argv: splitCmd(r.cmd ?? ""),
          file: f,
          t: etwTime(r.t),
        });
      }
  }
  return starts;
}

/**
 * An ETW start's `TimeCreated` (ISO 8601, to 100 ns) as unix nanoseconds, the trace's own
 * unit; null when absent or unreadable, which leaves the join to the pid alone.
 */
export function etwTime(iso) {
  if (typeof iso !== "string") return null;
  const m = /^(.*?T\d\d:\d\d:\d\d)(?:\.(\d+))?(Z|[+-]\d\d:\d\d)$/.exec(iso);
  if (!m) return null;
  const ms = Date.parse(`${m[1]}${m[3]}`);
  if (!Number.isFinite(ms)) return null;
  return ms * 1e6 + Number(`${m[2] ?? ""}000000000`.slice(0, 9));
}

// The trace names a program by Rust's `Path::file_stem` (everything before the last dot),
// so the OS's image is named the same way: `probe.sh` and `git.exe` are `probe` and `git`.
const stem = (path) => {
  const name = basename(String(path).replace(/\\/g, "/"));
  const dot = name.lastIndexOf(".");
  return (dot > 0 ? name.slice(0, dot) : name).toLowerCase();
};
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
/** Separate process lifetimes without changing the recorded schema. Linked copies emit
 * duplicate proc/end records; a new proc after an end, a changed argv, or a repeated
 * per-copy start sequence proves a new lifetime for a reused PID. */
function processInstances(trace) {
  const byPid = new Map();
  for (const r of trace) {
    const k = `${r.file}\u0000${r.pid}`;
    if (!byPid.has(k)) byPid.set(k, []);
    if (r.t != null) byPid.get(k).push(r);
  }
  for (const events of byPid.values()) {
    events.sort((a, b) => a.t - b.t);
    let part = [];
    let instance = 0;
    let ended = false;
    let signature = null;
    let lastProc = 0;
    let sequences = new Set();
    const finish = (records) => {
      for (const r of records) r.instance = instance;
      instance++;
    };
    for (const r of events) {
      if (r.kind === "proc") {
        const next = JSON.stringify([r.exe, r.argv]);
        if (part.length && (ended || (signature != null && signature !== next))) {
          finish(part);
          part = [];
          sequences = new Set();
          ended = false;
        }
        signature = next;
        lastProc = part.length;
      }
      if (r.kind === "start" && r.seq != null && sequences.has(r.seq)) {
        // The same linked copy cannot initialize its sequence twice in one process.
        // Its most recent proc record starts the replacement lifetime, even if the
        // previous process was killed before its end marker could be written.
        const split = lastProc > 0 ? lastProc : part.length;
        finish(part.slice(0, split));
        part = part.slice(split);
        sequences = new Set(part.filter((e) => e.kind === "start").map((e) => e.seq));
        ended = false;
        lastProc = 0;
      }
      part.push(r);
      if (r.kind === "start" && r.seq != null) sequences.add(r.seq);
      if (r.kind === "end") ended = true;
    }
    finish(part);
  }
}

export function joinTrace(trace) {
  processInstances(trace);
  const procs = new Map();
  const ends = new Map();
  const exits = new Map();
  const starts = [];
  const waits = [];
  const sleeps = [];
  const envs = new Map();
  const bySeq = new Map();
  const key = (file, pid, instance = 0) => `${file}\u0000${pid}\u0000${instance}`;
  const recordKey = (r) => key(r.file, r.pid, r.instance);
  // Environment ids are local to a linked copy and may restart when a PID is reused.
  // Resolve each reference against the preceding definition in the append-only file.
  for (const r of trace) {
    const envKey = (id) => `${r.file}\u0000${r.pid}\u0000${id}`;
    if (r.kind === "env") envs.set(envKey(r.id), r.env);
    else if (r.kind === "start" && (typeof r.env === "number" || typeof r.env === "string"))
      r.env = envs.get(envKey(r.env)) ?? { unresolved: r.env };
  }
  for (const r of trace) {
    if (r.kind === "proc" && !procs.has(recordKey(r))) procs.set(recordKey(r), r);
    else if (r.kind === "end") ends.set(recordKey(r), r.t);
    else if (r.kind === "exit" && r.seq != null) bySeq.set(`${recordKey(r)}\u0000${r.seq}`, r);
    else if (r.kind === "exit") exits.set(`${recordKey(r)}\u0000${r.child}`, r.t);
    else if (r.kind === "start") {
      starts.push(r);
    } else if (r.kind === "wait") waits.push(r);
    else if (r.kind === "sleeps") sleeps.push(r);
  }
  // A start written at its spawn has its exit, and any wait folded into it, in an
  // `exit` record of the same seq.
  for (const s of starts) {
    const e = s.seq != null ? bySeq.get(`${recordKey(s)}\u0000${s.seq}`) : null;
    if (!e) continue;
    for (const f of ["exit_ns", "status", "wait_site", "wait_ns", "polls", "interval_ns"])
      if (e[f] != null) s[f] = e[f];
  }
  for (const r of [...starts, ...waits, ...sleeps]) {
    const p = procs.get(recordKey(r));
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
      const at = exits.get(`${recordKey(s)}\u0000${s.child}`);
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
    if (s.child != null)
      child =
        candidates.find(
          (p) => p.pid === s.child && p.t >= s.t && (s.dur == null || p.t <= s.t + s.dur),
        ) ?? null;
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
        const end = ends.get(recordKey(child));
        if (end != null) s.dur = end - s.t;
      }
    }
  }
  const startsOf = new Map();
  for (const s of starts) {
    const k = recordKey(s);
    if (!startsOf.has(k)) startsOf.set(k, []);
    startsOf.get(k).push(s);
  }
  for (const s of starts) {
    const inner = s.childProc ? (startsOf.get(recordKey(s.childProc)) ?? []) : [];
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
const siteOf = (s) =>
  `${s.function} (${s.site.replaceAll("\\", "/").replace(/^.*?(src\/|vendor\/)/, "$1")})`;
const isTestBinary = (p) => Boolean(p) && /^[a-z0-9_]+-[0-9a-f]{16}$/.test(p.exe);

/** A job folded: every figure the tables print, as data. */
export function foldJob(job) {
  const t = joinTrace(job.trace);
  const testSeconds = job.tests.reduce((a, r) => a + (r.wall_s ?? 0), 0);
  const completed = largestRun(job.log);
  // A cancelled e2e run can leave profiles but no summary. A smaller tier's
  // completed summary cannot price those profiles' share of the e2e wall time.
  const summary = completed && completed.run >= job.tests.length ? completed : null;
  const wallS = summary?.seconds ?? null;
  const toWallMin = (ns) => (wallS && testSeconds ? ((ns / 1e9 / testSeconds) * wallS) / 60 : null);
  const procOf = (s) =>
    t.procs.find((p) => p.file === s.file && p.pid === s.pid && p.instance === s.instance);
  const procIndex = new Map(t.procs.map((p) => [t.key(p.file, p.pid, p.instance), p]));
  const startedBy = (s) => procIndex.get(t.key(s.file, s.pid, s.instance));

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
    const inner = t.startsOf.get(t.key(p.file, p.pid, p.instance)) ?? [];
    const end =
      t.ends.get(t.key(p.file, p.pid, p.instance)) ??
      Math.max(p.t, ...inner.map((c) => c.t + (c.dur ?? 0)));
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

  const verbs = new Map();
  for (const p of t.procs) {
    if (isTestBinary(p) || p.parentStart) continue;
    const k = `${p.exe} ${p.verb || "-"}`;
    if (!verbs.has(k)) verbs.set(k, { key: k, count: 0, lives: [] });
    const e = verbs.get(k);
    e.count++;
    const end = t.ends.get(t.key(p.file, p.pid, p.instance));
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

  const perTest = new Map();
  for (const s of t.starts) perTest.set(s.test || s.file, (perTest.get(s.test || s.file) ?? 0) + 1);
  for (const r of job.tests)
    perTest.set(
      r.test,
      (perTest.get(r.test) ?? 0) + Object.values(r.programs ?? {}).reduce((a, v) => a + v.count, 0),
    );
  const perSite = new Map();
  for (const s of t.starts) perSite.set(siteOf(s), (perSite.get(siteOf(s)) ?? 0) + 1);

  const relaunch = new Map();
  for (const p of t.procs) {
    if (p.exe !== "onepipeline" || p.parentStart || !launchReads(p.verb, p.argv)) continue;
    if (!relaunch.has(p.test)) relaunch.set(p.test, new Map());
    const m = relaunch.get(p.test);
    m.set(p.verb, (m.get(p.verb) ?? 0) + 1);
  }

  let traceNs = 0;
  let records = 0;
  const lastWrite = new Map();
  const endedWrites = new Map();
  for (const r of job.trace) {
    records++;
    traceNs += r.trace_ns ?? 0;
    const k = t.key(r.file, r.pid, r.instance);
    lastWrite.set(k, Math.max(lastWrite.get(k) ?? 0, r.write_ns_before ?? 0));
    // Every linked copy owns its own cumulative writer counter and emits its own end.
    // Summing those ends retains all writers; taking one PID's maximum loses copies.
    if (r.kind === "end") endedWrites.set(k, (endedWrites.get(k) ?? 0) + (r.write_ns_before ?? 0));
  }
  const writeNs = [...lastWrite].reduce((a, [k, latest]) => a + (endedWrites.get(k) ?? latest), 0);

  const names = job.tests.map((r) => r.test).sort();
  return {
    label: job.label,
    scope: job.scope,
    skipped: job.skipped ?? { malformed: 0, invalid: 0, records: 0, waits: 0, otherBinaries: 0 },
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
  // A pid names many processes over a whole tier — Windows reuses them within seconds — so
  // an OS start is first the traced start that named it as its child, in the same test (an
  // strace file is one test's) or, where the OS gave the start a time (ETW), the nearest
  // within SLACK. One with no such start falls back to its parent: the traced process of
  // that pid in the same test, or the one whose first record is the latest before it,
  // allowing SLACK, because ETW's clock drifts tens of milliseconds from the trace's.
  // A start the trace knows the child of is only ever matched by that child; the fallback
  // counts, per traced process and program, only the starts that name none.
  const SLACK = 100e6;
  const procsByPid = new Map();
  for (const p of t.procs) {
    if (!procsByPid.has(p.pid)) procsByPid.set(p.pid, []);
    procsByPid.get(p.pid).push(p);
  }
  const osByPid = new Map();
  for (const e of os) {
    if (e.reexec) continue;
    if (!osByPid.has(e.pid)) osByPid.set(e.pid, []);
    osByPid.get(e.pid).push(e);
  }
  // Which test a candidate belongs to: a traced process by its trace file, an OS start by
  // its strace file's test.
  const latest = (candidates, e, time, testOf) => {
    if (!candidates?.length) return null;
    let pool = candidates;
    if (e.test != null) pool = pool.filter((c) => testOf(c) === e.test);
    if (e.t != null) {
      let best = null;
      for (const c of pool) {
        const at = time(c);
        if (at != null && at <= e.t + SLACK && (best == null || at > time(best))) best = c;
      }
      return best;
    }
    return pool[0] ?? null;
  };
  // The parent itself, as the OS saw it start: on ETW's own clock, so with no slack.
  const parentOf = (e) => {
    if (e.t == null)
      return latest(
        osByPid.get(e.ppid),
        e,
        (o) => o.t,
        (o) => o.test,
      );
    let best = null;
    for (const o of osByPid.get(e.ppid) ?? [])
      if (o.t != null && o.t <= e.t && (best == null || o.t > best.t)) best = o;
    return best;
  };
  // A traced process is the starter only if it is that parent: its first record falls
  // after the parent's OS start, so a pid an untraced process reused is not taken for an
  // engine's.
  const starterOf = (e, parent) => {
    if (e.t == null || parent?.t == null)
      return latest(
        procsByPid.get(e.ppid),
        e,
        (p) => p.t,
        (p) => p.file,
      );
    let best = null;
    for (const p of procsByPid.get(e.ppid) ?? [])
      if (p.t >= parent.t - SLACK && p.t <= e.t + SLACK && (best == null || p.t < best.t)) best = p;
    return best;
  };
  const lineage = new Map();
  const descendsFromTier = (event) => {
    const path = [];
    const seen = new Set();
    let current = event;
    let proven = false;
    while (current) {
      if (lineage.has(current)) {
        proven = lineage.get(current);
        break;
      }
      // Corrupt input or a reused PID must not turn an ancestry walk into a loop.
      if (seen.has(current)) break;
      seen.add(current);
      path.push(current);
      const parent = parentOf(current);
      if (/^e2e(?:-[0-9a-f]{16})?$/.test(current.exe) || starterOf(current, parent)) {
        proven = true;
        break;
      }
      current = parent;
    }
    for (const e of path) lineage.set(e, proven);
    return proven;
  };
  const id = (file, pid, instance = 0) => `${file}\u0000${pid}\u0000${instance}`;
  const expected = new Map();
  const byChild = new Map();
  const keyOf = (s) => `${id(s.file, s.pid, s.instance)}\u0000${s.program.toLowerCase()}`;
  // A spawn that returned no child (`spawn_ns` set, `child` null) started no process: it is
  // reported as one the OS has no exec for, and never absorbs a start the OS did see.
  const failed = [];
  for (const s of t.starts) {
    if (s.child == null && s.spawn_ns != null) {
      failed.push(s);
      continue;
    }
    if (s.child == null) {
      expected.set(keyOf(s), (expected.get(keyOf(s)) ?? 0) + 1);
      continue;
    }
    if (!byChild.has(s.child)) byChild.set(s.child, []);
    byChild.get(s.child).push(s);
  }
  const claimed = new Set();
  const namedBy = (e) => {
    let best = null;
    for (const s of byChild.get(e.pid) ?? []) {
      if (claimed.has(s)) continue;
      // Windows executes .cmd/.bat programs through this very child, cmd.exe.
      if (s.program.toLowerCase() !== e.exe && e.exe !== "cmd") continue;
      if (e.ppid != null && s.pid !== e.ppid) continue;
      if (e.test != null && s.file !== e.test) continue;
      if (e.t != null && (e.t < s.t - SLACK || e.t > s.t + (s.spawn_ns ?? 0) + SLACK)) continue;
      if (best == null || (e.t != null && Math.abs(e.t - s.t) < Math.abs(e.t - best.t))) best = s;
    }
    return best;
  };
  const rows = new Map();
  let total = 0;
  let traced = 0;
  // What the records say the test processes started, per program, consumed as the OS's
  // record of those starts is read; the rest are starts the records do not hold.
  const recorded = new Map();
  for (const r of records)
    for (const [prog, v] of Object.entries(r.programs ?? {}))
      recorded.set(prog, (recorded.get(prog) ?? 0) + v.count);
  for (const e of os) {
    if (e.reexec) continue;
    total++;
    const named = namedBy(e);
    if (named) {
      claimed.add(named);
      traced++;
      if (named.program.toLowerCase() !== e.exe) {
        const kind = "OS shell image of a traced script";
        const starter = `${named.program} (requested program)`;
        const k = `${kind}\u0000${starter}\u0000${e.exe}`;
        rows.set(k, { kind, starter, program: e.exe, count: (rows.get(k)?.count ?? 0) + 1 });
      }
      continue;
    }
    const parent = parentOf(e);
    const starter = starterOf(e, parent);
    if (/^[a-z0-9_]+-[0-9a-f]{16}$/.test(e.exe) && !starter) {
      const k = `test binary (nextest's start)\u0000nextest\u0000${e.exe}`;
      rows.set(k, {
        kind: "test binary (nextest's start)",
        starter: "nextest",
        program: e.exe,
        count: (rows.get(k)?.count ?? 0) + 1,
      });
      continue;
    }
    const starterName = starter ? starter.exe : (parent?.exe ?? "?");
    const key = starter ? `${id(starter.file, starter.pid, starter.instance)}\u0000${e.exe}` : null;
    let kind;
    if (starter && !isTestBinary(starter)) {
      const k = key;
      if ((expected.get(k) ?? 0) > 0) {
        expected.set(k, expected.get(k) - 1);
        traced++;
        continue;
      }
      kind = "MISSED by the trace";
    } else if (starter && (expected.get(key) ?? 0) > 0) {
      // The engine's library, called in the test process, traced as it starts.
      expected.set(key, expected.get(key) - 1);
      traced++;
      continue;
    } else if (starter || /^e2e(-|$)/.test(starterName)) {
      const left = recorded.get(e.exe) ?? 0;
      if (left > 0) recorded.set(e.exe, left - 1);
      kind = left > 0 ? "test process (records)" : "test process, beyond the records";
    } else if (["onepipeline", "onevcs"].includes(starterName) && descendsFromTier(e))
      kind = "MISSED: an engine process with no trace";
    else kind = descendsFromTier(e) ? "uninstrumented descendant" : "unproven OS lineage";
    const k = `${kind}\u0000${starterName}\u0000${e.exe}`;
    rows.set(k, {
      kind,
      starter: starterName,
      program: e.exe,
      count: (rows.get(k)?.count ?? 0) + 1,
    });
  }
  const unseen = new Map();
  for (const list of [...byChild.values(), failed])
    for (const s of list) {
      if (claimed.has(s)) continue;
      const k = keyOf(s);
      expected.set(k, (expected.get(k) ?? 0) + 1);
    }
  for (const [k, n] of expected) {
    if (n <= 0) continue;
    const program = k.split("\u0000")[3];
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

/**
 * Each program's per-start cost in one folded job: the median start-to-exit of its cheapest
 * traced invocation (a program, git subcommand and site with three or more timed starts),
 * the minimal-start median the runner's spawn step measured, its start count, and the
 * minutes those starts cost at that floor (the spawn step's where it measured the program,
 * else the cheapest invocation's), in test-minutes and scaled to the tier's wall minutes.
 */
export function startCosts(f) {
  const cheapest = new Map();
  for (const e of f.bySite) {
    const [prog, site] = e.key.split("\u0000");
    const base = prog.split(" ")[0];
    const m = median(e.durs);
    if (m == null || e.durs.length < 3) continue;
    if (!cheapest.has(base) || m < cheapest.get(base).ns) cheapest.set(base, { ns: m, prog, site });
  }
  const spawn = new Map(
    (f.host?.spawn?.results ?? [])
      .filter((r) => r.median_ms != null)
      .map((r) => [r.name, r.median_ms]),
  );
  const starts = new Map();
  const add = (p, n) => starts.set(p, (starts.get(p) ?? 0) + n);
  for (const e of f.engine) add(e.program.split(" ")[0], e.count);
  for (const e of f.tests) add(e.program, e.count);
  return [...starts.keys()].sort().map((program) => {
    const c = cheapest.get(program) ?? null;
    const spawnMs = spawn.get(program) ?? null;
    const floorMs = spawnMs ?? (c ? c.ns / 1e6 : null);
    const n = starts.get(program);
    const testMin = floorMs == null ? null : (n * floorMs) / 1000 / 60;
    const wallMin = floorMs == null ? null : f.toWallMin(n * floorMs * 1e6);
    return { program, cheapest: c, spawnMs, floorMs, starts: n, testMin, wallMin };
  });
}

function table(head, rows) {
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
          ? `largest nextest run (the e2e tier in this repository's test jobs) \`${f.summary.line}\` = ${(f.summary.seconds / 60).toFixed(1)} min`
          : "no e2e nextest summary in job.log") +
        `; test-name set sha256/16 ${f.testsDigest}.\n`,
    );
    const k = f.skipped;
    out.push(
      f.scope
        ? `Time scope: nextest ${f.scope.id}, with one second of clock slack; excluded ${f.scope.records} trace records (${f.scope.starts} starts) and ${f.scope.os} OS records outside the tier. ${f.scope.otherProfiles} profiles belong to another invocation and remain an aggregate uncertainty.\n`
        : "Time scope unavailable: no completed nextest run matches the profiles; trace totals cover the supplied artifact.\n",
    );
    out.push(
      `Input: ${f.trace.records} trace records read; skipped ${k.malformed} lines that are not JSON, ${k.invalid} records of no known shape, ${k.records} test-profile records not of schema 2 and ${k.waits} harness wait entries; left out ${k.otherBinaries} trace and waits files of tests no e2e record names (other binaries that compile the harness, or the separate version tier).\n`,
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
    out.push(
      table(
        [
          "program",
          "cheapest traced invocation",
          "its median ms",
          "minimal-start median ms (spawn step)",
          "starts",
          "start cost min of test-time (est.)",
          "start cost wall min (est.)",
        ],
        startCosts(f).map((c) => [
          c.program,
          c.cheapest ? `${c.cheapest.prog} @ ${c.cheapest.site}` : "-",
          c.cheapest ? ms(c.cheapest.ns) : "-",
          c.spawnMs == null ? "-" : c.spawnMs.toFixed(1),
          c.starts,
          c.testMin == null ? "-" : c.testMin.toFixed(2),
          c.wallMin == null ? "-" : c.wallMin.toFixed(2),
        ]),
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
      `${f.trace.records} records from ${f.trace.procs} processes: ${sec(f.trace.traceNs)} s building them and ${sec(f.trace.writeNs)} s writing them, ${(per / 1000).toFixed(1)} µs per record (each writer's last write is not in its own total; a process killed before its end contributes only its largest observed writer total).\n`,
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
    const linux = folds.find((f) => f.os === "linux");
    if (linux) {
      out.push(`### Per-start cost against ${linux.label}\n`);
      const floor = new Map(startCosts(linux).map((c) => [c.program, c]));
      const rows = [];
      for (const f of folds) {
        if (f === linux) continue;
        for (const c of startCosts(f)) {
          const l = floor.get(c.program);
          if (c.floorMs == null || l?.floorMs == null) continue;
          const extra = c.floorMs - l.floorMs;
          rows.push([
            f.label,
            c.program,
            c.floorMs.toFixed(1),
            l.floorMs.toFixed(1),
            extra.toFixed(1),
            c.starts,
            ((c.starts * extra) / 1000 / 60).toFixed(2),
            f.toWallMin(c.starts * extra * 1e6)?.toFixed(2) ?? "-",
          ]);
        }
      }
      out.push(
        table(
          [
            "job",
            "program",
            "floor ms",
            `${linux.label} floor ms`,
            "difference ms",
            "starts",
            "difference × starts, test-min (est.)",
            "wall min (est.)",
          ],
          rows,
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
      const t = m ? Date.parse(m[1]) / 1000 : Number.NaN;
      return Number.isFinite(t) ? { t, text: m[2] } : null;
    })
    .filter(Boolean);
  if (!lines.length)
    throw new Refusal(
      "the job log holds no timestamped line",
      "save the log as 'gh api .../actions/jobs/<id>/logs' serves it, timestamps included",
    );
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
  facts.largest = largestRun(log);
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
  if (f.largest)
    out.push(
      `Largest nextest run (the e2e tier in this repository's test jobs): summary ${min(f.largest.seconds)} min, ${f.largest.run} run, ${f.largest.passed} passed; it started ${min(f.largest.start - f.job.start)} min into the job (${f.job.gateStart ? min(f.largest.start - f.job.gateStart) : "-"} min into the gate step, the compile and the tasks before it) and ended ${min(f.job.end - f.largest.end)} min before the job did.`,
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
      // llmlint: ignore-block[changed_behavior_has_e2e] Only a failed macOS sysctl query reaches this fallback; the real hosted macOS host journey proves the successful query, but cannot induce a runner's sysctl failure. This measurement-only diagnostic never lands (manager ruling a1); it records the unanswered query instead of claiming a measured processor value.
      unanswered.push(
        `${cmd} ${args.join(" ")}: ${String(error.stderr || error.message).trim()} (next: read the processor off the runner's own system report in the job log)`,
      );
      return null;
      // llmlint: ignore-end[changed_behavior_has_e2e]
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
  // llmlint: ignore-block[tool_output_is_signal] fold, diff, run and host are report commands: the multi-line Markdown or JSON they print on success is the whole of what they are for, and they never lands: this branch is preserved as the measurement record and nothing on it merges.
  if (verb === "host") {
    if (!(rest.length === 0 || (rest.length === 2 && rest[0] === "--runs")))
      throw usage("host takes nothing but an optional --runs N");
    return `${JSON.stringify(await host({ runs: rest.length ? Number(rest[1]) : 30 }), null, 2)}\n`;
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
