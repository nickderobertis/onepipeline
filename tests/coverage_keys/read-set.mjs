// What a coverage tier can read, worked out from the sources of what it runs, so a test can hold
// that set against what real Nx hashes for the tier.
//
// A tier's reach is: the Rust modules its test binaries compile, and every workspace path named
// in those modules, in the `src/` and `crates/` code it runs, and in any script such a name
// reaches. Paths named as a directory (`root.join("docs")`) reach everything under it. It is
// a static read of the sources: a path built only at run time from parts it cannot see is outside it.
import { execFileSync } from "node:child_process";
import { readFileSync } from "node:fs";
import { dirname, join, normalize } from "node:path";
import { fixtureEnvironment } from "../coverage_cache/support.mjs";

const TOKEN = /[A-Za-z0-9_./@-]+/g;
const MOD = /(?:#\[path\s*=\s*"([^"]+)"\]\s*)?(?:pub(?:\([^)]*\))?\s+)?mod\s+([A-Za-z0-9_]+)\s*;/g;
const INCLUDE = /include(?:_str|_bytes)?!\(\s*"([^"]+)"\s*\)/g;
const ROOT_JOIN =
  /(?:repo_root\(\)|repo_file\(|env!\("CARGO_MANIFEST_DIR"\)\)?)\s*(?:\.join\(\s*)?"([^"]+)"/g;
const SCRIPT = /\.(?:sh|mjs|py|bat)$/;

const stripComments = (text) =>
  text
    .split("\n")
    .filter((line) => !/^\s*\/\//.test(line))
    .join("\n");

export function workspace(dir) {
  const tracked = execFileSync("git", ["ls-files", "-z"], {
    cwd: dir,
    encoding: "utf8",
    maxBuffer: 64 * 1024 * 1024,
  })
    .split("\0")
    .filter(Boolean);
  const files = new Set(tracked);
  const dirs = new Set();
  for (const path of tracked)
    for (let at = path.lastIndexOf("/"); at > 0; at = path.lastIndexOf("/", at - 1))
      dirs.add(path.slice(0, at));
  const under = (directory) => tracked.filter((path) => path.startsWith(`${directory}/`));
  return { dir, tracked, files, dirs, under };
}

/** Every path a text names: a tracked file or directory under its own name. */
function namedPaths(ws, text, found) {
  // Adjacent `.join` calls name one path: `root.join("a").join("b")` is `a/b`.
  const joined = stripComments(text).replace(/"\s*\)\s*\.join\(\s*"/g, "/");
  for (const [, literal] of joined.matchAll(ROOT_JOIN)) addPath(ws, literal, found, true, true);
  for (const [, literal] of joined.matchAll(/"((?:[^"\\\n]|\\.)*)"/g)) {
    for (const token of literal.match(TOKEN) ?? [])
      addPath(ws, token, found, token.replace(/\/+$/, "").includes("/"));
  }
}

