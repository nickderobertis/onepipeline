#!/usr/bin/env node
// Report `pool-maintenance-engine-seconds` from the workload journey's recording.
import { readFileSync } from "node:fs";
import { report } from "@onebudgetspec/sdk";

// llmlint: ignore-block[boundary_inputs_validated] the task that added this budget rules it a direct analysis with no validator of its own: it does not check for a missing recording or validate result presence, shape or finiteness, because result validation belongs to onebudgetspec, which refuses a missing, non-finite or absent result itself. The one producer is `tests/pool_workload/main.rs`, which writes the recording only after asserting the sweep it reads.
// llmlint: ignore-block[contracts_have_one_source_or_a_drift_gate] the recording has one producer, `tests/pool_workload/main.rs`, and this one reader, and the `workload` target runs the two back to back over the same tree: a drifted field reaches onebudgetspec as a missing or non-finite result, which it refuses. A schema of its own is the second validator the task that added this budget rules out.
// llmlint: ignore-block[changed_behavior_has_e2e] driven end to end by its one caller: the `onepipeline-pool-workload:workload` target runs the journey that writes the recording and then this command through `onebudgetspec check`, which is the only interface it has. A test asserting the figure is what the task that added this budget rules out.
// llmlint: ignore-block[cli_output_contract] whether `report` reached a result file is onebudgetspec's to check: run outside `onebudgetspec check` there is no result to write, and the checker that invokes this refuses a command that reported nothing. The task that added this budget rules out checking result presence here.
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
