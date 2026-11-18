#!/usr/bin/env node
// Fold the per-test profile records a test run leaves in `target/test-profile/<binary>/`
// into a per-test and per-module breakdown, and each platform's cost per category
// against a baseline platform's.
//
//   node scripts/test-profile-report.mjs --out FILE [--json] [--top-tests N]
//        [--top-modules N] [--top-spans N] [--baseline LABEL] [--run LABEL=RUN_ID]
//        LABEL=DIR [LABEL=DIR ...]
//
// Each LABEL=DIR is one platform's records: a directory searched recursively for the
// records `onepipeline_testfakes::profile` writes, such as a CI leg's downloaded
// artifact. A directory can hold several runs' records — a test that was not run again
// keeps its old one, and a CI leg runs a tier and then a few of its tests again — so
// only one run per platform is read: the one `--run` names, or else the run with the
// most records, the newest of those on a tie. The rest are reported as set aside. A
// test retried within that run is read at its last attempt.
//
// A test run under ONEPIPELINE_TEST_PROFILE_SPANS also leaves, beside its record, a
// `<test>.spans/` directory of the span records each `onepipeline` it started wrote
// (`src/spans.rs` declares their shape). Those of the tests read are folded, one file
// at a time, into a per-module, per-verb section: how many runs, their wall and CPU,
// and the spans that account for them, largest first.
//
// The report (Markdown, or JSON with --json) is written to --out; what is printed is
// one line saying where it went. It exits 0 once the report is written, and 1 with
// the reason and a `next:` line on stderr when anything is refused or fails.
import { readdirSync, readFileSync, writeFileSync } from "node:fs";
import { basename, join } from "node:path";
import { pathToFileURL } from "node:url";

export const SCHEMA = 3;
/**
 * The schemas this reads: 3; 2, which is 3 without each program's `cpu_s` and the
 * record's `nproc` and load; and 1, which is 2 without the `unfinished` ending — so
 * records a leg wrote before either are read as they were written.
 */
export const READS = [1, 2, 3];
/** The span record's schema, `src/spans.rs`'s `SCHEMA`. */
export const SPAN_SCHEMA = 1;
/** The suffix of the directory a test's span records sit in, beside its record. */
export const SPANS_SUFFIX = ".spans";
/** How a record was written; `unfinished` is a test the runner ended before it did. */
export const ENDINGS = ["test-thread", "process-exit", "unfinished"];
export const CATEGORIES = ["spawn", "git", "build", "fixture", "wait", "fs"];

/** A failure the command explains, with what to do about it. */
export class Refusal extends Error {
  constructor(message, next) {
    super(message);
    this.next = next;
  }
}

/** How far a record's figures may disagree before they are not one writer's. */
const SLACK_S = 0.01;
const isSeconds = (value) => Number.isFinite(value) && value >= 0;
const isCount = (value) => Number.isSafeInteger(value) && value >= 0;
/** A figure a platform may not supply, which is then `null` and never `0`. */
const isFigureOrNull = (value) => value === null || isSeconds(value);

