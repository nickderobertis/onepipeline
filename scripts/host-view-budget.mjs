#!/usr/bin/env node
// Read the host-view journeys' record; this command never runs `onepipeline host`.
//
// The budget is one read: each of the steady single reads is judged against the
// threshold, so the value reported is the slowest of them. Their median, the phase
// breakdown, the debug build's reads, the cold read and the scaling journey's
// sizes go in the detail as evidence, never as the verdict.
import { readFileSync } from "node:fs";
import { report } from "@onebudgetspec/sdk";

const record = "target/budget-records/host-view.json";
// What the budget's workload states, and so what a record has to have measured.
const READS = 5;
const FIXTURE = { run_roots: 100, live_dispatches: 5, stale_entries: 4 };
const journeys = "just test-e2e 'test(/^host_view::the_host_view/)'";
let next = `run ${journeys} to regenerate the record, then run 'just budgets'`;

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
  if (held.steady.ended_journal_bytes_opened !== 0) {
    fail("steady.ended_journal_bytes_opened must be 0: a steady read that opened one failed its journey");
  }
  validateDetail(held);
  return held;
}

const isObject = (value) => value !== null && typeof value === "object" && !Array.isArray(value);

function names(value, name) {
  if (!Array.isArray(value) || value.some((item) => typeof item !== "string")) {
    fail(`${name} must be an array of run names`);
  }
}

function count(value, name) {
  if (!Number.isSafeInteger(value) || value < 0) {
    fail(`${name} must be a nonnegative integer`);
  }
}

function object(value, name) {
  if (value !== undefined && !isObject(value)) {
    fail(`${name} must be an object`);
  }
}

// Every figure a breakdown, a median or a load carries is a nonnegative number, or
// null where the read did not measure it.
function figure(value, name) {
  if (value !== undefined && value !== null && (typeof value !== "number" || !Number.isFinite(value) || value < 0)) {
    fail(`${name} must be a nonnegative number or null`);
  }
}

function breakdownOf(value, name) {
  object(value, name);
  for (const [key, field] of Object.entries(value ?? {})) figure(field, `${name}.${key}`);
}

// The detail's fields are each optional — a record holds what the journeys that ran
// wrote — but each one present has the shape the detail reads it as.
function validateDetail(held) {
  breakdownOf(held.steady.breakdown, "steady.breakdown");
  figure(held.steady.median_ms, "steady.median_ms");
  figure(held.steady.load1, "steady.load1");
  if (held.steady.runs_opened !== undefined) names(held.steady.runs_opened, "steady.runs_opened");
  object(held.steady_debug, "steady_debug");
  if (held.steady_debug !== undefined) {
    reads(held.steady_debug.reads_ms, "steady_debug.reads_ms");
    figure(held.steady_debug.median_ms, "steady_debug.median_ms");
    breakdownOf(held.steady_debug.breakdown, "steady_debug.breakdown");
  }
  object(held.cold, "cold");
  if (held.cold !== undefined) {
    milliseconds(held.cold.read_ms, "cold.read_ms");
    breakdownOf(held.cold.breakdown, "cold.breakdown");
  }
  object(held.scaling, "scaling");
  if (held.scaling !== undefined) {
    if (!Array.isArray(held.scaling.sizes)) fail("scaling.sizes must be an array");
    held.scaling.sizes.forEach((size, n) => {
      if (!isObject(size)) fail(`scaling.sizes[${n}] must be an object`);
      count(size.run_roots, `scaling.sizes[${n}].run_roots`);
      milliseconds(size.median_ms, `scaling.sizes[${n}].median_ms`);
      count(size.ended_journal_bytes_opened, `scaling.sizes[${n}].ended_journal_bytes_opened`);
      names(size.runs_opened, `scaling.sizes[${n}].runs_opened`);
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
}

const fixed = (value) => (typeof value === "number" && Number.isFinite(value) ? value.toFixed(1) : "n/a");

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
    `slowest of ${steady.reads_ms.length} steady single reads of the release build over the ${held.runs_root?.run_roots ?? "?"}-run fixture, each judged: ` +
      `${steady.reads_ms.map(fixed).join(", ")} ms; median ${fixed(steady.median_ms)} ms (reported, not the budget); load1 ${fixed(steady.load1)}`,
    `breakdown: ${breakdown(steady.breakdown)}`,
    `ended-run journal bytes opened: ${steady.ended_journal_bytes_opened}; runs opened: ${(steady.runs_opened ?? []).join(", ")}`,
  ];
  if (held.cold) {
    parts.push(
      `cold read (release, every ended run's derived documents absent): ${fixed(held.cold.read_ms)} ms, ` +
        `${held.cold.breakdown?.cold_journal_bytes ?? "?"} journal bytes folded, ${held.cold.breakdown?.read_bytes ?? "?"} bytes read`,
    );
  }
  if (held.steady_debug) {
    parts.push(
      `the e2e tier's own build (detail only): ${(held.steady_debug.reads_ms ?? []).map(fixed).join(", ")} ms, ` +
        `median ${fixed(held.steady_debug.median_ms)} ms, of which the event registry ${fixed(held.steady_debug.breakdown?.event_registry_ms)} ms`,
    );
  }
  const sizes = held.scaling?.sizes;
  if (Array.isArray(sizes) && sizes.length > 0) {
    const increments = (held.scaling.increments ?? [])
      .map((step) => `${fixed(step.ms_per_ended_run * 1000)} µs per ended run ${step.from}→${step.to}`)
      .join(", ");
    parts.push(
      `scaling (release, same live set): ` +
        sizes
          .map((size) => `${size.run_roots} roots median ${fixed(size.median_ms)} ms, ${size.ended_journal_bytes_opened} ended-journal bytes opened, runs opened ${(size.runs_opened ?? []).join("/")}`)
          .join("; ") +
        (increments ? `; ${increments}` : ""),
    );
  } else {
    parts.push("scaling: not recorded");
  }
  return parts.join(". ");
}

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
