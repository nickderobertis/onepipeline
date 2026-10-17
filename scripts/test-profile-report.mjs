#!/usr/bin/env node
// Fold the per-test profile records a test run leaves in `target/test-profile/<binary>/`
// into a per-test and per-module breakdown, and each platform's cost per category
// against a baseline platform's.
//
//   node scripts/test-profile-report.mjs --out FILE [--json] [--top-tests N]
//        [--top-modules N] [--baseline LABEL] [--run LABEL=RUN_ID] LABEL=DIR [LABEL=DIR ...]
//
// Each LABEL=DIR is one platform's records: a directory searched recursively for the
// records `onepipeline_testfakes::profile` writes, such as a CI leg's downloaded
// artifact. A directory can hold several runs' records — a test that was not run again
// keeps its old one, and a CI leg runs a tier and then a few of its tests again — so
// only one run per platform is read: the one `--run` names, or else the run with the
// most records, the newest of those on a tie. The rest are reported as set aside. A
// test retried within that run is read at its last attempt.
//
// The report (Markdown, or JSON with --json) is written to --out; what is printed is
// one line saying where it went. It exits 0 once the report is written, and 1 with
// the reason and a `next:` line on stderr when anything is refused or fails.
import { readdirSync, readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { pathToFileURL } from "node:url";

export const SCHEMA = 2;
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

/** Why `record` is not a record this reader can fold, or `undefined` when it is one. */
export function recordProblem(record) {
  if (record === null || typeof record !== "object") return "it is not an object";
  if (record.schema !== SCHEMA) return `it is schema ${record.schema}, and this reads ${SCHEMA}`;
  for (const field of ["binary", "test"]) {
    if (typeof record[field] !== "string" || record[field] === "") return `${field} is not a name`;
  }
  for (const field of ["wall_s", "unattributed_s", "started_at"]) {
    if (!isSeconds(record[field])) return `${field} is not a nonnegative number`;
  }
  if (record.run_id !== null && typeof record.run_id !== "string") return "run_id is not a string";
  if (!ENDINGS.includes(record.ended)) return `ended is not one of ${ENDINGS.join(", ")}`;
  if (record.attempt !== null && !isCount(record.attempt)) return "attempt is not a count";
  for (const category of CATEGORIES) {
    const tally = record.categories?.[category];
    if (!isCount(tally?.count)) return `categories.${category}.count is not a count`;
    if (!isSeconds(tally?.s)) return `categories.${category}.s is not a nonnegative number`;
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

/**
 * Every record under `dir`, however deep. A file a writer is still staging (a name
 * starting with `.`) is passed over; any other `.json` that is not a record is refused
 * by name, because a record read wrong is a report that is wrong.
 */
export function loadRecords(dir) {
  const records = [];
  const walk = (at) => {
    let entries;
    try {
      entries = readdirSync(at, { withFileTypes: true });
    } catch (error) {
      throw new Refusal(
        `${at} cannot be read: ${error.message}`,
        "name a directory of records: target/test-profile/e2e after an e2e run, or a CI leg's unpacked test-profile-* artifact",
      );
    }
    for (const entry of entries) {
      const path = join(at, entry.name);
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
  };
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
  }
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

function pct(part, whole) {
  return whole > 0 ? `${((100 * part) / whole).toFixed(1)}%` : "-";
}

function shareRow(name, total) {
  const cells = CATEGORIES.map((category) => pct(total.categories[category], total.wall));
  return `| ${name} | ${total.tests} | ${total.wall.toFixed(1)} | ${cells.join(" | ")} | ${pct(total.unattributed, total.wall)} |`;
}

const HEADER = `| | tests | wall s | ${CATEGORIES.join(" | ")} | unattributed |\n| --- | ---: | ---: | ${CATEGORIES.map(() => "---:").join(" | ")} | ---: |`;

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
export function buildReport(platforms, { baseline, runs = new Map(), topTests, topModules }) {
  const read = platforms.map(({ label, dir }) => {
    const { run, records, setAside } = selectRun(loadRecords(dir), runs.get(label));
    return {
      label,
      run,
      setAside,
      records,
      breakdown: breakdown(records, { topTests, topModules }),
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