/** Why `record` is not a record this reader can fold, or `undefined` when it is one. */
export function recordProblem(record) {
  if (record === null || typeof record !== "object") return "it is not an object";
  if (!READS.includes(record.schema)) {
    return `it is schema ${record.schema}, and this reads ${READS.slice(0, -1).join(", ")} and ${READS.at(-1)}`;
  }
  for (const field of ["binary", "test"]) {
    if (typeof record[field] !== "string" || record[field] === "") return `${field} is not a name`;
  }
  for (const field of ["wall_s", "unattributed_s", "started_at"]) {
    if (!isSeconds(record[field])) return `${field} is not a nonnegative number`;
  }
  if (record.run_id !== null && typeof record.run_id !== "string") return "run_id is not a string";
  const endings = record.schema === 1 ? ENDINGS.filter((e) => e !== "unfinished") : ENDINGS;
  if (!endings.includes(record.ended)) return `ended is not one of ${endings.join(", ")}`;
  if (record.attempt !== null && !isCount(record.attempt)) return "attempt is not a count";
  for (const category of CATEGORIES) {
    const tally = record.categories?.[category];
    if (!isCount(tally?.count)) return `categories.${category}.count is not a count`;
    for (const field of ["s", "inclusive_s", "background_s"]) {
      if (!isSeconds(tally?.[field])) {
        return `categories.${category}.${field} is not a nonnegative number`;
      }
    }
    if (!isCount(tally.background_count)) {
      return `categories.${category}.background_count is not a count`;
    }
  }
  const programs = record.programs;
  if (programs === null || typeof programs !== "object" || Array.isArray(programs)) {
    return "programs is not an object";
  }
  for (const [program, entry] of Object.entries(programs)) {
    if (!isCount(entry?.count) || !isSeconds(entry?.s)) {
      return `programs.${program} is not a count and a number of seconds`;
    }
    if (record.schema >= 3 && !isFigureOrNull(entry.cpu_s)) {
      return `programs.${program}.cpu_s is not a nonnegative number or null`;
    }
  }
  if (record.schema >= 3) {
    if (record.nproc !== null && !(isCount(record.nproc) && record.nproc > 0)) {
      return "nproc is not a positive count or null";
    }
    for (const field of ["load_1m_start", "load_1m_end"]) {
      if (!isFigureOrNull(record[field])) return `${field} is not a nonnegative number or null`;
    }
  }
  if (!isCount(record.categories.wait.polls)) return "categories.wait.polls is not a count";
  // The writer's categories are exclusive and its remainder is what they leave, so a
  // record whose figures do not add up was not written by it. The slack is for the
  // rounding a figure written in decimal seconds carries.
  const attributed = CATEGORIES.reduce((sum, category) => sum + record.categories[category].s, 0);
  if (attributed > record.wall_s + SLACK_S) {
    return `its categories hold ${attributed}s of a ${record.wall_s}s test`;
  }
  if (Math.abs(record.unattributed_s - Math.max(0, record.wall_s - attributed)) > SLACK_S) {
    return `unattributed_s ${record.unattributed_s} is not what its categories leave of wall_s`;
  }
  return undefined;
}

/** The entries of directory `at`, or a refusal naming it and what to do, `next`. */
function entriesOf(at, next) {
  try {
    return readdirSync(at, { withFileTypes: true });
  } catch (error) {
    throw new Refusal(`${at} cannot be read: ${error.message}`, next);
  }
}

/**
 * Every record under `dir`, however deep. A file a writer is still staging (a name
 * starting with `.`) is passed over; any other `.json` that is not a record is refused
 * by name, because a record read wrong is a report that is wrong.
 */
export function loadRecords(dir) {
  const records = [];
  const walk = (at) => {
    for (const entry of entriesOf(
      at,
      "name a directory of records: target/test-profile/e2e after an e2e run, or a CI leg's unpacked test-profile-* artifact",
    )) {
      const path = join(at, entry.name);
      // A test's span records are not test records; `foldSpans` reads them.
      if (entry.isDirectory() && entry.name.endsWith(SPANS_SUFFIX)) continue;
      if (entry.isDirectory()) walk(path);
      else if (entry.isFile() && entry.name.endsWith(".json") && !entry.name.startsWith(".")) {
        let record;
        try {
          record = JSON.parse(readFileSync(path, "utf8"));
        } catch (error) {
          throw new Refusal(
            `${path} is not JSON: ${error.message}`,
            `remove ${path}, or rerun the test that wrote it`,
          );
        }
        const problem = recordProblem(record);
        if (problem) {
          throw new Refusal(
            `${path} is not a test-profile record: ${problem}`,
            `remove ${path}, or rerun the test that wrote it`,
          );
        }
        records.push(record);
      }
    }
  };
  walk(dir);
  return records;
}

/**
 * One run's records, each test at its last attempt. `runId` picks the run; without one
 * it is the run with the most records, the newest of those on a tie. Returns the run, its records, and how
 * many records of other runs were set aside.
 */
export function selectRun(records, runId) {
  const runs = new Map();
  for (const record of records) {
    const id = record.run_id ?? "(no run id)";
    const run = runs.get(id) ?? { id, latest: -Infinity, records: [] };
    run.latest = Math.max(run.latest, record.started_at ?? 0);
    run.records.push(record);
    runs.set(id, run);
  }
  let chosen;
  if (runId !== undefined) {
    chosen = runs.get(runId);
    if (!chosen) {
      throw new Refusal(
        `no record belongs to run ${runId}`,
        `name one of the runs these records hold: ${[...runs.keys()].join(", ") || "(none)"}`,
      );
    }
  } else {
    chosen = [...runs.values()].sort(
      (a, b) => b.records.length - a.records.length || b.latest - a.latest,
    )[0];
  }
  if (!chosen) return { run: null, records: [], setAside: 0 };
  const last = new Map();
  for (const record of chosen.records) {
    const key = `${record.binary}\u0000${record.test}`;
    const held = last.get(key);
    if (!held || (record.attempt ?? 0) >= (held.attempt ?? 0)) last.set(key, record);
  }
  return {
    run: chosen.id,
    records: [...last.values()],
    setAside: records.length - chosen.records.length,
  };
}

