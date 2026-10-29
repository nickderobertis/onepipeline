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

/** Every reference the projection reads resolves to a known input, or the projection stops here naming it. */
export function unresolved(kind, reference, where) {
  return new Error(`unresolved ${kind} ${reference} (read from ${where})`);
}

const runnerProjects = ["project.json", "tests/e2e/project.json"];
// The target fields this projection models: command, executor and dependsOn are interpreted below; inputs,
// outputs, parallelism and cache are Nx scheduling metadata. Any other field (options, a custom executor's
// settings) is unresolved.
const runnerTargetFields = [
  "command",
  "executor",
  "dependsOn",
  "inputs",
  "outputs",
  "parallelism",
  "cache",
];

/** The existing e2e project's actual test commands and archive-build dependencies are authoritative. */
export function runnerEntries() {
  const projects = runnerProjects.map((file) => JSON.parse(readFileSync(file, "utf8")));
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
  const fileOf = Object.fromEntries(
    projects.map((project, index) => [project.name, runnerProjects[index]]),
  );
  const reached = new Set();
  const recipes = new Map();
  const visit = (name, target, from) => {
    const key = `${name}:${target}`;
    if (reached.has(key)) return;
    reached.add(key);
    if (!Object.hasOwn(byName, name) || !Object.hasOwn(byName[name].targets, target))
      throw unresolved("Nx target", key, from);
    const where = `${fileOf[name]} targets.${target}`;
    const definition = byName[name].targets[target];
    if (!record(definition)) throw new Error(`invalid runner target ${key}`);
    for (const field of Object.keys(definition))
      if (!field.startsWith("//") && !runnerTargetFields.includes(field))
        throw unresolved("Nx target field", field, where);
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
    if (recipe) {
      if (!recipes.has(recipe[1])) recipes.set(recipe[1], `${where}.command`);
    } else if (definition.executor !== "nx:noop")
      throw unresolved(
        "runner command",
        JSON.stringify(definition.command ?? definition.executor ?? null),
        `${where}.command`,
      );
    for (const dependency of definition.dependsOn ?? []) {
      if (typeof dependency === "string") {
        if (!/^[A-Za-z0-9_-]+$/.test(dependency))
          throw new Error(`unsupported runner dependency ${key}: ${dependency}`);
        visit(name, dependency, `${where}.dependsOn`);
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
        for (const project of selected) visit(project, dependency.target, `${where}.dependsOn`);
      }
    }
  };
  for (const target of ["test", "test-quick"])
    visit(projects[1].name, target, `${runnerProjects[1]} runner roots`);
  return [...recipes].map(([name, from]) => ({ name, from }));
}

/** Rendered body lines, with interpolations kept visibly unresolved rather than dropped. */
const bodyLines = (section) =>
  (section.body ?? []).map((line) =>
    line.map((fragment) => (typeof fragment === "string" ? fragment : "{{…}}")).join(""),
  );

const unsupportedSettings = {
  dotenv_load: (value) => value === true,
  dotenv_filename: (value) => value !== null,
  dotenv_path: (value) => value !== null,
  dotenv_command: (value) => Array.isArray(value) && value.length > 0,
  fallback: (value) => value === true,
};

