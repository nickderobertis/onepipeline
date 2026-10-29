import { execFileSync } from "node:child_process";
import { readFileSync } from "node:fs";
import { codeOnly } from "./graph.mjs";

const kebab = (word) =>
  word.replace(/[a-z0-9][A-Z]/g, (pair) => `${pair[0]}-${pair[1]}`).toLowerCase();

/** Balanced Rust declaration ranges, with literals erased so fixture text cannot close a body. */
export function declarations(text) {
  const code = codeOnly(text);
  const breaks = [0];
  for (let offset = 0; offset < code.length; offset++)
    if (code[offset] === "\n") breaks.push(offset + 1);
  const lineAt = (offset) => {
    let low = 0;
    let high = breaks.length;
    while (low < high) {
      const middle = (low + high) >>> 1;
      if (breaks[middle] <= offset) low = middle + 1;
      else high = middle;
    }
    return low;
  };
  return [...code.matchAll(/\b(fn|struct|enum|const|type)\s+(\w+)/g)]
    .filter((m) => m[2] !== "fn")
    .map((m) => {
      const brace = code.indexOf("{", m.index);
      const semi = code.indexOf(";", m.index);
      let end;
      if (semi >= 0 && (brace < 0 || semi < brace)) end = semi + 1;
      else if (brace >= 0) {
        end = brace + 1;
        let depth = 1;
        while (end < code.length && depth) {
          if (code[end] === "{") depth++;
          if (code[end] === "}") depth--;
          end++;
        }
      } else end = code.length;
      return {
        offset: m.index,
        kind: m[1],
        private: !/pub(?:\([^)]*\))?\s*$/.test(
          text.slice(
            Math.max(text.lastIndexOf("}", m.index), text.lastIndexOf(";", m.index)) + 1,
            m.index,
          ),
        ),
        name: m[2],
        start: lineAt(m.index),
        end: lineAt(end),
        code: code.slice(m.index, end),
        raw: text.slice(m.index, end),
      };
    });
}

/** One architecture map reconciled against the actual nested clap enums. */
export function commandPaths(config) {
  if (
    config?.schema !== 1 ||
    Object.keys(config).some((key) => !["schema", "paths"].includes(key)) ||
    !config.paths ||
    typeof config.paths !== "object" ||
    Array.isArray(config.paths) ||
    !Object.keys(config.paths).length ||
    Object.entries(config.paths).some(
      ([path, owner]) =>
        !/^[a-z][a-z-]*(?: [a-z][a-z-]*)*$/.test(path) ||
        typeof owner !== "string" ||
        !/^cmd-[a-z][a-z0-9-]*$/.test(owner),
    )
  )
    throw new Error("invalid CLI command mapping");
  return config.paths;
}

