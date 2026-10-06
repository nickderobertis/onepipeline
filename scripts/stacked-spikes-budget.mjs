#!/usr/bin/env node
// Read the fan-out journey's telemetry; this command never launches a run.
import { readFileSync, writeFileSync } from "node:fs";

const record = "target/budget-records/stacked-spikes-stage.json";
let next = "run just test-e2e 'test(publish_preserve::stacked_spikes_fan_out)' " +
  "to regenerate the record, then run 'onebudgetspec check budgets.yaml'";
try {
  const timing = JSON.parse(readFileSync(record, "utf8"));
  if (!Number.isSafeInteger(timing.wall_ms) || timing.wall_ms < 0) {
    throw new Error("wall_ms must be a nonnegative safe integer");
  }
  if (typeof timing.run_id !== "string" || !timing.run_id.trim()) {
    throw new Error("run_id must be a nonempty string");
  }
  next = "invoke this command through 'onebudgetspec check budgets.yaml'";
  if (!process.env.ONEBUDGETSPEC_RESULT) {
    throw new Error("ONEBUDGETSPEC_RESULT is missing; invoke through onebudgetspec");
  }
  next = "ensure ONEBUDGETSPEC_RESULT names a writable file, then rerun " +
    "'onebudgetspec check budgets.yaml'";
  writeFileSync(process.env.ONEBUDGETSPEC_RESULT, JSON.stringify({
    value: timing.wall_ms / 1000,
    detail: `run ${timing.run_id}: run journal's first event to its last; ` +
      "excludes binary startup before its first journal write; read from the test run",
  }));
} catch (error) {
  console.error(`${record}: ${error.message}\nnext: ${next}`);
  process.exitCode = 1;
}
