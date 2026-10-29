import { execFileSync } from "node:child_process";
import { readFileSync, writeFileSync } from "node:fs";
import { pathToFileURL } from "node:url";
import { cargoManifest } from "./manifest-projection.mjs";

const justCache = new Map();
const record = (value) => value !== null && typeof value === "object" && !Array.isArray(value);

/** Parse with just itself; discard documentation, retaining executable syntax. */
export function configSections(file, text) {
  if (file !== "justfile") {
    const parsed = cargoManifest(text);
    const { profile, ...sections } = parsed;
    if (profile !== undefined && !record(profile))
      throw new Error("invalid configuration profile: expected a table");
    for (const [name, value] of Object.entries(profile ?? {})) {
      if (!record(value))
        throw new Error(`invalid configuration profile ${name}: expected a table`);
      sections[`profile.${name}`] = value;
    }
    return sections;
  }
  if (!justCache.has(text)) {
    const parsed = JSON.parse(
      execFileSync("just", ["--dump", "--dump-format", "json", "--justfile", "-"], {
        input: text,
        encoding: "utf8",
        stdio: ["pipe", "pipe", "pipe"],
      }),
    );
    const sections = {
      globals: {
        settings: parsed.settings,
        unexports: parsed.unexports,
        aliases: parsed.aliases,
        modules: parsed.modules,
      },
    };
    for (const [name, assignment] of Object.entries(parsed.assignments))
      sections[`variable:${name}`] = assignment;
    for (const [name, { doc: _doc, ...recipe }] of Object.entries(parsed.recipes))
      sections[`recipe:${name}`] = recipe;
    justCache.set(text, sections);
  }
  return justCache.get(text);
}

/** The existing e2e project's actual test commands and archive-build dependencies are authoritative. */
export function runnerEntries() {
  const projects = ["project.json", "tests/e2e/project.json"].map((file) =>
    JSON.parse(readFileSync(file, "utf8")),
  );
  if (
    projects.some(
      (project) =>
        !record(project) ||
        typeof project.name !== "string" ||
        !project.name.trim() ||
        !record(project.targets),
    ) ||
    new Set(projects.map((project) => project.name)).size !== projects.length
  )
    throw new Error("invalid runner project metadata or duplicate project name");
  const byName = Object.fromEntries(projects.map((project) => [project.name, project]));
  const reached = new Set();
  const recipes = new Set();
  const visit = (name, target) => {
    const key = `${name}:${target}`;
    if (reached.has(key)) return;
    reached.add(key);
    if (!Object.hasOwn(byName, name) || !Object.hasOwn(byName[name].targets, target))
      throw new Error(`missing runner target ${key}`);
    const definition = byName[name].targets[target];
    if (!record(definition)) throw new Error(`invalid runner target ${key}`);
    if (
      (definition.command !== undefined && typeof definition.command !== "string") ||
      (definition.executor !== undefined && typeof definition.executor !== "string")
    )
      throw new Error(`invalid runner command metadata ${key}`);
    if (definition.executor !== undefined && definition.executor !== "nx:noop")
      throw new Error(`unsupported runner executor ${key}: ${definition.executor}`);
    if (definition.executor === "nx:noop" && definition.command !== undefined)
      throw new Error(`invalid runner command on noop target ${key}`);
    if (definition.dependsOn !== undefined && !Array.isArray(definition.dependsOn))
      throw new Error(`invalid runner dependency list ${key}`);
    const recipe = /^just\s+([\w-]+)\s*$/.exec(definition.command ?? "");
    if (recipe) recipes.add(recipe[1]);
    else if (definition.executor !== "nx:noop")
      throw new Error(
        `unsupported runner command ${key}: ${definition.command ?? definition.executor}`,
      );
    for (const dependency of definition.dependsOn ?? []) {
      if (typeof dependency === "string") {
        if (!/^[A-Za-z0-9_-]+$/.test(dependency))
          throw new Error(`unsupported runner dependency ${key}: ${dependency}`);
        visit(name, dependency);
      } else {
        if (
          !record(dependency) ||
          Object.keys(dependency).some((field) => !["target", "projects"].includes(field)) ||
          typeof dependency.target !== "string" ||
          !/^[A-Za-z0-9_-]+$/.test(dependency.target)
        )
          throw new Error(`invalid runner dependency ${key}`);
        const selected = dependency.projects === undefined ? [name] : dependency.projects;
        if (
          !Array.isArray(selected) ||
          !selected.length ||
          selected.some((project) => typeof project !== "string" || !Object.hasOwn(byName, project))
        )
          throw new Error(`unsupported runner dependency projects ${key}`);
        for (const project of selected) visit(project, dependency.target);
      }
    }
  };
  for (const target of ["test", "test-quick"]) visit(projects[1].name, target);
  return [...recipes];
}