/** The module a test belongs to: the first segment of its path in the test binary. */
export function moduleOf(test) {
  const at = test.indexOf("::");
  return at === -1 ? "(crate root)" : test.slice(0, at);
}

function seconds(record, category) {
  return record.categories?.[category]?.s ?? 0;
}

function counts(record, category) {
  return record.categories?.[category]?.count ?? 0;
}

/** The totals of a set of records: wall, each category, and what none of them holds. */
export function totals(records) {
  const total = {
    tests: records.length,
    unfinished: records.filter((record) => record.ended === "unfinished").length,
    wall: 0,
    unattributed: 0,
    categories: {},
    counts: {},
    // The children's CPU of the programs the tests ran, over the records that carry
    // one; `null` where none does, which is not none spent.
    cpu: null,
    // The mean 1-minute load at the tests' start and end, over the records carrying it.
    load: { start: null, end: null },
  };
  const loads = { start: [], end: [] };
  for (const category of CATEGORIES) {
    total.categories[category] = 0;
    total.counts[category] = 0;
  }
  total.counts.polls = 0;
  for (const record of records) {
    total.wall += record.wall_s ?? 0;
    total.unattributed += record.unattributed_s ?? 0;
    for (const category of CATEGORIES) {
      total.categories[category] += seconds(record, category);
      total.counts[category] += counts(record, category);
    }
    total.counts.polls += record.categories?.wait?.polls ?? 0;
    for (const entry of Object.values(record.programs ?? {})) {
      if (isSeconds(entry.cpu_s)) total.cpu = (total.cpu ?? 0) + entry.cpu_s;
    }
    if (isSeconds(record.load_1m_start)) loads.start.push(record.load_1m_start);
    if (isSeconds(record.load_1m_end)) loads.end.push(record.load_1m_end);
  }
  const mean = (values) =>
    values.length ? values.reduce((sum, value) => sum + value, 0) / values.length : null;
  total.load = { start: mean(loads.start), end: mean(loads.end) };
  return total;
}

/** One platform's breakdown: totals, the costliest tests, and the costliest modules. */
export function breakdown(records, { topTests = 30, topModules = 10 } = {}) {
  const tests = records
    .map((record) => ({ name: `${record.binary}::${record.test}`, ...totals([record]) }))
    .sort((a, b) => b.wall - a.wall);
  const byModule = new Map();
  for (const record of records) {
    const name = `${record.binary}::${moduleOf(record.test)}`;
    byModule.set(name, [...(byModule.get(name) ?? []), record]);
  }
  const modules = [...byModule]
    .map(([name, held]) => ({ name, ...totals(held) }))
    .sort((a, b) => b.wall - a.wall);
  return {
    total: totals(records),
    tests: tests.slice(0, topTests),
    modules: modules.slice(0, topModules),
    allModules: modules,
  };
}

/**
 * Per category, what `platform` spent against what `baseline` spent, over the tests
 * both ran: the ratio of the two sums, so a category one platform barely uses cannot
 * read as a large ratio of two medians of nothing.
 */
export function ratios(platform, baseline) {
  const key = (record) => `${record.binary}\u0000${record.test}`;
  const theirs = new Map(baseline.map((record) => [key(record), record]));
  const mine = platform.filter((record) => theirs.has(key(record)));
  const a = totals(mine);
  const b = totals(mine.map((record) => theirs.get(key(record))));
  const ratio = (x, y) => (y > 0 ? x / y : null);
  const out = { tests: mine.length, wall: ratio(a.wall, b.wall), categories: {} };
  out.unattributed = ratio(a.unattributed, b.unattributed);
  for (const category of CATEGORIES) {
    out.categories[category] = ratio(a.categories[category], b.categories[category]);
  }
  return out;
}

