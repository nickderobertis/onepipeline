#!/usr/bin/env node
// Fold the per-test profile records a test run leaves in `target/test-profile/<binary>/`
// into a per-test and per-module breakdown, and each platform's cost per category
// against a baseline platform's.
//
//   node scripts/test-profile-report.mjs [--top-tests N] [--top-modules N] [--baseline LABEL]
//        [--run LABEL=RUN_ID] [--json] LABEL=DIR [LABEL=DIR ...]
//
// Each LABEL=DIR is one platform's records: a directory searched recursively for the
// records `onepipeline_testfakes::profile` writes, such as a CI leg's downloaded
// artifact. A directory can hold several runs' records — a test that was not run again
// keeps its old one, and a CI leg runs a tier and then a few of its tests again — so
// only one run per platform is read: the one `--run` names, or else the run with the
// most records, the newest of those on a tie. The rest are reported as set aside. A test retried within that run is read at its last attempt.
import { readdirSync, readFileSync } from "node:fs";
import { join } from "node:path";
import { pathToFileURL } from "node:url";

export const SCHEMA = 1;
export const CATEGORIES = ["spawn", "git", "build", "fixture", "wait", "fs"];

/** Every record under `dir`, however deep; files that are not records are skipped. */
export function loadRecords(dir) {
  const records = [];
  const walk = (at) => {
    for (const entry of readdirSync(at, { withFileTypes: true })) {
      const path = join(at, entry.name);
      if (entry.isDirectory()) walk(path);
      else if (entry.isFile() && entry.name.endsWith(".json") && !entry.name.startsWith(".")) {
        let record;
        try {
          record = JSON.parse(readFileSync(path, "utf8"));
        } catch {
          continue;
        }
        if (record?.schema === SCHEMA && typeof record.test === "string") records.push(record);
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
    if (!chosen) throw new Error(`no record belongs to run ${runId}`);
  } else {
    chosen = [...runs.values()].sort(
      (a, b) => b.records.length - a.records.length || b.latest - a.latest,
    )[0];
  }
  if (!chosen) return { run: undefined, records: [], setAside: 0 };
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
  const total = { tests: records.length, wall: 0, unattributed: 0, categories: {}, counts: {} };
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
    lines.push(
      `Run \`${run}\`: ${b.total.tests} tests, ${b.total.wall.toFixed(1)} test-s` +
        (setAside ? ` (${setAside} records of other runs set aside)` : "") +
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
export function buildReport(platforms, { baseline, runs = {}, topTests, topModules }) {
  const read = platforms.map(({ label, dir }) => {
    const { run, records, setAside } = selectRun(loadRecords(dir), runs[label]);
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

function main(argv) {
  const options = { baseline: "linux", runs: {}, topTests: 30, topModules: 10, json: false };
  const platforms = [];
  for (let i = 0; i < argv.length; i++) {
    const arg = argv[i];
    const value = () => {
      if (i + 1 >= argv.length) throw new Error(`${arg} needs a value`);
      return argv[++i];
    };
    if (arg === "--json") options.json = true;
    else if (arg === "--baseline") options.baseline = value();
    else if (arg === "--top-tests") options.topTests = Number(value());
    else if (arg === "--top-modules") options.topModules = Number(value());
    else if (arg === "--run") {
      const [label, id] = value().split("=");
      options.runs[label] = id;
    } else if (arg.includes("=")) {
      const at = arg.indexOf("=");
      platforms.push({ label: arg.slice(0, at), dir: arg.slice(at + 1) });
    } else throw new Error(`not an option or a LABEL=DIR: ${arg}`);
  }
  if (platforms.length === 0) throw new Error("name at least one LABEL=DIR of records");
  const report = buildReport(platforms, options);
  process.stdout.write(
    options.json ? `${JSON.stringify(report, null, 2)}\n` : `${render(report)}\n`,
  );
}

if (import.meta.url === pathToFileURL(process.argv[1]).href) {
  try {
    main(process.argv.slice(2));
  } catch (error) {
    console.error(`test-profile-report: ${error.message}`);
    process.exitCode = 1;
  }
}