export function testRecipes(text, entries = runnerEntries()) {
  const sections = configSections("justfile", text);
  for (const name of entries)
    if (!Object.hasOwn(sections, `recipe:${name}`))
      throw new Error(`missing runner recipe ${name}`);
  const reached = new Set(
    [
      "globals",
      ...entries.map((name) => `recipe:${name}`),
      ...Object.keys(sections).filter((key) => sections[key].export),
    ].filter((key) => Object.hasOwn(sections, key)),
  );
  const variables = (value) => {
    if (!value || typeof value !== "object") return [];
    if (Array.isArray(value) && value[0] === "variable") return [value[1]];
    return Object.values(value).flatMap(variables);
  };
  for (const key of reached) {
    const section = sections[key];
    const references = [
      ...variables(section).map((name) => `variable:${name}`),
      ...(section.dependencies ?? []).map((dependency) => `recipe:${dependency.recipe}`),
    ];
    for (const line of section.body ?? [])
      for (const fragment of line.filter((value) => typeof value === "string"))
        for (const match of fragment.matchAll(
          /(?:^|[;\n]|&&|\|\||\$\()\s*@?just\s+(?:(--evaluate)\s+)?([\w-]+)/g,
        ))
          references.push(`${match[1] ? "variable" : "recipe"}:${match[2]}`);
    for (const reference of references)
      if (Object.hasOwn(sections, reference)) reached.add(reference);
  }
  return reached;
}

export function nextestProfiles(configs = []) {
  const sections = configSections("justfile", readFileSync("justfile", "utf8"));
  const bodies = [...testRecipes(readFileSync("justfile", "utf8"))]
    .map((key) => JSON.stringify(sections[key]))
    .join("\n");
  const profiles = new Set(["profile.default"]);
  for (const match of bodies.matchAll(/--profile(?:=|\s+)([a-z][\w-]*)/g))
    profiles.add(`profile.${match[1]}`);
  const workflow = readFileSync(".github/workflows/ci.yml", "utf8");
  for (const match of workflow.matchAll(/^\s*NEXTEST_PROFILE:\s*([a-z][\w-]*)\s*$/gm))
    profiles.add(`profile.${match[1]}`);
  for (const config of configs) {
    const visit = (key, chain = []) => {
      if (chain.includes(key))
        throw new Error(`nextest profile inheritance cycle: ${[...chain, key].join(" -> ")}`);
      const parent = config[key]?.inherits;
      if (parent === undefined) return;
      if (
        typeof parent !== "string" ||
        !/^[a-z][\w-]*$/.test(parent) ||
        (parent !== "default" && !Object.hasOwn(config, `profile.${parent}`))
      )
        throw new Error(`invalid nextest inherited profile ${key}`);
      profiles.add(`profile.${parent}`);
      visit(`profile.${parent}`, [...chain, key]);
    };
    for (const key of [...profiles]) visit(key);
  }
  return profiles;
}

export function configChanges(file, before, after, suites, recipeEntries) {
  const a = configSections(file, before);
  const b = configSections(file, after);
  const changed = Object.keys({ ...a, ...b }).filter(
    (key) => JSON.stringify(a[key]) !== JSON.stringify(b[key]),
  );
  const recipes =
    file === "justfile"
      ? new Set([
          ...testRecipes(before, recipeEntries?.before),
          ...testRecipes(after, recipeEntries?.after),
        ])
      : null;
  const profiles = file === ".config/nextest.toml" ? nextestProfiles([a, b]) : null;
  const consumed = changed.filter((key) => {
    if (recipes) return recipes.has(key);
    // Cargo aliases are developer shortcuts, not cargo nextest/build inputs.
    if (file === ".cargo/config.toml")
      return (
        key !== "alias" &&
        (!key.startsWith("profile.") || ["profile.dev", "profile.test"].includes(key))
      );
    return profiles.has(key) || ["test-groups", "store"].includes(key);
  });
  return consumed.length ? suites.map((suite) => `migrated-tests/${suite}/config/${file}`) : [];
}

/** Historical snapshots predate some modern runner templates. Absent templates have no historical body input. */
export function historicalConfigChanges(file, before, after, suites) {
  if (file !== "justfile") return configChanges(file, before, after, suites);
  const roots = runnerEntries();
  const existing = (text) =>
    roots.filter((name) => Object.hasOwn(configSections(file, text), `recipe:${name}`));
  return configChanges(file, before, after, suites, {
    before: existing(before),
    after: existing(after),
  });
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  try {
    if (
      process.argv.length !== 7 ||
      !["justfile", ".cargo/config.toml", ".config/nextest.toml"].includes(process.argv[2])
    )
      throw new Error("usage: config-projection.mjs FILE BEFORE AFTER SUITES.json OUTPUT.json");
    const suites = JSON.parse(readFileSync(process.argv[5], "utf8"));
    if (
      !Array.isArray(suites) ||
      !suites.length ||
      suites.some((name) => typeof name !== "string" || !/^[a-z][a-z0-9-]*$/.test(name))
    )
      throw new Error("suites must be nonempty project names");
    const result = configChanges(
      process.argv[2],
      readFileSync(process.argv[3], "utf8"),
      readFileSync(process.argv[4], "utf8"),
      suites,
    );
    writeFileSync(process.argv[6], JSON.stringify(result));
  } catch (error) {
    const next =
      error.code && /^(?:EACCES|EPERM|ENOENT|EISDIR|ENOTDIR|ENOSPC|EROFS)$/.test(error.code)
        ? "check the named path's type, existence, permissions and disk space, and that the output parent is writable; rerun the same config-projection.mjs invocation"
        : "correct the named configuration or suite-list input; rerun the same config-projection.mjs invocation with all paths";
    console.error(`${error.message}\nnext: ${next}`);
    process.exitCode = 1;
  }
}
