#!/usr/bin/env node
// Report one store-operation budget from the telemetry its e2e journey recorded; this command
// runs no journey and asks no store anything.
//
// onebudgetspec runs it from this repository's root with `ONEBUDGETSPEC_BUDGET_ID` naming the
// budget and `ONEBUDGETSPEC_RESULT` the file the result goes to. The record is the journey's,
// at `target/budget-records/store-operations/<budget id>.json`: every operation the engine
// asked the plan store for, counted at the store seam by kind and naming no backend. What
// each backend spends per operation is onetaskgraph's to bound.
import { readFileSync } from "node:fs";
import { report } from "@onebudgetspec/sdk";

const directory = "target/budget-records/store-operations";
/** The journey that records each budget, which is how a missing record is regenerated. */
const JOURNEYS = {
  "source-hold-store-operations-per-hour":
    "store::a_held_hour_asks_the_store_what_its_reset_and_backoff_schedule_and_nothing_more",
  "store-operations-per-dispatch":
    "store::each_dispatch_and_settlement_of_a_twenty_task_run_records_the_store_operations_it_asked_for",
  "store-operations-per-settlement":
    "store::each_dispatch_and_settlement_of_a_twenty_task_run_records_the_store_operations_it_asked_for",
};

function fail(message) {
  throw new Error(`unreadable: ${message}`);
}

const count = (value, name) => {
  if (!Number.isSafeInteger(value) || value < 0) fail(`${name} must be a nonnegative integer`);
  return value;
};

/** Operations by kind, each a count; the kinds are the store API's, never a backend's. */
function kinds(value, name) {
  if (value === null || typeof value !== "object" || Array.isArray(value)) {
    fail(`${name} must map each operation kind to its count`);
  }
  for (const [kind, n] of Object.entries(value)) count(n, `${name}.${kind}`);
  return value;
}

const total = (byKind) => Object.values(byKind).reduce((sum, n) => sum + n, 0);

/** Hold a record's workload to the one its budget states: a figure taken at another is not
 * this budget's figure, however well formed. */
function workload(record, stated) {
  const held = record.workload;
  if (held === null || typeof held !== "object" || Array.isArray(held)) fail("workload is missing");
  for (const [key, value] of Object.entries(stated)) {
    if (held[key] !== value) {
      fail(`workload.${key} is ${JSON.stringify(held[key])}, where the budget's workload is ${value}`);
    }
  }
  return held;
}

function said(byKind) {
  const entries = Object.entries(byKind);
  return entries.length === 0 ? "none" : entries.map(([kind, n]) => `${kind} ${n}`).join(", ");
}

/** What each write-back attempt spent that belongs to no single item: its overhead. */
function overhead(attempts) {
  if (!Array.isArray(attempts) || attempts.length === 0) fail("attempts must be a nonempty list");
  const summed = {};
  for (const [index, attempt] of attempts.entries()) {
    count(attempt.items, `attempts[${index}].items`);
    for (const [kind, n] of Object.entries(kinds(attempt.shared, `attempts[${index}].shared`))) {
      summed[kind] = (summed[kind] ?? 0) + n;
    }
  }
  return (
    `per-attempt overhead, attributed to no dispatch or settlement: ${attempts.length} ` +
    `write-back attempts spent ${said(summed)}`
  );
}

function hour(record) {
  const stated = workload(record, {
    tasks: 20,
    held_nodes: 3,
    private_sources: 1,
    poll_seconds: 1,
    hour_intervals: 60,
    reset_after_intervals: 30,
  });
  if (!Array.isArray(record.passes)) fail("passes must be a list");
  const passes = record.passes.map((pass, index) => {
    const name = `passes[${index}]`;
    count(pass.after_launch_ms, `${name}.after_launch_ms`);
    count(pass.wait_intervals, `${name}.wait_intervals`);
    count(pass.gap_ms, `${name}.gap_ms`);
    if (pass.by !== "reset" && pass.by !== "backoff") fail(`${name}.by must be reset or backoff`);
    return { ...pass, total: total(kinds(pass.operations, `${name}.operations`)) };
  });
  const value = count(record.total, "total");
  if (passes.reduce((sum, pass) => sum + pass.total, 0) !== value) {
    fail("total is not the sum of its passes' operations");
  }
  if (total(kinds(record.per_source, "per_source")) !== value) {
    fail("per_source does not sum to total");
  }
  if (Object.keys(kinds(record.per_node, "per_node")).length !== stated.held_nodes) {
    fail(`per_node must name each of the ${stated.held_nodes} held nodes`);
  }
  if (total(record.per_node) !== value) fail("per_node does not sum to total");
  const fixed = record.fixed_interval ?? fail("fixed_interval is missing");
  if (fixed.basis !== "arithmetic") fail("fixed_interval.basis must be arithmetic");
  if (fixed.interval_s !== stated.poll_seconds) {
    fail("fixed_interval.interval_s must be the workload's poll interval");
  }
  if (fixed.passes !== stated.hour_intervals || fixed.held_nodes !== stated.held_nodes) {
    fail("fixed_interval must re-read every held node once an interval through the hour");
  }
  if (count(fixed.operations, "fixed_interval.operations") !== fixed.passes * fixed.held_nodes) {
    fail("fixed_interval.operations is not its passes times its held nodes");
  }
  const at = (ms) => `${(ms / 1000).toFixed(1)}s`;
  const resets = passes.filter((pass) => pass.by === "reset").length;
  return [
    value,
    `${value} store operations over the held hour after launch (${stated.hour_intervals} ` +
      `poll intervals of ${stated.poll_seconds}s, a reset ${stated.reset_after_intervals} out), ` +
      `launch excluded: ${resets} re-read at a reset and ${passes.length - resets} on backoff; ` +
      `per source ${said(record.per_source)}; per held node ${said(record.per_node)}; passes ` +
      passes
        .map((pass) => `+${at(pass.after_launch_ms)} ${pass.by} [${said(pass.operations)}]`)
        .join(", ") +
      `; waits chosen ${passes.map((pass) => pass.wait_intervals).join(", ")} intervals ` +
      `(gaps measured ${passes.map((pass) => at(pass.gap_ms)).join(", ")}); the next re-read ` +
      `falls after the hour; the unchanged fixed ${fixed.interval_s}s loop re-reading every held ` +
      `node each interval issues ${fixed.operations} at this workload (${fixed.passes} passes × ` +
      `${fixed.held_nodes} held nodes, ${fixed.basis})`,
  ];
}