/** How far a span record's figures may disagree before they are not one writer's. */
const SLACK_MS = 0.01;

/** Why `record` is not a span record this reader can fold, or `undefined` when it is one. */
export function spanRecordProblem(record) {
  if (record === null || typeof record !== "object") return "it is not an object";
  if (record.schema !== SPAN_SCHEMA) {
    return `it is span schema ${record.schema}, and this reads ${SPAN_SCHEMA}`;
  }
  if (typeof record.verb !== "string") return "verb is not a string";
  if (!isCount(record.pid)) return "pid is not a count";
  for (const field of ["started_at", "wall_ms"]) {
    if (!isSeconds(record[field])) return `${field} is not a nonnegative number`;
  }
  for (const field of [
    "user_cpu_ms",
    "system_cpu_ms",
    "children_user_cpu_ms",
    "children_system_cpu_ms",
    "peak_rss_kb",
  ]) {
    if (!isFigureOrNull(record[field])) return `${field} is not a nonnegative number or null`;
  }
  if (!Number.isSafeInteger(record.exit_status)) return "exit_status is not an integer";
  if (!Array.isArray(record.spans)) return "spans is not a list";
  let selfWall = 0;
  for (const span of record.spans) {
    if (typeof span?.name !== "string" || span.name === "") return "a span has no name";
    if (!isCount(span.count)) return `spans.${span.name}.count is not a count`;
    for (const field of ["self_wall_ms", "wall_ms"]) {
      if (!isSeconds(span[field])) return `spans.${span.name}.${field} is not a nonnegative number`;
    }
    if (!isFigureOrNull(span.self_cpu_ms)) {
      return `spans.${span.name}.self_cpu_ms is not a nonnegative number or null`;
    }
    // A span's own time is the part of its whole that nothing nested in it took.
    if (span.self_wall_ms > span.wall_ms + SLACK_MS) {
      return `spans.${span.name} spent ${span.self_wall_ms}ms of its own in ${span.wall_ms}ms`;
    }
    selfWall += span.self_wall_ms;
  }
  // The writer's spans are one thread's and exclusive, so their own times fit the process.
  if (selfWall > record.wall_ms + SLACK_MS) {
    return `its spans hold ${selfWall}ms of a ${record.wall_ms}ms process`;
  }
  return undefined;
}

/** `a + b` where either may be a figure a platform did not supply. */
const plus = (a, b) => (a === null ? b : b === null ? a : a + b);
const cpuOf = (user, system) => (user === null || system === null ? null : user + system);

function spanFold(names, span) {
  const held = names.get(span.name) ?? {
    name: span.name,
    count: 0,
    self_wall_ms: 0,
    wall_ms: 0,
    self_cpu_ms: null,
  };
  held.count += span.count;
  held.self_wall_ms += span.self_wall_ms;
  held.wall_ms += span.wall_ms;
  held.self_cpu_ms = plus(held.self_cpu_ms, span.self_cpu_ms);
  names.set(span.name, held);
}

function readSpanRecord(path) {
  let record;
  try {
    record = JSON.parse(readFileSync(path, "utf8"));
  } catch (error) {
    throw new Refusal(
      `${path} is not JSON: ${error.message}`,
      `remove ${path}, or rerun the test whose directory holds it`,
    );
  }
  const problem = spanRecordProblem(record);
  if (problem) {
    throw new Refusal(
      `${path} is not a span record: ${problem}`,
      `remove ${path}, or rerun the test whose directory holds it`,
    );
  }
  return record;
}

/**
 * Fold the span records of the tests in `records` — each test's `<test>.spans/`
 * directory beside its record, anywhere under `dir` — into each module's `onepipeline`
 * runs, per verb. One file is read at a time, and only the per-verb and per-span totals
 * are kept, so a module's thousands of runs cost one record's memory. A span record
 * older than its test's record was left by an earlier run of that test and is set aside.
 * CPU sums and means are over the records that carry the figure, and `null` where none does.
 */
