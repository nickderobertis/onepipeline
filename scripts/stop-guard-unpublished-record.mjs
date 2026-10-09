// Read-only validation of the stop-verdict budget records, shared by the budget
// command and its tests. It launches nothing, builds nothing and times nothing.
import { createHash } from "node:crypto";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { repositoryRoot, sourceFingerprint } from "./stop-guard-unpublished-build.mjs";

export const journeyCommand = "just stop-verdict-journeys";
export const recordDirectory = fileURLToPath(new URL("../target/budget-records/", import.meta.url));
// Read on first use rather than at import, so a missing or broken schema reaches the
// caller's error handler, and its next action, instead of failing module loading.
let loaded;
function loadedSchema() {
  if (!loaded) {
    loaded = JSON.parse(
      readFileSync(new URL("./stop-guard-unpublished-budget.schema.json", import.meta.url), "utf8"),
    );
  }
  return loaded;
}
const sha = (bytes) => createHash("sha256").update(bytes).digest("hex");
const digest = (text) => typeof text === "string" && /^[a-f0-9]{64}$/.test(text);

/** Hold `value` to the checked schema: closed objects, every key required, types exact. */
function conforms(value, node, at) {
  const resolved = node.$ref ? loadedSchema().$defs[node.$ref.replace("#/$defs/", "")] : node;
  if (resolved.anyOf) {
    const errors = [];
    for (const option of resolved.anyOf) {
      try {
        conforms(value, option, at);
        return;
      } catch (error) {
        errors.push(error.message);
      }
    }
    throw new Error(errors.join("; "));
  }
  const type = resolved.type;
  const ok =
    (type === "object" && value !== null && typeof value === "object" && !Array.isArray(value)) ||
    (type === "array" && Array.isArray(value)) ||
    (type === "integer" && Number.isSafeInteger(value) && value >= 0) ||
    (type === "number" && Number.isFinite(value) && value >= 0) ||
    (type === "string" && typeof value === "string") ||
    (type === "null" && value === null);
  if (!ok) throw new Error(`${at} is not ${type}`);
  if (type === "array") {
    for (const [index, item] of value.entries()) conforms(item, resolved.items, `${at}[${index}]`);
  }
  if (type === "object" && resolved.properties) {
    for (const key of resolved.required) {
      if (!(key in value)) throw new Error(`${at}.${key} is missing`);
    }
    for (const [key, item] of Object.entries(value)) {
      if (!(key in resolved.properties)) throw new Error(`${at}.${key} is not a field`);
      conforms(item, resolved.properties[key], `${at}.${key}`);
    }
  } else if (type === "object" && resolved.additionalProperties) {
    for (const [key, item] of Object.entries(value)) {
      conforms(item, resolved.additionalProperties, `${at}.${key}`);
    }
  }
}

function sample(sample, what) {
  if (sample.calls_us.length !== 10)
    throw new Error(`incomplete ${what}: ${sample.calls_us.length} of 10 calls`);
  if (sample.calls_us.some((us) => us <= 0))
    throw new Error(`incomplete ${what}: a call took no time`);
  const sorted = [...sample.calls_us].sort((a, b) => a - b);
  if (sample.max_us !== sorted[9] || sample.median_us !== Math.floor((sorted[4] + sorted[5]) / 2)) {
    throw new Error(`${what}: its median or maximum is not its calls'`);
  }
}

