#!/usr/bin/env node
// Re-homes the start, reply, adopt and cross-command e2e tests onto the engine part each asserts.
// The engine part a test asserts is the crate whose production code names the string literals its
// assertions compare against: the literals inside its `assert!`, `assert_eq!` and `assert_ne!`
// invocations and in the `let` bindings those invocations name. A crate's production code is its
// files and the items it takes from other files, less comments, `#[cfg(test)]` modules and the items
// other crates take from its files. A literal counts only where at most DISCRIMINATING crates name
// it, so a common key ("status") is no evidence. The test moves to the engine crate with the smallest
// closure that contains every crate it asserts, as a crate-level test over a recorded fixture of the
// run its original journey drove. A test stays a full journey through the real binary when it
// asserts live ordering, a command's own output, or no engine literal at all, or when no single
// engine crate's closure contains everything it asserts. As in the test split, an engine crate's
// tests holding more than THRESHOLD of a full Linux run's estimated seconds run as one suite per
// source module.
import { readFileSync, writeFileSync } from "node:fs";
import { pathToFileURL } from "node:url";
import { codeOnly } from "./graph.mjs";
import { functions } from "./rust-syntax.mjs";
import { declarations } from "./source-projection.mjs";
import { targetGraph } from "./target-graph.mjs";
import { THRESHOLD, duplicateKey, linuxSeconds, targetCatalog } from "./test-split.mjs";
import { UsageError, exitStatus } from "./cli.mjs";

export const ENGINE_PATH = "scripts/modularization/engine-tests.json";
export const PARENTS = ["cmd-start", "cross-command", "cmd-reply", "cmd-adopt"];
export const DISCRIMINATING = 2;
export const KEEP = {
  live: "asserts live ordering, which no recorded fixture can replace",
  "command output": "asserts a literal only a command or the dispatcher names",
  "no engine literal": "its assertions compare against no literal an engine crate alone names",
  "no covering crate": "no single engine crate's closure contains every crate it asserts",
};

const read = (path) => readFileSync(path, "utf8");
/** The string literals of Rust source, outside comments, with their offsets. */
export function stringLiterals(text) {
  const found = [];
  let at = 0;
  while (at < text.length) {
    if (text.startsWith("//", at)) {
      const end = text.indexOf("\n", at);
      at = end < 0 ? text.length : end;
    } else if (text.startsWith("/*", at)) {
      let depth = 1;
      at += 2;
      while (at < text.length && depth) {
        if (text.startsWith("/*", at)) {
          depth++;
          at += 2;
        } else if (text.startsWith("*/", at)) {
          depth--;
          at += 2;
        } else at++;
      }
    } else {
      const raw =
        (at === 0 || !/[a-zA-Z0-9_]/.test(text[at - 1])) &&
        /^(?:b)?r(#{0,255})"/.exec(text.slice(at));
      if (raw) {
        const close = `"${raw[1]}`;
        const end = text.indexOf(close, at + raw[0].length);
        const stop = end < 0 ? text.length : end;
        found.push({ start: at, value: text.slice(at + raw[0].length, stop) });
        at = end < 0 ? text.length : end + close.length;
      } else if (text[at] === '"') {
        let end = at + 1;
        while (end < text.length && text[end] !== '"') end += text[end] === "\\" ? 2 : 1;
        found.push({ start: at, value: text.slice(at + 1, end) });
        at = end + 1;
      } else if (text[at] === "'") {
        const char = /^'(?:\\(?:u\{[0-9a-fA-F_]+\}|x[0-9a-fA-F]{2}|.)|[^'\\\n])'/u.exec(
          text.slice(at),
        );
        at += char ? char[0].length : 1;
      } else at++;
    }
  }
  return found;
}
const literals = (text) =>
  stringLiterals(text)
    .map((literal) => literal.value)
    .filter((literal) => literal.length >= 4 && !/[{}]/.test(literal));
const isEngine = (crate) =>
  !crate.startsWith("cmd-") && !["cli-dispatcher", "onepipeline"].includes(crate);

/** The target layout the replay uses: the reviewed module graph with the named moves applied. */
export const targetLayout = () =>
  targetGraph(
    JSON.parse(read("scripts/modularization/layout.json")),
    JSON.parse(read("scripts/modularization/target-moves.json")),
  );

/** The `#[cfg(test)]` modules of Rust source, as offset ranges. */
function testModules(text) {
  const code = codeOnly(text);
  const ranges = [];
  for (const found of code.matchAll(
    /#\[cfg\(test\)\]\s*(?:pub(?:\([^)]*\))?\s+)?mod\s+\w+\s*\{/g,
  )) {
    let end = found.index + found[0].length;
    for (let depth = 1; end < code.length && depth; end++) {
      if (code[end] === "{") depth++;
      if (code[end] === "}") depth--;
    }
    ranges.push([found.index, end]);
  }
  return ranges;
}

/**
 * Every literal the layout's crates name in their production code, with the crates naming it: each
 * crate's files and the items it takes from other files, less comments, `#[cfg(test)]` modules and
 * the items other crates take from its files.
 */