export function foldSpans(dir, records, { topSpans = 10 } = {}) {
  const tests = new Map(
    records.map((record) => [`${record.binary}\u0000${record.test.replaceAll("::", ".")}`, record]),
  );
  const modules = new Map();
  let read = 0;
  let stale = 0;
  const walk = (at) => {
    // Read again: a directory `loadRecords` read can have gone since.
    for (const entry of entriesOf(
      at,
      "rerun the report once the run writing these records has finished",
    )) {
      if (!entry.isDirectory()) continue;
      const path = join(at, entry.name);
      if (!entry.name.endsWith(SPANS_SUFFIX)) {
        walk(path);
        continue;
      }
      const test = tests.get(`${basename(at)}\u0000${entry.name.slice(0, -SPANS_SUFFIX.length)}`);
      if (!test) continue;
      const moduleName = `${test.binary}::${moduleOf(test.test)}`;
      const module = modules.get(moduleName) ?? { name: moduleName, verbs: new Map() };
      modules.set(moduleName, module);
      for (const file of entriesOf(
        path,
        `make ${path} readable, or remove it and rerun the test whose span records it holds`,
      )) {
        if (!file.isFile() || !file.name.endsWith(".json") || file.name.startsWith(".")) continue;
        const span = readSpanRecord(join(path, file.name));
        if (span.started_at + SLACK_S < test.started_at) {
          stale += 1;
          continue;
        }
        read += 1;
        const verb = module.verbs.get(span.verb) ?? {
          verb: span.verb,
          invocations: 0,
          wall_ms: 0,
          cpu_ms: null,
          // How many runs carried a CPU figure, which is what a mean of it is over.
          cpu_runs: 0,
          children_cpu_ms: null,
          spans: new Map(),
        };
        verb.invocations += 1;
        verb.wall_ms += span.wall_ms;
        const cpu = cpuOf(span.user_cpu_ms, span.system_cpu_ms);
        if (cpu !== null) verb.cpu_runs += 1;
        verb.cpu_ms = plus(verb.cpu_ms, cpu);
        verb.children_cpu_ms = plus(
          verb.children_cpu_ms,
          cpuOf(span.children_user_cpu_ms, span.children_system_cpu_ms),
        );
        for (const named of span.spans) spanFold(verb.spans, named);
        module.verbs.set(span.verb, verb);
      }
    }
  };
  walk(dir);
  const byWall = (a, b) => b.wall_ms - a.wall_ms;
  const folded = [...modules.values()]
    .map((module) => {
      const verbs = [...module.verbs.values()]
        .map(({ spans, ...verb }) => ({
          ...verb,
          mean_wall_ms: verb.wall_ms / verb.invocations,
          mean_cpu_ms: verb.cpu_ms === null ? null : verb.cpu_ms / verb.cpu_runs,
          span_names: spans.size,
          spans: [...spans.values()]
            .sort((a, b) => b.self_wall_ms - a.self_wall_ms)
            .slice(0, topSpans),
        }))
        .sort(byWall);
      return {
        name: module.name,
        invocations: verbs.reduce((sum, verb) => sum + verb.invocations, 0),
        wall_ms: verbs.reduce((sum, verb) => sum + verb.wall_ms, 0),
        cpu_ms: verbs.reduce((sum, verb) => plus(sum, verb.cpu_ms), null),
        verbs,
      };
    })
    .filter((module) => module.invocations > 0)
    .sort(byWall);
  return { records: read, stale, modules: folded };
}

function renderSpans(lines, spans) {
  if (spans.records === 0) return;
  const s = (ms) => (ms === null ? "-" : (ms / 1000).toFixed(2));
  const ms = (value) => (value === null ? "-" : value.toFixed(1));
  lines.push(
    "### Where the `onepipeline` runs went",
    "",
    `${spans.records} span records of the tests read` +
      (spans.stale ? ` (${spans.stale} of an earlier run of those tests set aside)` : "") +
      ". Each module's runs per verb, and each verb's spans by their own time, largest first.",
    "",
  );
  for (const module of spans.modules) {
    lines.push(
      `#### ${module.name}: ${module.invocations} runs, ${s(module.wall_ms)} wall s, ${s(module.cpu_ms)} cpu s`,
      "",
      "| verb | runs | wall s | mean wall ms | cpu s | mean cpu ms | child cpu s |",
      "| --- | ---: | ---: | ---: | ---: | ---: | ---: |",
    );
    for (const verb of module.verbs) {
      lines.push(
        `| ${verb.verb || "(none)"} | ${verb.invocations} | ${s(verb.wall_ms)} | ${ms(verb.mean_wall_ms)} | ${s(verb.cpu_ms)} | ${ms(verb.mean_cpu_ms)} | ${s(verb.children_cpu_ms)} |`,
      );
    }
    lines.push("");
    for (const verb of module.verbs) {
      lines.push(
        `\`${verb.verb || "(none)"}\`, ${verb.spans.length} of its ${verb.span_names} spans:`,
        "",
        "| span | count | self wall s | of the verb's wall | wall s | self cpu s |",
        "| --- | ---: | ---: | ---: | ---: | ---: |",
      );
      for (const span of verb.spans) {
        lines.push(
          `| ${span.name} | ${span.count} | ${s(span.self_wall_ms)} | ${pct(span.self_wall_ms, verb.wall_ms)} | ${s(span.wall_ms)} | ${s(span.self_cpu_ms)} |`,
        );
      }
      lines.push("");
    }
  }
}