/** The validated current-gate record, or an error naming why it is not one. */
export function readRecord(directory = recordDirectory, sourceRoot = repositoryRoot) {
  const schema = loadedSchema();
  const read = (name) => {
    let text;
    try {
      text = readFileSync(`${directory}/${name}`, "utf8");
    } catch (error) {
      throw new Error(
        error.code === "ENOENT" ? `${name} is missing` : `${name} is unreadable: ${error.message}`,
      );
    }
    try {
      return JSON.parse(text);
    } catch (error) {
      throw new Error(`${name} is unreadable: ${error.message}`);
    }
  };
  const invocation = read(schema["x-invocation"]);
  if (invocation.state !== "complete" || !digest(invocation.run_id)) {
    throw new Error("the producing invocation is missing or incomplete");
  }
  const record = read(schema["x-record"]);
  try {
    conforms(record, schema, "record");
  } catch (error) {
    throw new Error(`incomplete telemetry: ${error.message}`);
  }
  if (record.version !== schema["x-version"]) throw new Error("foreign telemetry version");
  if (record.load_workers !== 0) throw new Error("a load run's record, which no budget reads");
  const build = record.build;
  if (
    build.run_id !== invocation.run_id ||
    build.binary !== invocation.binary ||
    build.binary_sha256 !== invocation.binary_sha256 ||
    build.source_sha256 !== invocation.source_sha256
  ) {
    throw new Error(
      "stale or foreign invocation: the record and the invocation that produced it disagree",
    );
  }
  if (build.profile !== "release") throw new Error("the record measured a non-release binary");
  if (!digest(build.source_sha256) || build.source_sha256 !== sourceFingerprint(sourceRoot)) {
    throw new Error("stale record: the build inputs changed since it was produced");
  }
  // The name is checked before it is used: it is the record's to state, and only the
  // one file the producer writes beside it is ever read.
  if (build.binary !== schema["x-binary"]) {
    throw new Error(
      `the record names binary ${JSON.stringify(build.binary)}, not ${schema["x-binary"]}`,
    );
  }
  let binary;
  try {
    binary = readFileSync(`${directory}/${schema["x-binary"]}`);
  } catch (error) {
    throw new Error(
      error.code === "ENOENT"
        ? "the producing binary is missing beside its record"
        : `the producing binary beside its record is unreadable: ${error.message}`,
    );
  }
  if (sha(binary) !== build.binary_sha256) {
    throw new Error("the binary beside the record is not the one that produced it");
  }
  if (record.preparation_ms <= 0 || record.total_ms < record.preparation_ms) {
    throw new Error("incomplete preparation/total timing");
  }
  const scales = record.workloads.map((workload) => workload.scale);
  if (scales.join(",") !== "1,10") throw new Error(`foreign workloads: scales ${scales.join(",")}`);
  for (const workload of record.workloads) {
    const shape = schema["x-workloads"][String(workload.scale)];
    if (workload.shape.join(",") !== shape.join(","))
      throw new Error(`foreign workload at scale ${workload.scale}`);
    const classes = Object.keys(workload.class_counts).sort().join(",");
    if (
      classes !== "in-part,landed,live,no,retirable,superseded,unknown" ||
      Object.values(workload.class_counts).some((n) => n <= 0)
    ) {
      throw new Error(`scale ${workload.scale} is missing a recovery class`);
    }
    if (workload.own_runs <= 0) throw new Error(`scale ${workload.scale} measured no own runs`);
    const names = workload.scenarios.map((scenario) => scenario.scenario).join(",");
    if (names !== schema["x-scenarios"].join(","))
      throw new Error(`scale ${workload.scale} scenarios are ${names}`);
    for (const scenario of workload.scenarios) {
      const what = `scale ${workload.scale} ${scenario.scenario}`;
      if (!digest(scenario.verdict_sha256)) throw new Error(`${what} carries no verdict`);
      sample(scenario.warm, `${what} warm`);
      if (workload.scale === 1) {
        if (scenario.cold === null || scenario.cold_git === null)
          throw new Error(`${what} has no cold sample`);
        sample(scenario.cold, `${what} cold`);
      } else if (scenario.cold !== null || scenario.cold_git !== null) {
        throw new Error(`${what} carries a cold sample no budget reads`);
      }
      if (scenario.scenario !== "unreadable" && scenario.warm_git + (scenario.cold_git ?? 0) <= 0) {
        throw new Error(`${what} counted no git`);
      }
    }
  }
  return record;
}

/** The slowest scenario's maximum of one mode at one scale, in seconds. */
export function slowest(record, scale, mode) {
  const workload = record.workloads.find((row) => row.scale === scale);
  let worst = null;
  for (const scenario of workload.scenarios) {
    const max = scenario[mode].max_us;
    if (worst === null || max > worst.max) worst = { max, scenario };
  }
  return worst;
}
