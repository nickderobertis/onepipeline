// llmlint: ignore-block[contracts_have_one_source_or_a_drift_gate] These allowlists are this repository's closed interpreter subset, not a claim of GitHub schema support; every field outside them is refused explicitly and that refusal is tested.
const supportedFields = {
  workflow: ["name", "run-name", "on", "permissions", "concurrency", "env", "defaults", "jobs"],
  job: [
    "name",
    "if",
    "needs",
    "strategy",
    "runs-on",
    "timeout-minutes",
    "env",
    "defaults",
    "steps",
    "permissions",
    "uses",
    "with",
    "secrets",
    "outputs",
    "concurrency",
    "continue-on-error",
    "environment",
    "container",
    "services",
  ],
  step: [
    "name",
    "id",
    "if",
    "run",
    "uses",
    "with",
    "env",
    "shell",
    "working-directory",
    "continue-on-error",
    "timeout-minutes",
  ],
  schedule: ["cron"],
  strategy: ["matrix", "fail-fast", "max-parallel"],
};
// llmlint: ignore-end[contracts_have_one_source_or_a_drift_gate]
function fields(value, allowed, site) {
  if (!value || typeof value !== "object" || Array.isArray(value))
    throw new Error(`invalid workflow mapping: ${site}`);
  for (const key of Object.keys(value))
    if (!allowed.includes(key)) throw new Error(`unsupported workflow field ${site}.${key}`);
}
export function validateWorkflow(workflow) {
  fields(workflow, supportedFields.workflow, "workflow");
  const triggers = workflow.on;
  const triggerNames =
    typeof triggers === "string"
      ? [triggers]
      : Array.isArray(triggers)
        ? triggers
        : triggers && typeof triggers === "object"
          ? Object.keys(triggers)
          : [];
  if (
    !triggerNames.length ||
    triggerNames.some((name) => typeof name !== "string" || !name.trim()) ||
    (triggers &&
      typeof triggers === "object" &&
      !Array.isArray(triggers) &&
      Object.entries(triggers).some(
        ([event, options]) =>
          options !== null &&
          (typeof options !== "object" || (Array.isArray(options) && event !== "schedule")),
      ))
  )
    throw new Error("invalid workflow trigger shape");
  if (triggerNames.includes("schedule")) {
    const schedules = triggers.schedule;
    if (!Array.isArray(schedules) || !schedules.length) throw new Error("invalid schedule shape");
    for (const entry of schedules) {
      fields(entry, supportedFields.schedule, "schedule");
      // llmlint: ignore[boundary_inputs_validated] Cron text is carried as trigger metadata, not interpreted; GitHub validates its syntax when loading workflows. This interpreter validates the mapping and string shape.
      if (typeof entry.cron !== "string" || entry.cron.trim().split(/\s+/).length !== 5)
        throw new Error("invalid schedule cron");
    }
  }
  if (
    !workflow.on ||
    !workflow.jobs ||
    typeof workflow.jobs !== "object" ||
    Array.isArray(workflow.jobs)
  )
    throw new Error("workflow needs triggers and jobs");
  for (const [key, job] of Object.entries(workflow.jobs)) {
    fields(job, supportedFields.job, key);
    if (job.uses && job["runs-on"]) throw new Error(`invalid reusable runner: ${key}`);
    if (!job.uses && !job["runs-on"]) throw new Error(`unresolved runner: ${key}`);
    if (job.if !== undefined && typeof job.if !== "string")
      throw new Error(`unsupported job condition: ${key}`);
    if (
      job.needs !== undefined &&
      typeof job.needs !== "string" &&
      (!Array.isArray(job.needs) || job.needs.some((n) => typeof n !== "string"))
    )
      throw new Error(`invalid job needs: ${key}`);
    if (job.strategy !== undefined)
      fields(job.strategy, supportedFields.strategy, `${key}.strategy`);
    if (job.steps !== undefined && !Array.isArray(job.steps))
      throw new Error(`invalid job steps: ${key}`);
    for (const [index, step] of (job.steps ?? []).entries()) {
      fields(step, supportedFields.step, `${key}.steps[${index}]`);
      if (
        (step.run !== undefined && (typeof step.run !== "string" || !step.run.trim())) ||
        (step.uses !== undefined && (typeof step.uses !== "string" || !step.uses.trim())) ||
        Boolean(step.run) === Boolean(step.uses)
      )
        throw new Error(`invalid step command: ${key}[${index}]`);
    }
  }
  return workflow;
}
