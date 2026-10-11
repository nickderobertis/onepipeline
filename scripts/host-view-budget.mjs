#!/usr/bin/env node
// Report the record `scripts/host-view-measure.mjs` leaves; this command never runs
// `onepipeline host`.
//
// The budget is one read: each of the steady single reads is judged against the
// threshold, so the value reported is the slowest of them. Their median, the phase
// breakdown, the cold read and the 100/400/800-root scaling reads go in the detail
// as evidence, never as the verdict.
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { report } from "@onebudgetspec/sdk";

const record = "target/budget-records/host-view.json";
// What the budget's workload states, and so what a record has to have measured.
export const READS = 5;
/**
 * The breakdown of one read, as the record names each figure, and the phase of
 * `src/hostview.rs`, `src/lib.rs` and `src/driver.rs` it is read from: the one list
 * `scripts/host-view-measure.mjs` writes a breakdown by and this command reads it by.
 */
export const BREAKDOWN = [
  ["event_registry_ms", "event-registry"],
  ["list_ms", "list"],
  ["launch_and_summary_ms", "launch-and-summary"],
  ["registry_ms", "registry"],
  ["pid_proofs_ms", "pid-proofs"],
  ["decide_ended_ms", "decide-ended"],
  ["live_fold_ms", "live-fold"],
  ["cold_fold_ms", "cold-fold"],
  ["free_space_ms", "free-space"],
  ["render_ms", "render"],
];
/** The figures a breakdown carries beside its phases. */
export const MEASURED = [
  "process_start_ms",
  "exit_ms",
  "live_runs_folded",
  "live_journal_bytes",
  "cold_journal_bytes",
  "read_bytes",
];
const FIXTURE = { run_roots: 100, live_dispatches: 5, stale_entries: 4 };
let next =
  "run 'just host-view-budget BINARY' against an installed release build, which measures and checks it";

function fail(message) {
  throw new Error(`unreadable: ${message}`);
}

function milliseconds(value, name) {
  if (typeof value !== "number" || !Number.isFinite(value) || value < 0) {
    fail(`${name} must be a nonnegative number of milliseconds`);
  }
  return value;
}

function reads(value, name) {
  if (!Array.isArray(value) || value.length === 0) {
    fail(`${name} must be a nonempty array of reads`);
  }
  return value.map((read, n) => milliseconds(read, `${name}[${n}]`));
}

function readRecord() {
  let text;
  try {
    text = readFileSync(record, "utf8");
  } catch (error) {
    throw new Error(error.code === "ENOENT" ? "missing" : `unreadable: ${error.message}`);
  }
  let held;
  try {
    held = JSON.parse(text);
  } catch (error) {
    fail(error.message);
  }
  return validateRecord(held);
}

/** Refuse a record this command cannot vouch for; answer it otherwise. */
export function validateRecord(held) {
  if (held === null || typeof held !== "object" || held.schema_version !== 1) {
    fail("schema_version must be 1");
  }
  if (held.steady?.build !== "release") {
    fail("steady.build must be release: the budget times the binary a user runs");
  }
  if (reads(held.steady.reads_ms, "steady.reads_ms").length !== READS) {
    fail(`steady.reads_ms must hold the ${READS} single reads the budget judges`);
  }
  for (const [key, value] of Object.entries(FIXTURE)) {
    if (held.runs_root?.[key] !== value) {
      fail(`runs_root.${key} must be ${value}: the budget's workload is that fixture`);
    }
  }
  if (held.steady.ended_journals_unreadable !== true) {
    fail(
      "steady.ended_journals_unreadable must be true: the budget's reads are of a root whose ended journals they cannot open",
    );
  }
  validateDetail(held);
  return held;
}

const isObject = (value) => value !== null && typeof value === "object" && !Array.isArray(value);

function count(value, name) {
  if (!Number.isSafeInteger(value) || value < 0) {
    fail(`${name} must be a nonnegative integer`);
  }
}

// Every figure a breakdown, a median or a load carries is a nonnegative number, or
// null where the read did not measure it.
function figure(value, name) {
  if (
    value !== undefined &&
    value !== null &&
    (typeof value !== "number" || !Number.isFinite(value) || value < 0)
  ) {
    fail(`${name} must be a nonnegative number or null`);
  }
}

function breakdownOf(value, name) {
  if (!isObject(value)) fail(`${name} must be a breakdown`);
  for (const key of [...BREAKDOWN.map(([field]) => field), ...MEASURED]) {
    if (!(key in value))
      fail(`${name}.${key} is missing: the breakdown carries every figure, null where unmeasured`);
    figure(value[key], `${name}.${key}`);
  }
}

