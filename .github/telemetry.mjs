import { inventory } from "./workload.mjs";
function object(value, fields, site) {
  if (
    !value ||
    typeof value !== "object" ||
    Array.isArray(value) ||
    Object.keys(value).some((key) => !fields.includes(key)) ||
    fields.some((key) => !Object.hasOwn(value, key))
  )
    throw new Error(`invalid telemetry object: ${site}`);
}
function string(value, site) {
  if (typeof value !== "string" || !value) throw new Error(`invalid telemetry string: ${site}`);
}
function runs(value, site) {
  if (!Array.isArray(value)) throw new Error(`invalid telemetry runs: ${site}`);
  for (const run of value) {
    object(run, ["workflow", "event", "head", "tag", "jobs"], site);
    for (const key of ["workflow", "event", "head"]) string(run[key], `${site}.${key}`);
    if (run.tag !== null) string(run.tag, `${site}.tag`);
    if (!Array.isArray(run.jobs) || !run.jobs.length)
      throw new Error(`invalid telemetry jobs: ${site}`);
    for (const job of run.jobs) {
      object(
        job,
        ["targets", "job", "name", "runner", "selected", "macos", "target", "needs"],
        site,
      );
      if (typeof job.selected !== "boolean" || typeof job.macos !== "boolean")
        throw new Error(`invalid telemetry job flags: ${site}`);
      for (const key of ["job", "name"]) string(job[key], `${site}.${key}`);
      for (const key of ["targets", "needs"]) {
        if (!Array.isArray(job[key])) throw new Error(`invalid telemetry ${key}`);
        for (const entry of job[key]) string(entry, key);
      }
      if (job.target !== null) string(job.target, "target");
      const labels = Array.isArray(job.runner) ? job.runner : [job.runner];
      if (!labels.length) throw new Error("invalid telemetry runner");
      for (const label of labels) string(label, "runner");
      if (job.macos !== labels.some((label) => label.startsWith("macos-")))
        throw new Error("telemetry Mac flag disagrees with runner");
    }
  }
}
export function validateTelemetry(value) {
  object(
    value,
    ["schema_version", "source_digest", "event_classes", "scenarios", "lifecycles"],
    "root",
  );
  if (value.schema_version !== 1 || !/^[a-f0-9]{64}$/.test(value.source_digest))
    throw new Error("invalid telemetry schema/digest");
  if (!Array.isArray(value.event_classes) || !value.event_classes.length)
    throw new Error("missing telemetry event classes");
  for (const entry of value.event_classes) string(entry, "event class");
  for (const key of ["scenarios", "lifecycles"]) {
    if (!Array.isArray(value[key]) || !value[key].length)
      throw new Error(`missing telemetry ${key}`);
    for (const entry of value[key]) {
      object(entry, ["id", "runs"], key);
      string(entry.id, `${key}.id`);
      runs(entry.runs, entry.id);
      if (key === "lifecycles" && !entry.runs.length) throw new Error("empty telemetry lifecycle");
    }
  }
  // llmlint: ignore-block[boundary_inputs_validated] This repository writes and reads telemetry inside the same journey; producer tests prove every run and Mac job is recorded. Protection against deliberate row deletion is outside this change.
  const expected = inventory();
  for (const key of ["event_classes", "scenarios", "lifecycles"]) {
    const actual = key === "event_classes" ? value[key] : value[key].map((entry) => entry.id);
    if (
      new Set(actual).size !== actual.length ||
      actual.length !== expected[key].length ||
      actual.some((id) => !expected[key].includes(id))
    )
      throw new Error(
        `incomplete telemetry ${key} inventory; run just nx run github-workflows:test`,
      );
  }
  // llmlint: ignore-end[boundary_inputs_validated]
  return value;
}
