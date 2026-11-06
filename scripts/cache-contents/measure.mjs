#!/usr/bin/env node
// Runner half of the cache-contents spike (spike-onepipeline-contents, a throwaway):
// says what a restored Rust cache family holds, and what each warm compile line
// costs. Run by .github/workflows/cache-contents.yml; every line it prints for the
// report starts `cc|`, which is what scripts/cache-contents/run.sh keeps.
//
//   measure.mjs inventory --family F   bytes per directory, apparent and compressed
//   measure.mjs lines     --family F   each warm compile line: time, units, who wants what
//   measure.mjs price     --family F   each compile line from cold
//
// Compressed bytes are this tree's own tar and zstd, the commands actions/cache
// runs, so a group's figure is comparable with the family's listing size. Groups
// compress apart from each other, so they sum a little above the whole.
import { spawnSync } from "node:child_process";
import { existsSync, lstatSync, mkdtempSync, readdirSync, rmSync, writeFileSync } from "node:fs";
import { homedir, tmpdir } from "node:os";
import { extname, join, relative, sep } from "node:path";

const [verb, ...rest] = process.argv.slice(2);
const family = rest[rest.indexOf("--family") + 1];
const FAMILIES = ["test-linux", "test-windows", "test-macos", "ci-capture"];
if (!["inventory", "lines", "price"].includes(verb) || !FAMILIES.includes(family)) {
  console.error(`cache-contents: usage: measure.mjs inventory|lines|price --family ${FAMILIES.join("|")}`);
  console.error("ACTION: name a verb and one of those families");
  process.exit(2);
}

const CARGO_HOME = process.env.CARGO_HOME || join(homedir(), ".cargo");
const TARGET = join(process.cwd(), "target");
const scratch = mkdtempSync(join(tmpdir(), "cache-contents-"));
const out = (...cells) => console.log(["cc", ...cells].join("|"));
const posix = (p) => p.split(sep).join("/");
const sh = (script, env = {}) =>
  spawnSync("bash", ["-c", script], { env: { ...process.env, ...env }, encoding: "utf8", maxBuffer: 2 ** 31 });

// Every file under a root, as { root, path (relative, posix), size }.
function walk(root, dir = root, files = []) {
  if (!existsSync(dir)) return files;
  for (const entry of readdirSync(dir, { withFileTypes: true })) {
    const full = join(dir, entry.name);
    if (entry.isDirectory()) walk(root, full, files);
    else files.push({ root, path: posix(relative(root, full)), size: lstatSync(full).size });
  }
  return files;
}

// What actions/cache's tar and zstd make of these files. The driver's listing
// comparison is what says the method matches.
function compressed(files) {
  let total = 0;
  for (const root of new Set(files.map((f) => f.root))) {
    const list = join(scratch, "files.txt");
    writeFileSync(list, files.filter((f) => f.root === root).map((f) => f.path).join("\n") + "\n");
    const zstd = process.platform === "win32" ? "zstd -T0 --long=30" : "zstd -T0";
    const local = process.platform === "win32" ? "--force-local" : "";
    const run = sh(`set -o pipefail; tar --posix ${local} -cf - -C "${posix(root)}" --files-from "${posix(list)}" | ${zstd} -c | wc -c`);
    const bytes = Number(run.stdout.trim());
    if (run.status !== 0 || !Number.isFinite(bytes)) {
      console.error(`cache-contents: tar or zstd failed over ${root}: ${run.stderr}`);
      process.exit(1);
    }
    total += bytes;
  }
  return total;
}

const sum = (files) => files.reduce((total, f) => total + f.size, 0);
const row = (name, files) => out("group", name, files.length, sum(files), compressed(files));

// The directory a file is counted under: a profile's deps, build and .fingerprint
// apart, under the nearest nested target directory, and the first level elsewhere.
function groupOf(file) {
  const parts = file.path.split("/");
  const profile = parts.findIndex((p) => ["deps", "build", ".fingerprint", "incremental", "examples"].includes(p));
  if (profile > 0) return parts.slice(0, profile + 1).join("/");
  return parts.slice(0, Math.min(parts.length - 1, 2)).join("/") || parts[0];
}

