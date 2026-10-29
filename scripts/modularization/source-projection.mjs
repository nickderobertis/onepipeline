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

const ownerSet = (value) => new Set(value instanceof Set ? value : [value]);

/**
 * A driver line outside every owned declaration resolves to the owners of what it actually belongs to, in that
 * snapshot: an unowned private helper to the owners of the declarations calling it (a public one may have callers
 * elsewhere, so it stays unresolved); a unit test to the owners of the production items it references; a method or nested item to its
 * enclosing item; an `impl` block to its type; a comment or attribute to the item it precedes; a `use` statement
 * to the declarations naming what it imports. A blank line changes nothing. Anything else returns null, which
 * keeps the whole-file input.
 */
export function driverLineOwners(text, decls, fileOwners) {
  const code = codeOnly(text);
  const lines = text.split("\n");
  const codeLines = code.split("\n");
  const lineOf = (offset) => code.slice(0, offset).split("\n").length;
  const ranges = decls.map((decl) => ({
    ...decl,
    code: decl.code ?? codeLines.slice(decl.start - 1, decl.end).join("\n"),
  }));
  for (const match of code.matchAll(
    /^[ \t]*impl\b(?:<[^>{]*>)?\s+(?:[\w:<>, ]+\s+for\s+)?(\w+)[^{;]*\{/gm,
  )) {
    let end = match.index + match[0].length;
    for (let depth = 1; end < code.length && depth; end++)
      depth += code[end] === "{" ? 1 : code[end] === "}" ? -1 : 0;
    ranges.push({
      name: `impl:${match[1]}`,
      type: match[1],
      start: lineOf(match.index),
      end: lineOf(end),
      code: code.slice(match.index, end),
    });
  }
  for (const match of code.matchAll(/^[ \t]*#\[cfg\(test\)\]\s*mod\s+(\w+)\s*\{/gm)) {
    let end = match.index + match[0].length;
    for (let depth = 1; end < code.length && depth; end++)
      depth += code[end] === "{" ? 1 : code[end] === "}" ? -1 : 0;
    ranges.push({
      name: `mod:${match[1]}`,
      test: true,
      start: lineOf(match.index),
      end: lineOf(end),
    });
  }
  const span = (range) => range.end - range.start;
  const container = (range) =>
    ranges
      .filter(
        (other) =>
          other !== range &&
          other.start <= range.start &&
          other.end >= range.end &&
          span(other) > span(range),
      )
      .sort((a, b) => span(a) - span(b))[0];
  const innermost = (line, except) =>
    ranges
      .filter((range) => range !== except && range.start <= line && range.end >= line)
      .sort((a, b) => span(a) - span(b))[0];
  const inTests = (range) =>
    ranges.some(
      (other) =>
        other.test && other.start <= range.start && other.end >= range.end && other !== range,
    );
  // Owners of the in-file declarations calling this one, or null while any caller is unresolved.
  const callers = (range) => {
    const users = [];
    for (const [index, line] of codeLines.entries())
      if (new RegExp(`\\b${range.name}\\b`).test(line)) {
        const user = innermost(index + 1, null);
        if (!user || user === range || (user.start >= range.start && user.end <= range.end))
          continue;
        // A unit test exercising a production helper does not own it; its production callers do.
        if ((user.test || inTests(user)) && !inTests(range)) continue;
        // A type's own impl blocks take their owners from it, not the reverse.
        if (user.type === range.name || container(user)?.type === range.name) continue;
        if (!resolved.has(user)) return null;
        users.push(resolved.get(user));
      }
    return users.length ? new Set(users.flatMap((set) => [...set])) : null;
  };
  // A unit test moves with the owners of the production items it references (the documented unit-test rule).
  const referenced = (range) => {
    const owners = new Set();
    for (const other of ranges) {
      if (other.test || other.type || inTests(other) || !/^\w+$/.test(other.name)) continue;
      if (!new RegExp(`\\b${other.name}\\b`).test(range.code)) continue;
      if (container(other) && !container(other).type) continue;
      if (!resolved.has(other)) return null;
      for (const owner of resolved.get(other)) owners.add(owner);
    }
    return owners.size ? owners : null;
  };
  const resolved = new Map();
  for (const range of ranges)
    if (fileOwners.has(range.name)) resolved.set(range, ownerSet(fileOwners.get(range.name)));
  for (let changed = true; changed; ) {
    changed = false;
    for (const range of ranges) {
      if (resolved.has(range) || range.test) continue;
      const outer = container(range);
      let owners = null;
      if (outer?.test) owners = referenced(range) ?? (range.private ? callers(range) : null);
      else if (outer) owners = resolved.get(outer) ?? null;
      else if (range.type) {
        const type = ranges.find(
          (other) => other.name === range.type && !other.type && !container(other),
        );
        owners = type ? (resolved.get(type) ?? null) : null;
      } else if (range.private) owners = callers(range);
      if (owners) {
        resolved.set(range, owners);
        changed = true;
      }
    }
  }
  const uses = [...code.matchAll(/^[ \t]*(?:pub(?:\([^)]*\))?\s+)?use\s+([^;]*);/gm)].map(
    (match) => ({
      start: lineOf(match.index),
      end: lineOf(match.index + match[0].length - 1),
      names: [...match[1].matchAll(/\b([A-Za-z_]\w*)\b(?!\s*::)/g)]
        .map((name) => name[1])
        .filter((name) => !["self", "super", "crate", "as"].includes(name)),
    }),
  );
  return (line) => {
    const range = innermost(line, null);
    if (range) return resolved.get(range) ?? null;
    const statement = uses.find((use) => use.start <= line && use.end >= line);
    if (statement) {
      const owners = new Set();
      for (const name of statement.names)
        for (const [index, codeLine] of codeLines.entries()) {
          if (index + 1 >= statement.start && index + 1 <= statement.end) continue;
          if (!new RegExp(`\\b${name}\\b`).test(codeLine)) continue;
          const user = innermost(index + 1, null);
          if (!user || !resolved.has(user)) return null;
          for (const owner of resolved.get(user)) owners.add(owner);
        }
      return owners.size ? owners : null;
    }
    const raw = lines[line - 1] ?? "";
    if (!raw.trim()) return new Set();
    if (!/^\s*(?:\/\/|#\[|\/\*|\*)/.test(raw)) return null;
    const next = ranges
      .filter((candidate) => candidate.start > line)
      .sort((a, b) => a.start - b.start)[0];
    if (!next) return null;
    for (let between = line + 1; between < next.start; between++) {
      const text = lines[between - 1] ?? "";
      if (text.trim() && !/^\s*(?:\/\/|#\[|\/\*|\*|[\w(),\s="]*\]\s*$)/.test(text)) return null;
    }
    return resolved.get(next) ?? null;
  };
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
    const texts = ["", ""];
    for (const [index, rev] of [`${change.commit}^`, change.commit].entries()) {
      try {
        const text = git("show", `${rev}:${file}`);
        texts[index] = text;
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
          if (!held.length && file === "src/driver.rs") {
            snapshots[index].resolve ??= driverLineOwners(
              texts[index],
              snapshots[index],
              owners[file],
            );
            const resolved = snapshots[index].resolve(line);
            if (resolved) {
              for (const owner of resolved) destinations.add(owner);
              continue;
            }
          }
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