export function testRecipes(text, entries = runnerEntries()) {
  const sections = configSections("justfile", text);
  for (const { name, from } of entries)
    if (!Object.hasOwn(sections, `recipe:${name}`)) throw unresolved("just recipe", name, from);
  const { settings = {} } = sections.globals;
  for (const [setting, active] of Object.entries(unsupportedSettings))
    if (active(settings[setting]))
      throw unresolved(
        "just setting",
        setting,
        "justfile settings (reads an input outside this file)",
      );
  // A module needs its source file beside the justfile, which a projected snapshot read from stdin never has, so
  // just itself refuses one while parsing; there is no module section here to resolve.
  const reached = new Set([
    "globals",
    ...entries.map(({ name }) => `recipe:${name}`),
    ...Object.keys(sections).filter(
      (key) => key.startsWith("variable:") && (settings.export || sections[key].export),
    ),
  ]);
  const expressions = (value, found = []) => {
    if (!value || typeof value !== "object") return found;
    if (
      Array.isArray(value) &&
      value.length >= 2 &&
      value[0] === "call" &&
      value[1] === "just_executable"
    )
      found.push(["invocation", "just_executable()"]);
    if (
      Array.isArray(value) &&
      value.length === 2 &&
      value[0] === "variable" &&
      typeof value[1] === "string"
    )
      found.push(["variable", value[1]]);
    else for (const item of Object.values(value)) expressions(item, found);
    return found;
  };
  for (const key of reached) {
    if (key === "globals") continue;
    const section = sections[key];
    const parameters = new Set((section.parameters ?? []).map((parameter) => parameter.name));
    const references = [];
    for (const [kind, name] of expressions(section)) {
      if (kind === "invocation") throw unresolved("just invocation", name, `justfile ${key}`);
      if (!parameters.has(name)) references.push(["just variable", `variable:${name}`]);
    }
    for (const dependency of section.dependencies ?? [])
      references.push(["just recipe", `recipe:${dependency.recipe}`]);
    for (const line of bodyLines(section))
      for (const match of line.matchAll(/(?:^|[;\n]|&&|\|\||\$\()\s*@?just(?=\s|$)([^;&|)\n]*)/g)) {
        const invocation = /^\s+(?:(--evaluate)\s+)?([A-Za-z_][\w-]*)(?:\s|$)/.exec(match[1]);
        if (!invocation || /\{\{…\}\}/.test(invocation[0]))
          throw unresolved(
            "just invocation",
            JSON.stringify(`just${match[1]}`.trim()),
            `justfile ${key} body`,
          );
        references.push(
          invocation[1]
            ? ["just variable", `variable:${invocation[2]}`]
            : ["just recipe", `recipe:${invocation[2]}`],
        );
      }
    for (const [kind, reference] of references) {
      if (!Object.hasOwn(sections, reference))
        throw unresolved(kind, reference.slice(reference.indexOf(":") + 1), `justfile ${key}`);
      reached.add(reference);
    }
  }
  return reached;
}