function restoredFiles() {
  const target = family === "ci-capture" ? join(TARGET, "ci-capture-target") : TARGET;
  const files = walk(target);
  const cargo = family === "ci-capture" ? [] : ["bin", ".crates.toml", ".crates2.json", "registry", "git"].flatMap((name) => {
    const full = join(CARGO_HOME, name);
    if (!existsSync(full)) return [];
    return lstatSync(full).isDirectory() ? walk(CARGO_HOME, full) : [{ root: CARGO_HOME, path: name, size: lstatSync(full).size }];
  });
  return { target: files, cargo };
}

function inventory() {
  const { target, cargo } = restoredFiles();
  const whole = [...target, ...cargo];
  row("WHOLE", whole);
  const groups = new Map();
  for (const f of whole) {
    const name = (f.root === CARGO_HOME ? "~/.cargo/" : "target/") + groupOf(f);
    groups.set(name, [...(groups.get(name) ?? []), f]);
  }
  for (const [name, files] of [...groups].sort((a, b) => sum(b[1]) - sum(a[1]))) {
    if (sum(files) > 1e6) row(name, files);
  }
  // Which kinds of file, and which crates, take a profile's deps.
  const kinds = new Map();
  const crates = new Map();
  for (const f of target.filter((f) => f.path.includes("/deps/"))) {
    const base = f.path.split("/").pop();
    const kind = extname(base) || "(none)";
    kinds.set(kind, (kinds.get(kind) ?? 0) + f.size);
    const crate = base.replace(/^lib/, "").replace(/-[0-9a-f]{16}.*$/, "").replace(/\..*$/, "");
    const entry = crates.get(crate) ?? { bytes: 0, units: new Set() };
    entry.bytes += f.size;
    entry.units.add(`${f.path.split("/deps/")[0]}/${base.split(".")[0]}`);
    crates.set(crate, entry);
  }
  for (const [kind, bytes] of [...kinds].sort((a, b) => b[1] - a[1]).slice(0, 12)) out("kind", kind, bytes);
  for (const [crate, e] of [...crates].sort((a, b) => b[1].bytes - a[1].bytes).slice(0, 40)) {
    out("crate", crate, e.units.size, e.bytes);
  }
}

// The warm script's compile lines for this family, each as the recipe it runs and
// the same compile asked for its artifact list, which a fresh unit also answers.
const ARCHIVE = `${posix(TARGET)}/cache-contents-archive.tar.zst`;
const DOUBLES = (prefix, dir) =>
  ["--package onepipeline-testfakes", "--package oneagentgraph --bin oneagentgraph --locked", "--package oneharness --bin oneharness --locked", "--package onevcs --bin onevcs --locked"]
    .map((selection) => `${prefix} cargo build --offline ${selection} --target-dir "${dir}" --message-format=json`)
    .join("; ");
const LLVM = `eval "$(RUSTFLAGS='-D warnings' cargo llvm-cov show-env --export-prefix 2>/dev/null)"`;
const COV_DIR = `${posix(TARGET)}/llvm-cov-target`;
const COMMON = {
  clippy: { recipe: "just _crate-lint", units: "cargo clippy --all-targets --locked --message-format=json -- -D warnings", cold: ["debug"] },
  release: { recipe: "just _stop-verdict-build", units: `RUSTFLAGS="-D warnings" cargo build --release --locked --bin onepipeline --message-format=json`, cold: ["release"] },
  "doubles-plain": { recipe: "just _e2e-doubles-build plain", units: DOUBLES("unset RUSTFLAGS;", posix(TARGET)), cold: ["debug"] },
};
const LINES = {
  linux: {
    "instrumented-tests": { recipe: `ONEPIPELINE_TEST_ARCHIVE="${ARCHIVE}" just _crate-test-archive`, units: `${LLVM}; cargo test --no-run --locked --target-dir "${COV_DIR}" --message-format=json`, cold: ["llvm-cov-target"] },
    "doubles-instrumented": { recipe: "just _e2e-doubles-build instrumented", units: `${LLVM}; ${DOUBLES("", COV_DIR)}`, cold: ["llvm-cov-target"] },
    "debug-build": { recipe: "just _crate-build", units: `RUSTFLAGS="-D warnings" cargo build --locked --all-targets --message-format=json`, cold: ["debug"] },
    release: COMMON.release,
    doc: { recipe: "just _crate-doc", units: `RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --locked --message-format=json`, cold: ["debug", "doc"] },
    clippy: COMMON.clippy,
    "doubles-plain": COMMON["doubles-plain"],
    "doubles-plain-deny": { recipe: "just _e2e-doubles-build plain-deny-warnings", units: DOUBLES(`export RUSTFLAGS="-D warnings";`, posix(TARGET)), cold: ["debug"] },
  },
  cross: {
    "plain-tests": { recipe: `ONEPIPELINE_TEST_ARCHIVE="${ARCHIVE}" just _crate-test-quick-archive`, units: "cargo test --no-run --locked --message-format=json", cold: ["debug"] },
    clippy: COMMON.clippy,
    release: COMMON.release,
    "doubles-plain": COMMON["doubles-plain"],
  },
};
const linesFor = () => LINES[family === "test-linux" ? "linux" : "cross"];

