// The coverage tiers `onepipeline:test` merges, read from the project graph.
//
//   node scripts/coverage-tiers.mjs
//
// A tier is any target whose Nx `outputs` name a `target/coverage-units/<tier>.lcov`, or whose
// command is a recipe that runs `_tier-test`. Prints every tier's unit name, space-separated.
// Reads every `project.json` git tracks and the `justfile`.
//
// Exit: 0 when `onepipeline:test` depends on every tier and every tier declares its report. 2
// when one does not, naming it: a tier the floor would not count cannot reach the total in
// silence, and a graph that cannot be read names what stopped it.
import { execFileSync } from "node:child_process";
import { readFileSync } from "node:fs";

const refuse = (message) => {
  console.error(`coverage-tiers: ${message}`);
  process.exit(2);
};

let tracked;
try {
  tracked = execFileSync("git", ["ls-files", "-z"], {
    encoding: "utf8",
    maxBuffer: 64 * 1024 * 1024,
  })
    .split("\0")
    .filter((path) => path === "project.json" || path.endsWith("/project.json"));
} catch (error) {
  refuse(
    `cannot list the project files: ${error.message}. ACTION: run this inside the repository's checkout`,
  );
}

const justfile = readFileSync("justfile", "utf8");
// `_e2e-test: _strace-preflight (_tier-test "onepipeline-e2e" e2e-tier)` -> the recipe's unit.
const unitOfRecipe = new Map(
  [...justfile.matchAll(/^(_[\w-]+):.*\(_tier-test "([\w-]+)"/gm)].map(([, recipe, unit]) => [
    recipe,
    unit,
  ]),
);

const REPORT = /^\{workspaceRoot\}\/target\/coverage-units\/([\w-]+)\.lcov$/;
const tiers = new Map();
let aggregate;
for (const path of tracked) {
  let project;
  try {
    project = JSON.parse(readFileSync(path, "utf8"));
  } catch (error) {
    refuse(`${path} cannot be read: ${error.message}. ACTION: repair that project file`);
  }
  if (project.name === "onepipeline") aggregate = project.targets?.test;
  for (const [target, definition] of Object.entries(project.targets ?? {})) {
    const id = `${project.name}:${target}`;
    const recipe = String(definition.command ?? "").match(/^just (\S+)/)?.[1];
    const declared = (definition.outputs ?? [])
      .map((output) => output.match(REPORT)?.[1])
      .filter(Boolean);
    const unit = declared[0] ?? unitOfRecipe.get(recipe);
    if (unit === undefined) continue;
    if (declared.length === 0) {
      refuse(
        `${id} runs coverage unit ${unit} but declares no target/coverage-units/${unit}.lcov output. ACTION: add it to that target's outputs`,
      );
    }
    if (tiers.has(unit))
      refuse(
        `two targets write coverage unit ${unit}: ${tiers.get(unit)} and ${id}. ACTION: give each tier its own unit name`,
      );
    tiers.set(unit, id);
  }
}

if (aggregate === undefined)
  refuse(
    "no project named onepipeline declares a test target. ACTION: restore the aggregate target",
  );
if (tiers.size === 0)
  refuse(
    "the project graph holds no coverage tier. ACTION: restore the tiers' target/coverage-units outputs",
  );
const dependedOn = new Set(
  (aggregate.dependsOn ?? []).flatMap((need) =>
    typeof need === "string"
      ? [`onepipeline:${need}`]
      : (need.projects ?? []).map((name) => `${name}:${need.target}`),
  ),
);
const uncounted = [...tiers]
  .filter(([, id]) => !dependedOn.has(id))
  .map(([unit, id]) => `${unit} (${id})`);
if (uncounted.length) {
  refuse(
    `onepipeline:test does not depend on coverage tier(s): ${uncounted.sort().join(", ")}. ACTION: add each to that target's dependsOn so the floor counts it`,
  );
}
console.log([...tiers.keys()].sort().join(" "));