function pct(part, whole) {
  return whole > 0 ? `${((100 * part) / whole).toFixed(1)}%` : "-";
}

const figure = (value, digits = 1) => (value === null ? "-" : value.toFixed(digits));

function shareRow(name, total) {
  const cells = CATEGORIES.map((category) => pct(total.categories[category], total.wall));
  const load = `${figure(total.load.start, 2)} → ${figure(total.load.end, 2)}`;
  return `| ${name} | ${total.tests} | ${total.wall.toFixed(1)} | ${figure(total.cpu)} | ${load} | ${cells.join(" | ")} | ${pct(total.unattributed, total.wall)} |`;
}

const HEADER = `| | tests | wall s | child cpu s | load 1m | ${CATEGORIES.join(" | ")} | unattributed |\n| --- | ---: | ---: | ---: | ---: | ${CATEGORIES.map(() => "---:").join(" | ")} | ---: |`;

/** The report as Markdown. */
export function render(report) {
  const lines = [];
  for (const platform of report.platforms) {
    const { label, run, setAside, breakdown: b } = platform;
    lines.push(`## ${label}`, "");
    if (run === null) {
      lines.push("No records.", "");
      continue;
    }
    lines.push(
      `Run \`${run}\`: ${b.total.tests} tests, ${b.total.wall.toFixed(1)} test-s` +
        (setAside ? ` (${setAside} records of other runs set aside)` : "") +
        (b.total.unfinished
          ? `, ${b.total.unfinished} of them unfinished (ended by the runner; their figures are their last heartbeat's)`
          : "") +
        `. Counts: ${CATEGORIES.map((c) => `${c} ${b.total.counts[c]}`).join(", ")}, polls ${b.total.counts.polls}.`,
      "",
    );
    lines.push("### Whole run", "", HEADER, shareRow("all", b.total), "");
    lines.push(`### The ${b.tests.length} costliest tests`, "", HEADER);
    for (const test of b.tests) lines.push(shareRow(test.name, test));
    lines.push("", `### The ${b.modules.length} costliest modules`, "", HEADER);
    for (const module of b.modules) lines.push(shareRow(module.name, module));
    lines.push("");
    renderSpans(lines, platform.spans);
  }
  lines.push(`## Against ${report.baseline}`, "");
  if (report.missingBaseline) {
    lines.push(`No records for ${report.baseline}, so no platform is compared with it.`, "");
  } else if (report.ratios.length === 0) {
    lines.push(`No other platform's records were given, so none is compared with it.`, "");
  } else {
    lines.push(
      `Each figure is the platform's seconds in that category over ${report.baseline}'s, summed over the tests both ran.`,
      "",
      `| platform | tests both ran | wall | ${CATEGORIES.join(" | ")} | unattributed |`,
      `| --- | ---: | ---: | ${CATEGORIES.map(() => "---:").join(" | ")} | ---: |`,
    );
    const x = (value) => (value === null ? "-" : `${value.toFixed(2)}x`);
    for (const { label, ratios: r } of report.ratios) {
      const cells = CATEGORIES.map((category) => x(r.categories[category]));
      lines.push(
        `| ${label} | ${r.tests} | ${x(r.wall)} | ${cells.join(" | ")} | ${x(r.unattributed)} |`,
      );
    }
    lines.push("");
  }
  return lines.join("\n");
}