function timed(script) {
  const started = Date.now();
  const run = sh(script);
  return { ...run, seconds: Math.round((Date.now() - started) / 100) / 10 };
}

// The unit a build file belongs to, the same for a path a cargo message names and
// a path found on disk: `<profile>/deps/<stem>`, or `<profile>/build/<dir>`. A
// .fingerprint's name does not carry the artifact's hash, so it is `(not a unit)`.
function unitKey(path) {
  const p = posix(path);
  const deps = p.match(/^(.*\/deps\/)(?:lib)?([^/.]+)[^/]*$/);
  if (deps) return `${deps[1]}${deps[2]}`;
  const build = p.match(/^(.*\/build\/[^/]+)(\/.*)?$/);
  if (build) return build[1];
  return null;
}

// Each line's artifact list, with how many units it did not find fresh.
function askUnits(script) {
  const run = sh(script);
  if (run.status !== 0) {
    console.error(run.stderr.slice(-4000));
    return { keys: new Set(), stale: ["(the compile failed)"], failed: true };
  }
  const keys = new Set();
  const stale = [];
  for (const line of run.stdout.split("\n")) {
    if (!line.startsWith("{")) continue;
    let message;
    try {
      message = JSON.parse(line);
    } catch {
      continue;
    }
    if (message.reason === "compiler-artifact") {
      for (const name of message.filenames ?? []) {
        const key = unitKey(relativeToTarget(name));
        if (key) keys.add(key);
      }
      if (!message.fresh) stale.push(message.target?.name ?? message.package_id);
    }
    if (message.reason === "build-script-executed" && message.out_dir) {
      const key = unitKey(relativeToTarget(message.out_dir));
      if (key) keys.add(key);
    }
  }
  return { keys, stale, failed: false };
}
const relativeToTarget = (file) => posix(relative(TARGET, file));

// ci-capture holds one build under one set of flags, so the repository's pruner
// answers it where it cannot answer test-linux: newest build of each unit, kept.
function capture() {
  const dir = join(TARGET, "ci-capture-target");
  const before = walk(dir);
  out("candidate", "family as restored", before.length, sum(before), compressed(before));
  const dry = sh("cargo-prune-superseded --keep-within 1h --dry-run --json target/ci-capture-target");
  out("prune-dry-run", dry.status, dry.stdout.trim().slice(0, 2000));
  const apply = sh("cargo-prune-superseded --keep-within 1h --json target/ci-capture-target");
  out("prune-applied", apply.status, apply.stdout.trim().slice(0, 2000));
  const after = walk(dir);
  out("candidate", "newest build of each unit", after.length, sum(after), compressed(after));
  const run = timed("SHOTS_OUT=shots/current/x86_64 bash screenshots/ci-capture.sh 2>&1");
  const compiling = (run.stdout.match(/^\s*Compiling /gm) ?? []).length;
  out("capture-after-prune", run.status, run.seconds, compiling, run.stdout.split("\n").filter((l) => /^\s*Compiling /.test(l)).map((l) => l.trim().split(" ")[1]).join(" ").slice(0, 600));
  if (run.status !== 0) console.error(run.stdout.slice(-4000));
}

