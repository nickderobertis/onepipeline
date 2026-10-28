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

/** Auxiliary Rust and live journeys come from their actual Nx projects, including live/workload tiers. */
export function auxiliaryInputs(root = ".") {
  const named = JSON.parse(readFileSync(join(root, "nx.json"), "utf8")).namedInputs;
  const result = {};
  for (const file of files(join(root, "tests")).filter(
    (path) => path.endsWith("/project.json") || path.endsWith("\\project.json"),
  )) {
    const project = JSON.parse(readFileSync(file, "utf8"));
    if (
      (!project.tags?.includes("lang:rust") && !project.tags?.includes("tier:live")) ||
      project.sourceRoot === "tests/e2e"
    )
      continue;
    if (!/^tests\/[a-z][a-z0-9_]*$/.test(project.sourceRoot))
      throw new Error(`unsupported journey sourceRoot: ${project.sourceRoot}`);
    const expand = (input, stack = []) => {
      if (typeof input !== "string") {
        if (input && (Object.hasOwn(input, "env") || Object.hasOwn(input, "runtime"))) return [];
        throw new Error(`unsupported auxiliary file input in ${file}`);
      }
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
        ...Object.values(project.targets).flatMap((target) =>
          (target.inputs ?? []).flatMap((input) => expand(input)),
        ),
      ]),
    ].sort();
  }
  return result;
}
