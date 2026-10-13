import { readFileSync } from "node:fs";
import { report } from "@onebudgetspec/sdk";

const selector = process.argv[2];
if (selector !== "ending" && selector !== "discard") {
  throw new Error("expected ending or discard");
}

// llmlint: ignore-block[boundary_inputs_validated] The task explicitly assigns telemetry
// result presence, shape, finiteness and threshold validation to onebudgetspec. This
// analysis only reads the journey's recording and reports its figures through the SDK.
const recording = JSON.parse(
  readFileSync("../../target/budget-records/scratch-workload.json", "utf8"),
);
// llmlint: ignore-end[boundary_inputs_validated]
const describe = (name, timing) =>
  `${name}: ${timing.entries} entries, ${timing.dirs} directories, ${timing.bytes} bytes, ${timing.seconds.toFixed(3)} seconds`;
if (selector === "ending") {
  report(
    Math.max(recording.failed.seconds, recording.success.seconds),
    `${describe("failed ending", recording.failed)}; ${describe("success ending", recording.success)}`,
  );
} else {
  report(
    recording.discard.wall_seconds,
    `${describe("discard removal", recording.discard)}; state check ${recording.discard.state_seconds.toFixed(3)} seconds; invocation to exit ${recording.discard.wall_seconds.toFixed(3)} seconds`,
  );
}
