#!/usr/bin/env node
// Report `pool-maintenance-engine-seconds` from the workload journey's recording.
import { readFileSync } from "node:fs";
import { report } from "@onebudgetspec/sdk";

const recording = JSON.parse(
  readFileSync("../../target/budget-records/pool-maintenance.json", "utf8"),
);
const sweep = (Date.parse(recording.journalled_at) - Date.parse(recording.started_at)) / 1000;
const commands = recording.slot_duration_ms.reduce((sum, ms) => sum + ms, 0) / 1000;
const results = recording.results_seconds;
report(
  sweep - commands + results,
  `sweep ${sweep.toFixed(3)} s less declared commands ${commands.toFixed(3)} s, ` +
    `plus results render ${results.toFixed(3)} s; ` +
    `${recording.identities} identities, ${recording.slots} slots`,
);
