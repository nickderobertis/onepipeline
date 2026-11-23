import { readFileSync } from "node:fs";
import { report } from "@onebudgetspec/sdk";

// llmlint: ignore-block[contracts_have_one_source_or_a_drift_gate] one producer,
// `tests/human_items/main.rs`'s timed journey, and this one reader run back to back over one
// tree in the `human-items-e2e:timed` target, so a drifted field throws here or reaches
// onebudgetspec as a missing or non-finite result, which it refuses.
// llmlint: ignore-block[changed_behavior_has_e2e] the `human-items-e2e:timed` target drives
// it end to end: the journey writes the recording, then `onebudgetspec check` runs this,
// its only interface, and refuses a figure over its threshold by name.
// llmlint: ignore-block[boundary_inputs_validated] onebudgetspec owns validating what a
// reported budget's command hands it; this reads only the recording the same target's
// journey wrote, and reports one figure and its breakdown through the SDK.
const recording = JSON.parse(
  readFileSync(process.argv[2], "utf8"),
);
const ms = (value) => `${Number(value).toFixed(2)} ms`;
const phases = (sample) =>
  `index I/O ${ms(sample.index_io_ms)}, discovery ${ms(sample.discovery_ms)}, ` +
  `replay ${ms(sample.replay_ms)}, blocked work ${ms(sample.blocker_ms)}, ` +
  `rendering ${ms(sample.render_ms)}, process/capture ${ms(sample.capture_ms)}, ` +
  `sweep ${ms(sample.sweep_ms)} over ${sample.conditions_evaluated} conditions`;
const forms = [...new Set(recording.warm.map((s) => s.phase))]
  .map((form) => {
    const samples = recording.warm.filter((s) => s.phase === form);
    const worst = samples.reduce((a, b) => (b.elapsed_s > a.elapsed_s ? b : a));
    return `${form} max ${worst.elapsed_s.toFixed(4)} s (${phases(worst)})`;
  })
  .join("; ");
const slowest = recording.warm.reduce((a, b) => (b.elapsed_s > a.elapsed_s ? b : a));
const w = recording.workload;
report(
  recording.warm_max_elapsed_s,
  `host ${recording.host}, load ${recording.load}; ${recording.warm.length} warm samples, max ` +
    `${recording.warm_max_elapsed_s.toFixed(4)} s (${slowest.phase}: ${phases(slowest)}); ` +
    `slowest form ${slowest.phase}; the shown item's history holds ${w.shown_history} events; ` +
    `workload ${w.runs} runs, ${w.run_records} run records, ${w.open_items} open items, ` +
    `${w.managers} managers, ${w.plans} plans, ${w.steps} steps, ${w.conditions} conditions; ` +
    `fixture ${recording.fixture.key} ${recording.fixture.provenance.toLowerCase()}; per form — ${forms}`,
);
// llmlint: ignore-end[boundary_inputs_validated]
// llmlint: ignore-end[changed_behavior_has_e2e]
// llmlint: ignore-end[contracts_have_one_source_or_a_drift_gate]