// Every part of the detail is one the measurement writes, in the shape the detail
// reads it as.
function validateDetail(held) {
  breakdownOf(held.steady.breakdown, "steady.breakdown");
  figure(held.steady.median_ms, "steady.median_ms");
  figure(held.steady.load1, "steady.load1");
  if (!isObject(held.cold)) fail("cold must be the cold read");
  milliseconds(held.cold.read_ms, "cold.read_ms");
  breakdownOf(held.cold.breakdown, "cold.breakdown");
  if (!isObject(held.scaling)) fail("scaling must be the scaling reads");
  if (!Array.isArray(held.scaling.sizes) || held.scaling.sizes.length === 0)
    fail("scaling.sizes must be a nonempty array");
  held.scaling.sizes.forEach((size, n) => {
    if (!isObject(size)) fail(`scaling.sizes[${n}] must be an object`);
    count(size.run_roots, `scaling.sizes[${n}].run_roots`);
    milliseconds(size.median_ms, `scaling.sizes[${n}].median_ms`);
  });
  if (!Array.isArray(held.scaling.increments)) fail("scaling.increments must be an array");
  held.scaling.increments.forEach((step, n) => {
    if (!isObject(step)) fail(`scaling.increments[${n}] must be an object`);
    count(step.from, `scaling.increments[${n}].from`);
    count(step.to, `scaling.increments[${n}].to`);
    if (typeof step.ms_per_ended_run !== "number" || !Number.isFinite(step.ms_per_ended_run)) {
      fail(`scaling.increments[${n}].ms_per_ended_run must be a finite number`);
    }
  });
}

const fixed = (value) =>
  typeof value === "number" && Number.isFinite(value) ? value.toFixed(1) : "n/a";

function breakdown(phases) {
  if (!phases) return "no breakdown recorded";
  return [
    `process start ${fixed(phases.process_start_ms)} ms`,
    `event registry ${fixed(phases.event_registry_ms)} ms`,
    `listing the root ${fixed(phases.list_ms)} ms`,
    `launch records and summaries ${fixed(phases.launch_and_summary_ms)} ms`,
    `registries ${fixed(phases.registry_ms)} ms`,
    `proving pids ${fixed(phases.pid_proofs_ms)} ms`,
    `deciding ended runs ${fixed(phases.decide_ended_ms)} ms`,
    `folding ${phases.live_runs_folded ?? 0} live run(s) of ${phases.live_journal_bytes ?? 0} journal bytes ${fixed(phases.live_fold_ms)} ms`,
    `free space ${fixed(phases.free_space_ms)} ms`,
    `rendering ${fixed(phases.render_ms)} ms`,
    `exit ${fixed(phases.exit_ms)} ms`,
  ].join(", ");
}

function detail(held) {
  const steady = held.steady;
  const parts = [
    `slowest of ${steady.reads_ms.length} steady single reads of the release build over the ${held.runs_root.run_roots}-run fixture, each judged: ` +
      `${steady.reads_ms.map(fixed).join(", ")} ms; median ${fixed(steady.median_ms)} ms (reported, not the budget); load1 ${fixed(steady.load1)}`,
    `breakdown: ${breakdown(steady.breakdown)}`,
    "every ended run's journal unreadable to the reads; that none is opened is held under strace by tests/e2e/host_view.rs",
    `cold read (every ended run's derived documents absent): ${fixed(held.cold.read_ms)} ms, ` +
      `${held.cold.breakdown.cold_journal_bytes ?? 0} journal bytes folded, ${held.cold.breakdown.read_bytes ?? 0} bytes read`,
  ];
  const increments = (held.scaling.increments ?? [])
    .map(
      (step) => `${fixed(step.ms_per_ended_run * 1000)} µs per ended run ${step.from}→${step.to}`,
    )
    .join(", ");
  parts.push(
    "scaling (same live set, more ended history): " +
      held.scaling.sizes
        .map((size) => `${size.run_roots} roots median ${fixed(size.median_ms)} ms`)
        .join("; ") +
      (increments ? `; ${increments}` : ""),
  );
  return parts.join(". ");
}

function main() {
  try {
    const held = readRecord();
    next = "ensure ONEBUDGETSPEC_RESULT names a writable file, then rerun 'just budgets'";
    const reported = report(Math.max(...held.steady.reads_ms), detail(held));
    if (!reported) {
      next = "invoke this command through 'just budgets'";
      throw new Error("ONEBUDGETSPEC_RESULT is missing; invoke through onebudgetspec");
    }
  } catch (error) {
    console.error(`${record}: ${error.message}\nnext: ${next}`);
    process.exitCode = 1;
  }
}

if (process.argv[1] === fileURLToPath(import.meta.url)) main();
