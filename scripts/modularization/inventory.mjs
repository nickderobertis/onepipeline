import { readFileSync, readdirSync } from "node:fs";
import { join } from "node:path";

function files(root) {
  return readdirSync(root, { withFileTypes: true }).flatMap((entry) => {
    const path = join(root, entry.name);
    return entry.isDirectory() ? files(path) : [path];
  });
}

export function sourceAssets(root = ".") {
  return files(join(root, "src"))
    .filter((path) => !path.endsWith(".rs"))
    .map((path) => path.slice(root === "." ? 0 : root.length + 1).replaceAll("\\", "/"))
    .sort();
}

const record = (value) => value !== null && typeof value === "object" && !Array.isArray(value);
const text = (value) => typeof value === "string" && value.trim().length > 0;

function inputShape(input, file) {
  if (text(input)) return;
  if (record(input)) {
    const keys = Object.keys(input);
    if (keys.length === 1 && ["env", "runtime"].includes(keys[0]) && text(input[keys[0]])) return;
  }
  throw new Error(`unsupported auxiliary file input in ${file}`);
}

/** Auxiliary Rust and live journeys come from their actual Nx projects, including live/workload tiers. */
export function auxiliaryInputs(root = ".") {
  const config = JSON.parse(readFileSync(join(root, "nx.json"), "utf8"));
  if (!record(config) || !record(config.namedInputs))
    throw new Error("invalid namedInputs in nx.json");
  const named = config.namedInputs;
  for (const [name, inputs] of Object.entries(named)) {
    if (!text(name) || !Array.isArray(inputs))
      throw new Error(`invalid namedInputs entry: ${name}`);
    for (const input of inputs) inputShape(input, "nx.json");
  }
  const result = {};
  for (const file of files(join(root, "tests")).filter(
    (path) => path.endsWith("/project.json") || path.endsWith("\\project.json"),
  )) {
    const project = JSON.parse(readFileSync(file, "utf8"));
    if (
      !record(project) ||
      (project.tags !== undefined && (!Array.isArray(project.tags) || !project.tags.every(text))) ||
      (project.sourceRoot !== undefined && !text(project.sourceRoot)) ||
      (project.targets !== undefined && !record(project.targets))
    )
      throw new Error(`invalid auxiliary project in ${file}`);
    for (const target of Object.values(project.targets ?? {})) {
      if (!record(target) || (target.inputs !== undefined && !Array.isArray(target.inputs)))
        throw new Error(`invalid auxiliary project target in ${file}`);
      for (const input of target.inputs ?? []) inputShape(input, file);
    }
    if (
      (!project.tags?.includes("lang:rust") && !project.tags?.includes("tier:live")) ||
      project.sourceRoot === "tests/e2e"
    )
      continue;
    if (!/^tests\/[a-z][a-z0-9_]*$/.test(project.sourceRoot))
      throw new Error(`unsupported journey sourceRoot: ${project.sourceRoot}`);
    const expand = (input, stack = []) => {
      if (typeof input !== "string") return [];
      if (Object.hasOwn(named, input)) {
        if (stack.includes(input))
          throw new Error(`auxiliary input cycle: ${[...stack, input].join(" -> ")}`);
        return named[input].flatMap((child) => expand(child, [...stack, input]));
      }
      const path = input
        .replaceAll("{workspaceRoot}/", "")
        .replaceAll("{projectRoot}", project.sourceRoot);
      if (path.includes("{") || path.startsWith("!") || path.startsWith("^"))
        throw new Error(`unsupported auxiliary file input: ${input}`);
      return [path];
    };
    result[project.sourceRoot.slice(6)] = [
      ...new Set([
        `${project.sourceRoot}/**/*`,
        ...Object.values(project.targets ?? {}).flatMap((target) =>
          (target.inputs ?? []).flatMap((input) => expand(input)),
        ),
      ]),
    ].sort();
  }
  return result;
}
