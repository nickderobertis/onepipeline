import { readFileSync } from "node:fs";
import { report } from "@onebudgetspec/sdk";

// llmlint: ignore-block[contracts_have_one_source_or_a_drift_gate] one producer,
// `tests/human_items/main.rs`, and this one reader run back to back over one tree in the
// `human-items-e2e:test` target, so a drifted field throws here or reaches onebudgetspec
// as a missing or non-finite result, which it refuses.
// llmlint: ignore-block[changed_behavior_has_e2e] the `human-items-e2e:test` target drives
// it end to end: the journey writes the recording, then `onebudgetspec check` runs this,
// its only interface, and refuses a figure over its threshold by name.
// llmlint: ignore-block[boundary_inputs_validated] onebudgetspec owns validating what a
// reported budget's command hands it; this reads only the recording the same target's
// journey wrote, and reports one figure and its breakdown through the SDK.
const recording = JSON.parse(
  readFileSync(new URL("../../target/budget-records/human-items.json", import.meta.url), "utf8"),
);
const ms = (value) => `${Number(value).toFixed(2)} ms`;
const phases = (sample) =>
  `index I/O ${ms(sample.index_io_ms)}, discovery ${ms(sample.discovery_ms)}, ` +
  `replay ${ms(sample.replay_ms)}, blocked work ${ms(sample.blocker_ms)}, ` +
  `rendering ${ms(sample.render_ms)}, process/capture ${ms(sample.capture_ms)}`;
const slowest = recording.warm.reduce((a, b) => (b.elapsed_s > a.elapsed_s ? b : a));
const recoveries = [...recording.cold, ...recording.tail]
  .map(
    (p) =>
      `${p.phase}: ${p.elapsed_s.toFixed(3)} s, ${p.item_events_replayed} item and ` +
      `${p.run_events_replayed} run events replayed, ${p.runs_rederived} runs rederived`,
  )
  .join("; ");
const w = recording.workload;
report(
  recording.warm_max_elapsed_s,
  `host ${recording.host}; ${recording.warm.length} warm samples, max ` +
    `${recording.warm_max_elapsed_s.toFixed(4)} s (${slowest.phase}: ${phases(slowest)}); ` +
    `workload ${w.runs} runs x ${w.events_per_run} events, ${w.open_items} open items, ` +
    `${w.plans} plans, ${w.steps} steps; disclosed recoveries — ${recoveries}`,
);
// llmlint: ignore-end[boundary_inputs_validated]
// llmlint: ignore-end[changed_behavior_has_e2e]
// llmlint: ignore-end[contracts_have_one_source_or_a_drift_gate]