function lines() {
  if (family === "ci-capture") return capture();
  // The tree as it was restored: what is attributed, whatever the recipes then build.
  const restored = walk(TARGET);
  const prune = sh("cargo-prune-superseded --keep-within 1h --dry-run --json target");
  out("prune-dry-run", prune.status, prune.stdout.trim().slice(0, 2000));
  const wanted = new Map();
  for (const [name, line] of Object.entries(linesFor())) {
    const run = timed(line.recipe);
    if (run.status !== 0) console.error(run.stderr.slice(-4000));
    const units = askUnits(line.units);
    out("line", name, run.status, run.seconds, units.keys.size, units.stale.length, units.stale.slice(0, 15).join(" "));
    wanted.set(name, units.keys);
  }
  rmSync(ARCHIVE, { force: true });

  const owners = new Map();
  for (const [name, keys] of wanted) for (const key of keys) owners.set(key, [...(owners.get(key) ?? []), name]);
  const buckets = new Map();
  for (const f of restored) {
    const key = unitKey(f.path);
    const owner = key === null ? "(not a unit)" : (owners.get(key) ?? ["(no line wants it)"]).join("+");
    buckets.set(owner, [...(buckets.get(owner) ?? []), f]);
  }
  for (const [owner, files] of [...buckets].sort((a, b) => sum(b[1]) - sum(a[1]))) {
    if (sum(files) > 1e6) out("owner", owner, files.length, sum(files), compressed(files));
  }

  const unwanted = buckets.get("(no line wants it)") ?? [];
  const byTree = new Map();
  for (const f of unwanted) {
    const tree = f.path.split("/").slice(0, f.path.startsWith("llvm-cov-target") ? 2 : 1).join("/");
    byTree.set(tree, [...(byTree.get(tree) ?? []), f]);
  }
  for (const [tree, files] of byTree) out("unwanted-in", tree, files.length, sum(files), compressed(files));

  // The family as the warm job would save it with each candidate applied: the
  // restored files alone, so what the recipes above built counts for nothing.
  const cargo = restoredFiles().cargo;
  const gone = new Set(unwanted);
  const capture = (f) => f.path.startsWith("ci-capture-target/");
  const tools = (f) => f.path.startsWith("tools/");
  const candidates = {
    "family as restored": () => true,
    "without what no line wants": (f) => !gone.has(f),
    "without ci-capture-target": (f) => !capture(f),
    "without target/tools": (f) => !tools(f),
    "without all three": (f) => !gone.has(f) && !capture(f) && !tools(f),
  };
  for (const [name, keep] of Object.entries(candidates)) {
    const kept = [...restored.filter(keep), ...cargo];
    out("candidate", name, kept.length, sum(kept), compressed(kept));
  }

  // Take away what no line named and ask each line again: a unit now not fresh is
  // a compile a pull request would pay for. A .fingerprint stays, being `(not a unit)`.
  for (const f of unwanted) rmSync(join(TARGET, f.path), { force: true });
  for (const [name, line] of Object.entries(linesFor())) {
    const units = askUnits(line.units);
    out("after-removal", name, units.failed ? "failed" : "ok", units.stale.length, units.stale.slice(0, 25).join(" "));
  }
}

// Each compile line from cold: only the directories it builds into are removed,
// and removed again before the next, so one line's figure owes nothing to another.
function price() {
  if (family === "ci-capture") {
    const capture = "SHOTS_OUT=shots/current/x86_64 bash screenshots/ci-capture.sh";
    const restored = timed(capture);
    out("price", "capture from the restored ci-capture family", restored.status, restored.seconds);
    rmSync(join(TARGET, "ci-capture-target"), { recursive: true, force: true });
    const cold = timed(capture);
    out("price", "capture from cold", cold.status, cold.seconds);
    return;
  }
  if (family === "test-linux") {
    const capture = timed("SHOTS_OUT=shots/current/x86_64 bash screenshots/ci-capture.sh");
    out("price", "capture from test-linux's own ci-capture-target, no ci-capture family", capture.status, capture.seconds);
    if (capture.status !== 0) console.error(capture.stderr.slice(-4000));
    rmSync(join(TARGET, "ci-capture-target"), { recursive: true, force: true });
  }
  const incremental = new Map();
  for (const [name, line] of Object.entries(linesFor())) {
    const run = timed(line.recipe);
    incremental.set(name, run.seconds);
    out("price", `${name}: from the restored family`, run.status, run.seconds);
  }
  for (const [name, line] of Object.entries(linesFor())) {
    for (const dir of line.cold) rmSync(join(TARGET, dir), { recursive: true, force: true });
    const run = timed(line.recipe);
    if (run.status !== 0) console.error(run.stderr.slice(-4000));
    out("price", `${name}: from cold (${line.cold.join(", ")} removed)`, run.status, run.seconds);
  }
  rmSync(ARCHIVE, { force: true });
  rmSync(join(CARGO_HOME, "registry"), { recursive: true, force: true });
  const fetch = timed("cargo fetch --locked");
  out("price", "~/.cargo/registry from cold: cargo fetch", fetch.status, fetch.seconds);
}

({ inventory, lines, price })[verb]();
rmSync(scratch, { recursive: true, force: true });