export function cliPaths(cli, configured) {
  const enums = new Map(
    [...cli.matchAll(/pub enum (\w+Command|Command) \{([\s\S]*?)^\}/gm)].map((m) => [m[1], m[2]]),
  );
  if (!enums.has("Command")) throw new Error("missing CLI Command enum");
  const paths = [];
  for (const variant of enums.get("Command").matchAll(/^ {4}([A-Z]\w*)(?:\((\w+)\))?[, ]/gm)) {
    const parent = kebab(variant[1]);
    const children = enums.get(variant[2]);
    if (children)
      for (const child of children.matchAll(/^ {4}([A-Z]\w*)(?:\(| \{)/gm))
        paths.push(`${parent} ${kebab(child[1])}`);
    else paths.push(parent);
  }
  if (JSON.stringify([...paths].sort()) !== JSON.stringify(Object.keys(configured).sort()))
    throw new Error("CLI path mapping differs from src/cli.rs");
  return enums;
}

/** Each enum variant, including its clap attributes, belongs to its command. */
export function cliVariants(text) {
  const result = [];
  for (const decl of declarations(text).filter((entry) => entry.kind === "enum")) {
    const lines = text.split("\n");
    let start = decl.start + 1;
    for (let line = start; line < decl.end; line++) {
      const match = /^ {4}([A-Z]\w*)(?:\(|\s*\{|,)/.exec(lines[line - 1]);
      if (!match) continue;
      const next = lines
        .slice(line, decl.end - 1)
        .findIndex((value) => /^ {4}(?:[A-Z]\w*(?:\(|\s*\{|,)|#|\/\/)/.test(value));
      const end = next < 0 ? decl.end - 1 : line + next;
      result.push({ name: `__variant_${decl.name}_${match[1]}`, start, end });
      start = end + 1;
    }
  }
  return result;
}

export function cliArgumentOwners(cli, commands) {
  const enums = cliPaths(cli, commands);
  const cliOwners = new Map([
    ["Cli", "onepipeline"],
    ["Command", "onepipeline"],
  ]);
  const typeUsers = new Map();
  const use = (type, owner) => {
    if (!typeUsers.has(type)) typeUsers.set(type, new Set());
    typeUsers.get(type).add(owner);
  };
  for (const [path, owner] of Object.entries(commands)) {
    const [parent, child] = path.split(" ");
    const outer = [...enums.get("Command").matchAll(/^ {4}([A-Z]\w*)(?:\((\w+)\))?[, ]/gm)].find(
      (m) => kebab(m[1]) === parent,
    );
    cliOwners.set(`__variant_Command_${outer[1]}`, child ? "onepipeline" : owner);
    if (!child) {
      if (outer[2]) use(outer[2], owner);
    } else {
      cliOwners.set(outer[2], "onepipeline");
      const inner = [...enums.get(outer[2]).matchAll(/^ {4}([A-Z]\w*)(?:\((\w+)\)| \{)/gm)].find(
        (m) => kebab(m[1]) === child,
      );
      if (inner) cliOwners.set(`__variant_${outer[2]}_${inner[1]}`, owner);
      if (inner?.[2]) use(inner[2], owner);
    }
  }
  const types = declarations(cli).filter((entry) => ["struct", "enum"].includes(entry.kind));
  for (const [type, users] of typeUsers) {
    const definition = types.find((entry) => entry.name === type);
    if (definition)
      for (const nested of types)
        if (nested.name !== type && new RegExp(`\\b${nested.name}\\b`).test(definition.code))
          for (const owner of users) use(nested.name, owner);
  }
  for (const [type, users] of typeUsers)
    cliOwners.set(type, users.size === 1 ? [...users][0] : users);
  for (const type of types) if (!cliOwners.has(type.name)) cliOwners.set(type.name, "cli-schema");
  return cliOwners;
}

export function sourceOwners(layout, moves, catalog) {
  const cli = readFileSync("src/cli.rs", "utf8");
  const commands = commandPaths(
    JSON.parse(readFileSync("scripts/modularization/cli-commands.json", "utf8")),
  );
  const enums = cliPaths(cli, commands);
  const result = { "src/cli.rs": cliArgumentOwners(cli, commands) };
  const helpers = new Map();
  for (const entry of [
    ...moves.extractions,
    ...moves.edgeMoves,
    ...(moves.recordFiles ? [moves.recordFiles] : []),
    ...(moves.sharedSlices ?? []),
    ...(moves.commandSlices ?? []),
  ]) {
    if (!result[entry.file]) result[entry.file] = new Map();
    const owner = entry.shared ?? entry.replacement ?? entry.crate;
    const text = readFileSync(entry.file, "utf8");
    const decls = declarations(text);
    const reached = new Set(entry.symbols);
    for (const name of reached)
      for (const decl of decls.filter((d) => d.name === name))
        for (const candidate of decls)
          if (
            candidate.kind === "fn" &&
            candidate.private &&
            new RegExp(`\\b${candidate.name}\\b`).test(decl.code)
          )
            reached.add(candidate.name);
    for (const name of entry.symbols) {
      const existing = result[entry.file].get(name);
      if (existing && existing !== owner)
        throw new Error(`duplicate symbol owner ${entry.file}::${name}`);
      result[entry.file].set(name, owner);
    }
    for (const name of reached) {
      const key = `${entry.file}::${name}`;
      if (!helpers.has(key)) helpers.set(key, new Set());
      helpers.get(key).add(owner);
    }
  }
  for (const [key, users] of helpers) {
    const [file, name] = key.split("::");
    if (!result[file].has(name)) {
      const primary = Object.entries(layout.crates).find(([, crate]) =>
        crate.files.includes(file),
      )?.[0];
      result[file].set(name, users.size === 1 ? [...users][0] : primary);
    }
  }
  // Explicit subsystem item ownership supersedes old aggregate extractions.
  for (const [owner, entry] of Object.entries(layout.engineSubsystems?.subsystems ?? {}))
    for (const item of entry.items) {
      result[item.file] ??= new Map();
      for (const symbol of item.symbols) result[item.file].set(symbol, owner);
    }
  // The dispatch match is a routing facade; implementation functions remain with their concepts.
  result["src/driver.rs"] ??= new Map();
  result["src/driver.rs"].set("dispatch", "onepipeline");
  for (const variant of enums.get("Command").matchAll(/^ {4}([A-Z]\w*)/gm)) {
    const parent = kebab(variant[1]);
    result["src/driver.rs"].set(
      `__arm_${variant[1]}`,
      new Set(
        Object.entries(commands)
          .filter(([path]) => path === parent || path.startsWith(`${parent} `))
          .map(([, owner]) => owner),
      ),
    );
  }
  for (const [id, test] of Object.entries(catalog.tests)) {
    const [module, name] = (test.original ?? id).split("::");
    const file = `tests/e2e/${module}.rs`;
    result[file] ??= new Map();
    const destinations = result[file].get(name) ?? new Set();
    destinations.add(`suite:${test.suite}`);
    result[file].set(name, destinations);
  }
  return result;
}

/** Historical hunks are charged to both old and new declaration owners. Unknown lines retain the whole-file input. */
export function projectSourceChange(change, owners, layout, root = ".") {
  const git = (...args) =>
    execFileSync("git", args, {
      cwd: root,
      encoding: "utf8",
      maxBuffer: 32 * 1024 * 1024,
      stdio: ["ignore", "pipe", "pipe"],
    });
  const files = [];
  const projected = [];
  for (const file of change.files) {
    if (!owners[file]) {
      files.push(file);
      continue;
    }
    const snapshots = [null, null];
    for (const [index, rev] of [`${change.commit}^`, change.commit].entries()) {
      try {
        const text = git("show", `${rev}:${file}`);
        snapshots[index] = declarations(text);
        if (file === "src/cli.rs") snapshots[index].push(...cliVariants(text));
        if (file === "src/driver.rs") {
          const arms = [...codeOnly(text).matchAll(/^ {8}Verb::([A-Z]\w*)/gm)];
          for (const [position, arm] of arms.entries())
            snapshots[index].push({
              name: `__arm_${arm[1]}`,
              start: text.slice(0, arm.index).split("\n").length,
              end:
                position + 1 < arms.length
                  ? text.slice(0, arms[position + 1].index).split("\n").length - 1
                  : (snapshots[index].find((d) => d.name === "dispatch")?.end ?? 0),
            });
        }
      } catch (error) {
        if (
          !String(error.stderr).includes("does not exist") &&
          !String(error.stderr).includes("exists on disk, but not in")
        )
          throw error;
        snapshots[index] = [];
      }
    }
    const diff = git(
      "diff",
      "--unified=0",
      "--no-ext-diff",
      `${change.commit}^`,
      change.commit,
      "--",
      file,
    );
    let unknown = false;
    const destinations = new Set();
    for (const hunk of diff.matchAll(/^@@ -(\d+)(?:,(\d+))? \+(\d+)(?:,(\d+))? @@/gm)) {
      for (const index of [0, 1]) {
        const first = Number(hunk[index === 0 ? 1 : 3]);
        const count = Number(hunk[index === 0 ? 2 : 4] ?? 1);
        for (let line = first; line < first + count; line++) {
          let held = snapshots[index].filter(
            (decl) => decl.start <= line && decl.end >= line && owners[file].has(decl.name),
          );
          if (file === "src/cli.rs" && held.length)
            held = [held.sort((a, b) => a.end - a.start - (b.end - b.start))[0]];
          if (!held.length) unknown = true;
          for (const decl of held) {
            const destination = owners[file].get(decl.name);
            for (const owner of destination instanceof Set ? destination : [destination])
              destinations.add(owner);
          }
        }
      }
    }
    if (unknown) {
      files.push(file);
      if (file.startsWith("tests/e2e/"))
        for (const value of owners[file].values())
          for (const owner of value instanceof Set ? value : [value]) destinations.add(owner);
      // A changed import, impl or unclassified helper may affect every extracted facet from this file.
      for (const [name, paths] of Object.entries(layout.historicalPaths ?? {}))
        if (paths.includes(file)) destinations.add(name);
    }
    for (const owner of destinations)
      files.push(
        owner.startsWith("suite:")
          ? `migrated-tests/${owner.slice(6)}/${file}`
          : `moved/${owner}/${file}`,
      );
    projected.push({ file, destinations: [...destinations].sort(), retainedWholeFile: unknown });
  }
  return { files: [...new Set(files)], projected };
}