/** Read every platform's records and build the report. */
export function buildReport(
  platforms,
  { baseline, runs = new Map(), topTests, topModules, topSpans },
) {
  const read = platforms.map(({ label, dir }) => {
    const { run, records, setAside } = selectRun(loadRecords(dir), runs.get(label));
    return {
      label,
      run,
      setAside,
      records,
      breakdown: breakdown(records, { topTests, topModules }),
      spans: foldSpans(dir, records, { topSpans }),
    };
  });
  const base = read.find((platform) => platform.label === baseline);
  const report = {
    baseline,
    missingBaseline: !base,
    platforms: read.map(({ records: _records, ...rest }) => rest),
    ratios: [],
  };
  if (base) {
    for (const platform of read) {
      if (platform !== base) {
        report.ratios.push({
          label: platform.label,
          ratios: ratios(platform.records, base.records),
        });
      }
    }
  }
  return report;
}

const USAGE =
  "node scripts/test-profile-report.mjs --out report.md linux=target/test-profile/e2e [windows=DIR ...]";
const LABEL = /^[A-Za-z0-9._-]+$/;

/** `LABEL=VALUE`, both halves present and the label a plain name. */
function pair(arg, what) {
  const at = arg.indexOf("=");
  const label = arg.slice(0, at);
  const value = arg.slice(at + 1);
  if (at <= 0 || value === "" || !LABEL.test(label)) {
    throw new Refusal(`${arg} is not ${what}`, `write it as LABEL=VALUE, e.g. ${USAGE}`);
  }
  return [label, value];
}

function count(arg, value) {
  const n = Number(value);
  if (!/^[0-9]+$/.test(value) || !Number.isSafeInteger(n)) {
    throw new Refusal(`${arg} ${value} is not a whole number`, `give ${arg} a count such as 30`);
  }
  return n;
}

export function parseArgs(argv) {
  const options = {
    baseline: "linux",
    runs: new Map(),
    topTests: 30,
    topModules: 10,
    topSpans: 10,
    json: false,
  };
  const platforms = [];
  for (let i = 0; i < argv.length; i++) {
    const arg = argv[i];
    const value = () => {
      if (i + 1 >= argv.length) throw new Refusal(`${arg} needs a value`, USAGE);
      return argv[++i];
    };
    if (arg === "--json") options.json = true;
    else if (arg === "--out") options.out = value();
    else if (arg === "--baseline") options.baseline = value();
    else if (arg === "--top-tests") options.topTests = count(arg, value());
    else if (arg === "--top-modules") options.topModules = count(arg, value());
    else if (arg === "--top-spans") options.topSpans = count(arg, value());
    else if (arg === "--run") {
      const [label, id] = pair(value(), "a LABEL=RUN_ID");
      options.runs.set(label, id);
    } else if (!arg.startsWith("-") && arg.includes("=")) {
      const [label, dir] = pair(arg, "a LABEL=DIR");
      if (platforms.some((platform) => platform.label === label)) {
        throw new Refusal(`${label} names two directories`, "give each platform one LABEL=DIR");
      }
      platforms.push({ label, dir });
    } else throw new Refusal(`${arg} is not an option or a LABEL=DIR`, USAGE);
  }
  if (platforms.length === 0) throw new Refusal("no LABEL=DIR of records was named", USAGE);
  if (!options.out) throw new Refusal("--out names no file for the report", USAGE);
  for (const label of options.runs.keys()) {
    if (!platforms.some((platform) => platform.label === label)) {
      throw new Refusal(
        `--run names ${label}, which no LABEL=DIR does`,
        "give --run a label the command also reads",
      );
    }
  }
  return { options, platforms };
}

function main(argv) {
  const { options, platforms } = parseArgs(argv);
  const report = buildReport(platforms, options);
  try {
    writeFileSync(
      options.out,
      options.json ? `${JSON.stringify(report, null, 2)}\n` : `${render(report)}\n`,
    );
  } catch (error) {
    throw new Refusal(
      `--out ${options.out} cannot be written: ${error.message}`,
      "point --out at a file in a directory that exists and that you can write to",
    );
  }
  const tests = report.platforms.map((p) => `${p.label} ${p.breakdown.total.tests}`).join(", ");
  console.log(`test-profile-report: wrote ${options.out} (tests read: ${tests})`);
}

if (import.meta.url === pathToFileURL(process.argv[1]).href) {
  try {
    main(process.argv.slice(2));
  } catch (error) {
    console.error(`test-profile-report: ${error.message}`);
    console.error(`next: ${error instanceof Refusal ? error.next : USAGE}`);
    process.exitCode = 1;
  }
}