/** Profiles the runners select: literal `--profile`/`NEXTEST_PROFILE` values, then their `inherits` chains. */
export function nextestProfiles(configs = []) {
  const text = readFileSync("justfile", "utf8");
  const sections = configSections("justfile", text);
  const referenced = new Map([["default", "nextest's built-in default profile"]]);
  const literal = (value, where) => {
    if (!/^[a-z][\w-]*$/.test(value))
      throw unresolved("nextest profile", JSON.stringify(value), where);
    if (!referenced.has(value)) referenced.set(value, where);
  };
  for (const key of testRecipes(text)) {
    for (const line of bodyLines(sections[key])) {
      for (const match of line.matchAll(/--cargo-profile\b|--release\b/g))
        throw unresolved("cargo profile selection", match[0], `justfile ${key} body`);
      for (const match of line.matchAll(/(?:--profile|\bNEXTEST_PROFILE)(?:=|\s+)(\S*)/g))
        literal(match[1].replace(/^["']|["']$/g, ""), `justfile ${key} body`);
    }
  }
  const workflow = readFileSync(".github/workflows/ci.yml", "utf8");
  for (const [index, line] of workflow.split("\n").entries()) {
    const match = /^\s*NEXTEST_PROFILE:\s*(.*?)\s*$/.exec(line);
    if (match)
      literal(match[1].replace(/^["']|["']$/g, ""), `.github/workflows/ci.yml:${index + 1}`);
  }
  const profiles = new Set();
  const visit = (key, from, chain = []) => {
    if (chain.includes(key))
      throw new Error(`nextest profile inheritance cycle: ${[...chain, key].join(" -> ")}`);
    profiles.add(key);
    if (
      key !== "profile.default" &&
      configs.length &&
      !configs.some((config) => Object.hasOwn(config, key))
    )
      throw unresolved("nextest profile", key.slice("profile.".length), from);
    for (const config of configs) {
      const parent = config[key]?.inherits;
      if (parent === undefined) continue;
      if (
        typeof parent !== "string" ||
        !/^[a-z][\w-]*$/.test(parent) ||
        (parent !== "default" &&
          !configs.some((other) => Object.hasOwn(other, `profile.${parent}`)))
      )
        throw new Error(`invalid nextest inherited profile ${key}`);
      visit(`profile.${parent}`, `.config/nextest.toml ${key}.inherits`, [...chain, key]);
    }
  };
  for (const [name, from] of referenced) visit(`profile.${name}`, from);
  return profiles;
}

// Every top-level section each file may carry, and whether a change to it reaches the runners.
// A section absent here is unresolved: the projection refuses it rather than guessing either way.
const cargoSections = {
  build: true,
  target: true,
  env: true,
  source: true,
  registries: true,
  registry: true,
  net: true,
  http: true,
  patch: true,
  resolver: true,
  unstable: true,
  // Developer shortcuts and presentation: no runner invokes an alias, and none changes what is compiled or tested.
  alias: false,
  term: false,
  doc: false,
  "cargo-new": false,
  install: false,
  "credential-alias": false,
  "future-incompat-report": false,
};
const nextestSections = {
  "test-groups": true,
  store: true,
  experimental: true,
  "nextest-version": true,
};

// nextest's profile script entry: which tests (filter, platform) run which setup scripts or wrapper scripts.
const scriptEntryFields = ["filter", "platform", "setup", "list-wrapper", "run-wrapper"];
const scriptKinds = { setup: "setup", "list-wrapper": "wrapper", "run-wrapper": "wrapper" };

function nextestScripts(configs, profiles) {
  const used = new Map();
  for (const config of configs)
    for (const key of profiles) {
      const entries = config[key]?.scripts ?? [];
      if (!Array.isArray(entries))
        throw new Error(`invalid nextest ${key}.scripts: expected an array of script entries`);
      for (const [index, entry] of entries.entries()) {
        const from = `.config/nextest.toml ${key}.scripts[${index}]`;
        if (!record(entry)) throw new Error(`invalid nextest script entry (read from ${from})`);
        for (const field of Object.keys(entry))
          if (!scriptEntryFields.includes(field))
            throw unresolved("nextest script field", field, from);
        for (const [field, kind] of Object.entries(scriptKinds))
          for (const name of [entry[field] ?? []].flat())
            used.set(`${kind}:${name}`, { kind, name, from });
      }
    }
  for (const { kind, name, from } of used.values())
    if (
      typeof name !== "string" ||
      !configs.some((config) => Object.hasOwn(config.scripts?.[kind] ?? {}, name))
    )
      throw unresolved("nextest script", JSON.stringify(name), from);
  return used;
}

export function configChanges(file, before, after, suites, recipeEntries) {
  if (!["justfile", ".cargo/config.toml", ".config/nextest.toml"].includes(file))
    throw unresolved("configuration file", file, "the projected path");
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
  // Every section either side carries must be one this file may hold, in that section's shape, whether or not it
  // changed: an unknown or malformed section is unresolved even when both snapshots agree on it.
  if (file !== "justfile")
    for (const [key, value] of [...Object.entries(a), ...Object.entries(b)]) {
      if (key.startsWith("profile.")) continue;
      const known = file === ".cargo/config.toml" ? cargoSections : nextestSections;
      if (!Object.hasOwn(known, key) && !(file === ".config/nextest.toml" && key === "scripts"))
        throw unresolved("configuration section", key, file);
      const shaped =
        file === ".config/nextest.toml" && key === "nextest-version"
          ? typeof value === "string" || record(value)
          : file === ".config/nextest.toml" && key === "experimental"
            ? Array.isArray(value) || record(value)
            : record(value);
      if (!shaped)
        throw new Error(`invalid configuration section ${key} in ${file}: unexpected shape`);
    }
  const profiles = file === ".config/nextest.toml" ? nextestProfiles([a, b]) : null;
  const scripts = profiles ? nextestScripts([a, b], profiles) : null;
  // Runners build with Cargo's dev/test profiles. The same runner-body scan that resolves nextest profiles refuses
  // a runner selecting any other (--release, --cargo-profile), so run it for Cargo configuration too.
  if (file === ".cargo/config.toml") nextestProfiles([]);
  const consumed = changed.filter((key) => {
    if (recipes) return recipes.has(key);
    if (file === ".cargo/config.toml") {
      if (key.startsWith("profile.")) return ["profile.dev", "profile.test"].includes(key);
      return cargoSections[key];
    }
    if (key.startsWith("profile.")) return profiles.has(key);
    if (key === "scripts")
      return [...scripts.values()].some(
        ({ kind, name }) =>
          JSON.stringify(a.scripts?.[kind]?.[name]) !== JSON.stringify(b.scripts?.[kind]?.[name]),
      );
    return nextestSections[key];
  });
  return consumed.length ? suites.map((suite) => `migrated-tests/${suite}/config/${file}`) : [];
}

/** Historical snapshots predate some modern runner templates. Absent templates have no historical body input. */
export function historicalConfigChanges(file, before, after, suites) {
  if (file !== "justfile") return configChanges(file, before, after, suites);
  const roots = runnerEntries();
  const existing = (text) =>
    roots.filter(({ name }) => Object.hasOwn(configSections(file, text), `recipe:${name}`));
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
