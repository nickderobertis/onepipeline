import { readFileSync } from "node:fs";
import { report } from "@onebudgetspec/sdk";

// llmlint: ignore-block[contracts_have_one_source_or_a_drift_gate] one producer,
// `tests/journal_workload/main.rs`, and this one reader run back to back over one tree in
// the `workload` target, so a drifted field throws here or reaches onebudgetspec as a
// missing or non-finite result, which it refuses.
// llmlint: ignore-block[changed_behavior_has_e2e] the `onepipeline-journal-workload:workload`
// target drives it end to end: the journey writes the recording, then `onebudgetspec check`
// runs this, its only interface, and refuses a figure over its threshold by name.
// llmlint: ignore-block[boundary_inputs_validated] onebudgetspec owns validating what a
// reported budget's command hands it: presence, shape, finiteness and the threshold.
// This analysis only reads the journey's recording, written by
// `tests/journal_workload/main.rs` in the same `workload` target, and reports one
// budget's figure and its per-size detail through the SDK.
const recording = JSON.parse(
  readFileSync("../../target/budget-records/journal-workload.json", "utf8"),
);
const ms = (value) => `${Math.round(value)} ms`;
const spread = (p, unit = ms) =>
  p.n ? `p50 ${unit(p.p50)}, p95 ${unit(p.p95)}, max ${unit(p.max)} of ${p.n}` : "none";
const passes = (size) =>
  `reconcile passes ${size.pass.count}, pass time as flush-interval averages over ` +
  `${size.pass.intervals} intervals, not per-pass percentiles ` +
  `(${spread(size.pass.interval_average_ms)})`;

const budgets = {
  "event-delay": [
    (size) => size.event_delay_ms.p95,
    (size) =>
      `delay ${spread(size.event_delay_ms)} over ${size.activity.emitted} records; ` +
      `first loop-stats flush counting a pass ${size.to_first_pass_flush_s.toFixed(1)} s ` +
      `after adoption; ${passes(size)}`,
  ],
  "envelope-latency": [
    (size) => size.envelope_latency_ms.p95,
    (size) =>
      `answer ${spread(size.envelope_latency_ms)}; ` +
      `outcome in queue ${spread(size.envelope_outcome_ms)}; ${passes(size)}`,
  ],
  "idle-cpu": [
    (size) => size.idle.share,
    (size) =>
      `${size.idle.share.toFixed(4)} cores over ${size.idle.seconds.toFixed(0)} s; ` +
      `${size.idle.pass.cpu_s_per_pass.toFixed(4)} CPU s per pass; ${passes(size)}`,
  ],
  "peak-rss": [(size) => size.peak_rss_mb, (size) => `${size.peak_rss_mb.toFixed(1)} MB`],
  "liveness-bytes": [
    (size) => size.liveness.bytes_per_member_hour,
    (size) =>
      `${size.liveness.heartbeats} heartbeats, ${size.liveness.liveness_bytes} B of liveness ` +
      `kinds ${JSON.stringify(size.liveness.kinds)}, ${size.liveness.other_bytes} B of other ` +
      `kinds ${JSON.stringify(size.liveness.other_kinds)}, ` +
      `liveness.json ${size.liveness.liveness_json_bytes} B`,
  ],
};

const budget = budgets[process.argv[2]];
if (!budget) {
  throw new Error(`expected one of ${Object.keys(budgets).join(", ")}`);
}
const [figure, detail] = budget;
report(
  Math.max(...recording.sizes.map(figure)),
  recording.sizes.map((size) => `${size.size_mb} MB: ${detail(size)}`).join("; "),
);
// llmlint: ignore-end[boundary_inputs_validated]
// llmlint: ignore-end[contracts_have_one_source_or_a_drift_gate]
// llmlint: ignore-end[changed_behavior_has_e2e]
