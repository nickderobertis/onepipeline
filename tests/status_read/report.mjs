import { readFileSync } from "node:fs";
import { report } from "@onebudgetspec/sdk";

// llmlint: ignore-block[contracts_have_one_source_or_a_drift_gate] one producer,
// `tests/status_read/main.rs`, and this one reader run back to back over one tree in the
// `onepipeline-status-read:test` target, so a drifted field throws here or reaches
// onebudgetspec as a missing or non-finite result, which it refuses.
// llmlint: ignore-block[changed_behavior_has_e2e] the `onepipeline-status-read:test` target
// drives it end to end: the journey writes the recording, then `onebudgetspec check` runs
// this, its only interface, and refuses a figure over its threshold by name.
// llmlint: ignore-block[boundary_inputs_validated] onebudgetspec owns validating what a
// reported budget's command hands it; this reads only the recording the same target's journey
// wrote, and what it refuses below is a corpus without its drivers, not malformed input.
const [path, flag, metric] = process.argv.slice(2);
if (flag !== "--metric" || !["status", "results", "transcript"].includes(metric)) {
  throw new Error("usage: report.mjs RECORD --metric status|results|transcript");
}
const recording = JSON.parse(readFileSync(path, "utf8"));
const phase = (read, name) => {
  const wall = read.phases?.phases?.[name]?.wall_ms;
  if (typeof wall !== "number") throw new Error(`the recording holds no ${name} phase`);
  return `${wall.toFixed(1)} ms`;
};
const load = `host load ${recording.host_load}`;

const reporters = {
  status({ status: s }) {
    const missing = [];
    if (s.checkpoint_bytes < 6_500_000) {
      missing.push(`the run's checkpoint is ${s.checkpoint_bytes} bytes, under 6.5 MB`);
    }
    if (s.streams < 2_000) missing.push(`the run holds ${s.streams} streams, under 2,000`);
    if (s.stand_in_starts !== 0) {
      missing.push(`the stand-in provider CLI started ${s.stand_in_starts} times in the default read`);
    }
    if (missing.length > 0) throw new Error(`the corpus lacks its drivers: ${missing.join("; ")}`);
    report(
      s.seconds,
      `${load}; large run ${s.large.seconds.toFixed(3)} s (checkpoint read ` +
        `${phase(s.large, "read")}, render ${phase(s.large, "render")}), small run ` +
        `${s.small.seconds.toFixed(3)} s (checkpoint read ${phase(s.small, "read")}, render ` +
        `${phase(s.small, "render")}); checkpoint ${s.checkpoint_bytes} bytes, ` +
        `${s.records} records, ${s.streams} streams, ${s.run_directories} run directories, ` +
        `stand-in provider CLI started ${s.stand_in_starts} times`,
    );
  },
  results({ results: r }) {
    if (r.streams < 2_000) throw new Error(`the run holds ${r.streams} streams, under 2,000`);
    report(
      r.seconds,
      `${load}; journal read and fold ${phase(r, "read")}, render ${phase(r, "render")}; ` +
        `${r.records} records, ${r.streams} streams`,
    );
  },
  transcript({ transcript: t }) {
    if (t.unresolved !== 0) {
      throw new Error(`${t.unresolved} of ${t.references} references did not resolve`);
    }
    report(
      t.seconds,
      `${load}; ${t.references} references into ${t.session_files} session files, ` +
        `${t.session_bytes} bytes in all`,
    );
  },
};
reporters[metric](recording);
// llmlint: ignore-end[boundary_inputs_validated]
// llmlint: ignore-end[changed_behavior_has_e2e]
// llmlint: ignore-end[contracts_have_one_source_or_a_drift_gate]