export function literalOwners(layout) {
  const takenFrom = new Map();
  for (const sources of Object.values(layout.symbolSources ?? {}))
    for (const { file, symbols } of sources) {
      if (!takenFrom.has(file)) takenFrom.set(file, new Set());
      for (const symbol of symbols) takenFrom.get(file).add(symbol);
    }
  const owned = (text, dropped) => {
    const ranges = [
      ...testModules(text),
      ...declarations(text)
        .filter((decl) => dropped.has(decl.name))
        .map((decl) => [decl.offset, decl.offset + decl.raw.length]),
    ];
    return stringLiterals(text)
      .filter(({ start }) => !ranges.some(([from, to]) => start >= from && start < to))
      .map(({ value }) => value)
      .filter((literal) => literal.length >= 4 && !/[{}]/.test(literal));
  };
  const owners = new Map();
  for (const [name, crate] of Object.entries(layout.crates)) {
    const found = crate.files.flatMap((file) =>
      owned(read(file), takenFrom.get(file) ?? new Set()),
    );
    for (const { file, symbols } of layout.symbolSources?.[name] ?? [])
      for (const decl of declarations(read(file)))
        if (symbols.includes(decl.name)) found.push(...owned(decl.raw, new Set()));
    for (const literal of new Set(found)) {
      if (!owners.has(literal)) owners.set(literal, new Set());
      owners.get(literal).add(name);
    }
  }
  return owners;
}