function addPath(ws, raw, found, mayBeDirectory, rootRelative = false) {
  const path = raw.replace(/^\.\//, "").replace(/\/+$/, "");
  // A bare root-level name counts only where the code roots it at the workspace.
  if (ws.files.has(path) && (rootRelative || path.includes("/"))) found.add(path);
  else if (mayBeDirectory && ws.dirs.has(path)) for (const file of ws.under(path)) found.add(file);
}

function withoutTestModules(text) {
  const start = /#\[cfg\(test\)\]/g;
  let out = "";
  let at = 0;
  for (let match = start.exec(text); match; match = start.exec(text)) {
    out += text.slice(at, match.index);
    let i = match.index + match[0].length;
    while (i < text.length && text[i] !== "{" && text[i] !== ";") i++;
    if (text[i] === "{") {
      let depth = 1;
      i++;
      while (i < text.length && depth > 0) {
        if (text[i] === "{") depth++;
        else if (text[i] === "}") depth--;
        i++;
      }
    } else i++;
    at = i;
    start.lastIndex = i;
  }
  return out + text.slice(at);
}

/** A Rust root and every file it compiles through `mod` and `include!`. */
function moduleClosure(ws, root, closure = new Set()) {
  if (closure.has(root) || !ws.files.has(root)) return closure;
  closure.add(root);
  const text = stripComments(readFileSync(join(ws.dir, root), "utf8"));
  const base = dirname(root);
  const nested =
    /\/(?:main|lib|mod)\.rs$/.test(root) || /^tests\/[^/]+\.rs$/.test(root)
      ? base
      : root.replace(/\.rs$/, "");
  for (const [, explicit, name] of text.matchAll(MOD)) {
    const candidates = explicit
      ? [normalize(join(base, explicit))]
      : [join(nested, `${name}.rs`), join(nested, name, "mod.rs")];
    for (const candidate of candidates) moduleClosure(ws, candidate, closure);
  }
  for (const [, included] of text.matchAll(INCLUDE)) {
    const file = normalize(join(base, included));
    if (ws.files.has(file)) {
      closure.add(file);
      if (file.endsWith(".rs")) moduleClosure(ws, file, closure);
    }
  }
  return closure;
}

function binaries(ws) {
  const manifest = readFileSync(join(ws.dir, "Cargo.toml"), "utf8");
  const declared = new Map(
    [...manifest.matchAll(/\[\[test\]\]\s*\nname = "([^"]+)"\s*\npath = "([^"]+)"/g)].map(
      ([, name, path]) => [name, path],
    ),
  );
  for (const path of ws.tracked) {
    const name = path.match(/^tests\/([^/]+)\.rs$/)?.[1];
    if (name && !declared.has(name)) declared.set(name, path);
  }
  return declared;
}

function tierDefinitions(ws) {
  const justfile = readFileSync(join(ws.dir, "justfile"), "utf8");
  const tiers = new Map();
  for (const [, recipe, unit, variable] of justfile.matchAll(
    /^(_[\w-]+):.*\(_tier-test "([\w-]+)" ([\w-]+)\)/gm,
  )) {
    const filter = justfile.match(new RegExp(`^${variable} := "(.*)"$`, "m"))?.[1];
    if (filter === undefined) throw new Error(`the justfile names no filter ${variable}`);
    tiers.set(unit, { recipe, variable, filter });
  }
  return tiers;
}

/** The read set of every coverage tier in `dir`: unit name -> { path: why }. */
export function readSets(dir, { rustFiles = null } = {}) {
  const ws = workspace(dir);
  const allBinaries = binaries(ws);
  const sourceFiles =
    rustFiles ?? ws.tracked.filter((path) => /^(?:src|crates)\/.*\.rs$/.test(path));
  const sets = new Map();
  for (const [unit, { filter }] of tierDefinitions(ws)) {
    const positive = [...filter.matchAll(/(?<!not )binary\((\w+)\)/g)].map(([, name]) => name);
    const excluded = [...filter.matchAll(/not binary\((\w+)\)/g)].map(([, name]) => name);
    const roots = positive.length
      ? positive
      : [...allBinaries.keys()].filter((name) => !excluded.includes(name));
    const found = new Map();
    const note = (path, why) => {
      if (!found.has(path)) found.set(path, why);
    };
    const queue = [];
    const scan = (path, why) => {
      if (!ws.files.has(path)) return;
      const text = readFileSync(join(ws.dir, path), "utf8");
      const names = new Set();
      namedPaths(ws, text, names);
      for (const name of names) {
        if (!found.has(name) && SCRIPT.test(name)) queue.push(name);
        note(name, `${path} names it${why ? ` (${why})` : ""}`);
      }
    };
    for (const name of roots) {
      const root = allBinaries.get(name);
      if (!root) continue;
      for (const file of moduleClosure(ws, root)) {
        note(file, `compiled by test binary ${name}`);
        if (file.endsWith(".rs")) scan(file);
      }
    }
    for (const file of sourceFiles) note(file, "compiled into every binary");
    // Every tier links `src/` and `crates/`, and those compile the same files for all; the
    // literal paths in them run only under the crate's own unit tests, in the rest tier.
    if (!positive.length) for (const file of sourceFiles) scan(file, "unit test in the rest tier");
    for (const file of sourceFiles) {
      // `#[cfg(test)]` modules compile only into the rest tier's unit-test binary.
      const text = positive.length
        ? withoutTestModules(readFileSync(join(ws.dir, file), "utf8"))
        : readFileSync(join(ws.dir, file), "utf8");
      for (const [, included] of text.matchAll(INCLUDE)) {
        const target = normalize(join(dirname(file), included));
        if (ws.files.has(target)) note(target, `include! in ${file}`);
      }
    }
    while (queue.length) {
      const script = queue.pop();
      scan(script, "a script a journey runs");
    }
    sets.set(unit, found);
  }
  return sets;
}

const HASHES = new URL("../../.github/ci-shape/support/nx-hashes.mjs", import.meta.url).pathname;

/** Every read Nx does not hash for a tier: [{ tier, path, why }], `exceptions` left out. */
export function keyGaps(dir, exceptions = {}) {
  const sets = readSets(dir);
  const taskOf = (unit) => (unit === "onepipeline-rest" ? "onepipeline:test-rest" : `${unit}:test`);
  const out = join(dir, ".nx-coverage-hashes.json");
  execFileSync(
    process.execPath,
    [HASHES, "capture", dir, ...[...sets.keys()].map(taskOf), "--out", out],
    {
      cwd: dir,
      env: fixtureEnvironment({ NX_DAEMON: "false" }),
      stdio: ["ignore", "ignore", "pipe"],
      maxBuffer: 256 * 1024 * 1024,
    },
  );
  const { tasks } = JSON.parse(readFileSync(out, "utf8"));
  const gaps = [];
  for (const [unit, reads] of sets) {
    const task = tasks[taskOf(unit)];
    const hashed = new Set([...task.sourceFiles, ...task.runtimeFiles]);
    for (const [path, why] of reads) {
      if (!hashed.has(path) && !exceptions[unit]?.[path]) gaps.push({ tier: unit, path, why });
    }
  }
  return { gaps, sets, tasks, taskOf };
}
