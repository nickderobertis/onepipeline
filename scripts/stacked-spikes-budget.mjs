#!/usr/bin/env node
// Read the fan-out journey's telemetry; this command never launches a run.
// The result is written by onebudgetspec's own `report`, not by hand.
import { readFileSync } from "node:fs";
import { report } from "@onebudgetspec/sdk";

const record = "target/budget-records/stacked-spikes-stage.json";
let next =
  "run just test-e2e 'test(publish_preserve::stacked_spikes_fan_out)' " +
  "to regenerate the record, then run 'onebudgetspec check budgets.yaml'";
// The record as the journey wrote it, or an error saying whether it is missing or unreadable.
function readTiming() {
  let text;
  try {
    text = readFileSync(record, "utf8");
  } catch (error) {
    throw new Error(error.code === "ENOENT" ? "missing" : `unreadable: ${error.message}`);
  }
  try {
    const timing = JSON.parse(text);
    if (!Number.isSafeInteger(timing.wall_ms) || timing.wall_ms < 0) {
      throw new Error("wall_ms must be a nonnegative safe integer");
    }
    if (typeof timing.run_id !== "string" || !timing.run_id.trim()) {
      throw new Error("run_id must be a nonempty string");
    }
    return timing;
  } catch (error) {
    throw new Error(`unreadable: ${error.message}`);
  }
}

try {
  const timing = readTiming();
  next =
    "ensure ONEBUDGETSPEC_RESULT names a writable file, then rerun " +
    "'onebudgetspec check budgets.yaml'";
  const reported = report(
    timing.wall_ms / 1000,
    `run ${timing.run_id}: run journal's first event to its last; ` +
      "excludes binary startup before its first journal write; read from the test run",
  );
  if (!reported) {
    next = "invoke this command through 'onebudgetspec check budgets.yaml'";
    throw new Error("ONEBUDGETSPEC_RESULT is missing; invoke through onebudgetspec");
  }
} catch (error) {
  console.error(`${record}: ${error.message}\nnext: ${next}`);
  process.exitCode = 1;
}