/** The text a test's assertions compare against: its assertion macros and the bindings they name. */
export function assertionText(body) {
  const asserts = [...body.matchAll(/\bassert(?:_eq|_ne)?!\s*\(/g)].map((match) => {
    let depth = 1;
    let at = match.index + match[0].length;
    for (; at < body.length && depth; at++) {
      if (body[at] === "(") depth++;
      if (body[at] === ")") depth--;
    }
    return body.slice(match.index, at);
  });
  const named = new Set(asserts.join("\n").match(/\b[a-z_][a-z0-9_]*\b/g) ?? []);
  const bound = [...body.matchAll(/\blet\s+(?:mut\s+)?([a-z_][a-z0-9_]*)\b[^=;]*=([^;]*);/g)]
    .filter((match) => named.has(match[1]))
    .map((match) => match[2]);
  return [...asserts, ...bound].join("\n");
}

/** Where each test of PARENTS goes, by the rule at the head of this file. */
export function deriveEngineSplit(catalog, layout) {
  const owners = literalOwners(layout);
  const closures = new Map();
  const closure = (crate) => {
    if (!closures.has(crate))
      closures.set(
        crate,
        new Set([crate, ...layout.crates[crate].deps.flatMap((dep) => [...closure(dep)])]),
      );
    return closures.get(crate);
  };
  const engines = Object.keys(layout.crates).filter(isEngine).sort();
  const bodies = new Map();
  const body = (original) => {
    const [module, name] = original.split("::");
    if (!bodies.has(module))
      bodies.set(
        module,
        new Map(functions(read(`tests/e2e/${module}.rs`)).map((fn) => [fn.name, fn.body])),
      );
    const found = bodies.get(module).get(name);
    if (found === undefined) throw new Error(`no test body for ${original}`);
    return found;
  };
  const moved = {};
  const kept = {};
  for (const [id, test] of Object.entries(catalog.tests)) {
    if (!PARENTS.includes(test.suite)) continue;
    const evidence = {};
    for (const literal of literals(assertionText(body(test.original)))) {
      const crates = owners.get(literal);
      if (crates && crates.size <= DISCRIMINATING) evidence[literal] = [...crates].sort();
    }
    const asserted = new Set(Object.values(evidence).flat());
    const keep = (reason) => {
      kept[id] = { parent: test.suite, reason, evidence };
    };
    if (catalog.dispositions[test.original].reason.startsWith("live-")) keep("live");
    else if (!asserted.size) keep("no engine literal");
    else if ([...asserted].some((crate) => !isEngine(crate))) keep("command output");
    else {
      const covering = engines.filter((crate) =>
        [...asserted].every((part) => closure(crate).has(part)),
      );
      if (!covering.length) keep("no covering crate");
      else {
        const crate = covering.reduce((best, crate) =>
          closure(crate).size < closure(best).size ? crate : best,
        );
        const [module, name] = id.split("::");
        moved[id] = {
          parent: test.suite,
          crate,
          suite: `engine--${crate}`,
          crateTest: `${crate}/tests/${module}.rs::${name}`,
          fixture: `tests/recorded/engine/${crate}/${module}/${name}/`,
          evidence,
        };
      }
    }
  }
  const seconds = linuxSeconds(catalog);
  const total = Object.values(seconds).reduce((sum, value) => sum + value, 0);
  const byCrate = {};
  for (const [id, entry] of Object.entries(moved)) {
    byCrate[entry.crate] ??= [];
    byCrate[entry.crate].push(id);
  }
  for (const [crate, ids] of Object.entries(byCrate))
    if (ids.reduce((sum, id) => sum + seconds[id], 0) / total > THRESHOLD)
      for (const id of ids) moved[id].suite = `engine--${crate}--${id.split("::")[0]}`;
  const sortedKeys = (object) =>
    Object.fromEntries(
      Object.keys(object)
        .sort()
        .map((key) => [key, object[key]]),
    );
  return {
    schema: 1,
    parents: PARENTS,
    discriminating: DISCRIMINATING,
    moved: sortedKeys(moved),
    kept: sortedKeys(kept),
  };
}

/**
 * The engine crate and suite each moved test runs in. Refuses a file that drops or repeats a test of PARENTS,
 * names a test the catalog lacks or under the wrong parent, moves a live-ordering test, or names a
 * crate the layout does not hold or that is not an engine crate.
 */
export function checkEngineSplit(text, catalog, layout) {
  const split = JSON.parse(text);
  const repeated = duplicateKey(text);
  if (
    !split ||
    typeof split !== "object" ||
    Array.isArray(split) ||
    Object.keys(split).some(
      (key) => !["schema", "parents", "discriminating", "moved", "kept"].includes(key),
    ) ||
    split.schema !== 1 ||
    JSON.stringify(split.parents) !== JSON.stringify(PARENTS) ||
    split.discriminating !== DISCRIMINATING ||
    [split.moved, split.kept].some(
      (entries) => !entries || typeof entries !== "object" || Array.isArray(entries),
    )
  )
    throw new Error(`invalid engine split ${ENGINE_PATH}`);
  if (repeated !== undefined) throw new Error(`engine split names ${repeated} twice`);
  const engineOf = new Map();
  for (const [section, entries] of [
    ["moved", split.moved],
    ["kept", split.kept],
  ])
    for (const [id, entry] of Object.entries(entries)) {
      const test = catalog.tests[id];
      if (!Object.hasOwn(catalog.tests, id))
        throw new Error(`engine split names unknown test ${id}`);
      if (!entry || typeof entry !== "object" || entry.parent !== test.suite)
        throw new Error(`engine split files ${id} under ${entry?.parent}, not ${test.suite}`);
      if (section === "kept") {
        if (!Object.hasOwn(KEEP, entry.reason))
          throw new Error(`engine split keeps ${id} for no known reason`);
        if (Object.hasOwn(split.moved, id)) throw new Error(`test ${id} is both moved and kept`);
        continue;
      }
      if (catalog.dispositions[test.original].reason.startsWith("live-"))
        throw new Error(`engine split moves live-ordering test ${id}`);
      if (
        typeof entry.crate !== "string" ||
        !Object.hasOwn(layout.crates, entry.crate) ||
        !isEngine(entry.crate)
      )
        throw new Error(`engine split moves ${id} to ${entry.crate}, which is no engine crate`);
      const [, crate, module] =
        /^engine--([a-z][a-z0-9-]*?)(?:--([a-z][a-z0-9_]*))?$/.exec(entry.suite ?? "") ?? [];
      if (crate !== entry.crate || (module !== undefined && module !== id.split("::")[0]))
        throw new Error(`engine split runs ${id} in ${entry.suite}, not its crate's suite`);
      engineOf.set(id, { crate: entry.crate, suite: entry.suite });
    }
  for (const [id, test] of Object.entries(catalog.tests))
    if (
      PARENTS.includes(test.suite) &&
      !Object.hasOwn(split.moved, id) &&
      !Object.hasOwn(split.kept, id)
    )
      throw new Error(`test ${id} of ${test.suite} is neither moved nor kept`);
  return engineOf;
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  try {
    const mode = process.argv[2];
    if (process.argv.length !== 3 || !["--write", "--check"].includes(mode))
      throw new UsageError("usage: engine-tests.mjs --write | --check");
    const catalog = targetCatalog();
    const layout = targetLayout();
    const derived = `${JSON.stringify(deriveEngineSplit(catalog, layout), null, 2)}\n`;
    if (mode === "--write") writeFileSync(ENGINE_PATH, derived);
    const committed = read(ENGINE_PATH);
    const engineOf = checkEngineSplit(committed, catalog, layout);
    if (committed !== derived)
      throw new Error(`${ENGINE_PATH} differs from the split its rule derives`);
    console.log(
      `engine split: ${engineOf.size} tests move to ${new Set([...engineOf.values()].map((entry) => entry.crate)).size} engine crates in ${new Set([...engineOf.values()].map((entry) => entry.suite)).size} suites; ${Object.keys(JSON.parse(committed).kept).length} stay full journeys`,
    );
  } catch (error) {
    const next =
      error.code && /^(?:EACCES|EPERM|ENOENT|EISDIR|ENOTDIR|ENOSPC|EROFS)$/.test(error.code)
        ? "check the named path's type, existence, permissions and disk space; rerun the same engine-tests.mjs invocation"
        : "regenerate with engine-tests.mjs --write and review the split, or restore the named test to exactly one of moved or kept";
    console.error(`${error.message}\nnext: ${next}`);
    process.exitCode = exitStatus(error);
  }
}
