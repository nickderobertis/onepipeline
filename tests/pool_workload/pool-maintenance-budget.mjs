#!/usr/bin/env node
// Report `pool-maintenance-engine-seconds` from the workload journey's recording.
import { readFileSync } from "node:fs";
import { report } from "@onebudgetspec/sdk";

// llmlint: ignore-block[boundary_inputs_validated] onebudgetspec validates the reported result — it refuses a missing or non-finite value — and this budget's analysis carries no second validator; the one producer, `tests/pool_workload/main.rs`, asserts the sweep before writing the recording.
// llmlint: ignore-block[contracts_have_one_source_or_a_drift_gate] one producer, `tests/pool_workload/main.rs`, and this one reader run back to back over one tree in the `workload` target, so a drifted field reaches onebudgetspec as a missing or non-finite result, which it refuses.
// llmlint: ignore-block[changed_behavior_has_e2e] the `onepipeline-pool-workload:workload` target drives it end to end: the journey writes the recording, then `onebudgetspec check` runs this, its only interface.
// llmlint: ignore-block[cli_output_contract] whether `report` reached a result file is onebudgetspec's to check: the `onebudgetspec check` that runs this refuses a command that reported nothing.
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
// llmlint: ignore-end[cli_output_contract]
// llmlint: ignore-end[changed_behavior_has_e2e]
// llmlint: ignore-end[contracts_have_one_source_or_a_drift_gate]
// llmlint: ignore-end[boundary_inputs_validated]