function dispatch(record) {
  workload(record, { tasks: 20, holds: 0 });
  if (!Array.isArray(record.dispatches) || record.dispatches.length !== 20) {
    fail("dispatches must list the run's 20 dispatches");
  }
  const totals = record.dispatches.map((each, index) => {
    const spent = total(kinds(each.operations, `dispatches[${index}].operations`));
    if (count(each.total, `dispatches[${index}].total`) !== spent) {
      fail(`dispatches[${index}].total is not the sum of its operations`);
    }
    return spent;
  });
  const summed = {};
  for (const each of record.dispatches) {
    for (const [kind, n] of Object.entries(each.operations)) summed[kind] = (summed[kind] ?? 0) + n;
  }
  const value = Math.max(...totals);
  return [
    value,
    `the most store operations one of ${totals.length} dispatches asked for, its write-back ` +
      `projection included: ${value} (each: ${totals.join(", ")}); by kind across them ` +
      `${said(summed)}; ${overhead(record.attempts)}; the launch's plan read ` +
      `${said(kinds(record.launch, "launch"))}`,
  ];
}

function settlement(record) {
  workload(record, { tasks: 20, holds: 0 });
  if (!Array.isArray(record.settlements) || record.settlements.length !== 5) {
    fail("settlements must list five consecutive settlements");
  }
  const totals = record.settlements.map((each, index) => {
    if (each.ordinal !== index + 1) fail(`settlements[${index}].ordinal must be ${index + 1}`);
    const spent = total(kinds(each.operations, `settlements[${index}].operations`));
    if (count(each.total, `settlements[${index}].total`) !== spent) {
      fail(`settlements[${index}].total is not the sum of its operations`);
    }
    return spent;
  });
  const value = Math.max(...totals);
  return [
    value,
    `the most store operations one of five consecutive settlement write-backs asked for: ` +
      `${value}; the run's first settlement ${totals[0]}, then ${totals.slice(1).join(", ")}; ` +
      `by kind ${record.settlements.map((each) => `[${said(each.operations)}]`).join(", ")}; ` +
      overhead(record.attempts),
  ];
}

const REPORTERS = {
  "source-hold-store-operations-per-hour": hour,
  "store-operations-per-dispatch": dispatch,
  "store-operations-per-settlement": settlement,
};

const id = process.env.ONEBUDGETSPEC_BUDGET_ID;
const record = `${directory}/${id}.json`;
let next =
  "run this through 'onebudgetspec check budgets.yaml --id <budget>', which names the budget in " +
  "ONEBUDGETSPEC_BUDGET_ID";
try {
  const reporter = REPORTERS[id];
  if (reporter === undefined) {
    throw new Error(
      `ONEBUDGETSPEC_BUDGET_ID ${JSON.stringify(id ?? null)} names none of ` +
        Object.keys(REPORTERS).join(", "),
    );
  }
  next =
    `run just test-e2e 'test(=${JOURNEYS[id]})' to record it, then run ` +
    `'onebudgetspec check budgets.yaml --id ${id}'`;
  let text;
  try {
    text = readFileSync(record, "utf8");
  } catch (error) {
    throw new Error(error.code === "ENOENT" ? "missing" : `unreadable: ${error.message}`);
  }
  let parsed;
  try {
    parsed = JSON.parse(text);
  } catch (error) {
    fail(error.message);
  }
  if (parsed.schema_version !== 1) fail("schema_version must be 1");
  if (parsed.budget !== id) fail(`the record is ${JSON.stringify(parsed.budget)}'s, not ${id}'s`);
  const [value, detail] = reporter(parsed);
  next = "invoke this command through 'onebudgetspec check budgets.yaml'";
  if (!report(value, detail)) {
    throw new Error("ONEBUDGETSPEC_RESULT is missing; invoke through onebudgetspec");
  }
} catch (error) {
  console.error(`${record}: ${error.message}\nnext: ${next}`);
  process.exitCode = 1;
}
