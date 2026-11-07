#!/usr/bin/env node
// A throwaway on a draft whose branch disables ci.yml: what each altered warm leg's save
// would archive, built cold and saving nothing. Every figure it prints starts `cc|`.
//
//   measure.mjs doubles            coverage tier's flags over the instrumented doubles
//   measure.mjs capture-archive    the capture's archive, each unit named by its own build
//   measure.mjs clean-and-archive --leg linux|cross    rust-cache's post-step clean, then archive
//   measure.mjs rerun --leg linux|cross                each warm line again: what recompiles
import { spawnSync } from "node:child_process";
import { existsSync, lstatSync, mkdtempSync, readdirSync, rmSync, writeFileSync } from "node:fs";
import { homedir, tmpdir } from "node:os";
import { basename, dirname, extname, join, relative, sep } from "node:path";

const [verb, ...rest] = process.argv.slice(2);
const leg = rest[rest.indexOf("--leg") + 1];
const CARGO_HOME = process.env.CARGO_HOME || join(homedir(), ".cargo");
const TARGET = join(process.cwd(), "target");
const scratch = mkdtempSync(join(tmpdir(), "cache-demo-"));
const out = (...cells) => console.log(["cc", ...cells].join("|"));
const posix = (p) => p.split(sep).join("/");
const sh = (script, env = {}) => spawnSync("bash", ["-c", script], { env: { ...process.env, ...env }, encoding: "utf8", maxBuffer: 2 ** 31 });
function walk(root, dir = root, files = []) {
  if (!existsSync(dir)) return files;
  for (const entry of readdirSync(dir, { withFileTypes: true })) {
    const full = join(dir, entry.name);
    if (entry.isDirectory()) walk(root, full, files);
    else files.push({ root, path: posix(relative(root, full)), size: lstatSync(full).size });
  }
  return files;
}
const sum = (files) => files.reduce((t, f) => t + f.size, 0);
// actions/cache's own tar and zstd, the method spike-onepipeline-contents held to within 5%
// of each family's listing. Roots compress apart, so they sum a little above a whole.
function compressed(files) {
  let total = 0;
  for (const root of new Set(files.map((f) => f.root))) {
    const list = join(scratch, "files.txt");
    writeFileSync(list, `${files.filter((f) => f.root === root).map((f) => f.path).join("\n")}\n`);
    const local = process.platform === "win32" ? "--force-local" : "";
    const run = sh(`set -o pipefail; tar --posix ${local} -cf - -C "${posix(root)}" --files-from "${posix(list)}" | zstd -T0 -c | wc -c`);
    const bytes = Number(run.stdout.trim());
    if (run.status !== 0 || !Number.isFinite(bytes)) { console.error(`tar or zstd failed over ${root}: ${run.stderr}`); process.exit(1); }
    total += bytes;
  }
  return total;
}
const unitKey = (path) => {
  const p = posix(path);
  const deps = p.match(/^(.*\/deps\/)(?:lib)?([^/.]+)[^/]*$/);
  if (deps) return `${deps[1]}${deps[2]}`;
  const build = p.match(/^(.*\/build\/[^/]+)(\/.*)?$/);
  return build ? build[1] : null;
};
const members = () => new Set(JSON.parse(sh("cargo metadata --no-deps --format-version 1 --locked").stdout).packages.map((p) => p.name.replaceAll("-", "_")));
// What a compile asks of cargo, as its JSON says: the artifacts, and which it did not find fresh.
function ask(script, relativeTo, from) {
  const run = sh(script);
  const stale = [];
  const keys = new Set();
  let artifacts = 0;
  const said = from ? run.stdout.replaceAll(from, relativeTo) : run.stdout;
  for (const line of said.split("\n")) {
    if (!line.startsWith("{")) continue;
    let message;
    try { message = JSON.parse(line); } catch { continue; }
    if (message.reason === "compiler-artifact") {
      artifacts++;
      if (!message.fresh) stale.push(message.package_id.includes("registry+") ? `dep:${message.target.name}` : `workspace:${message.target.name}`);
      for (const name of message.filenames ?? []) { const key = unitKey(relative(relativeTo, name)); if (key) keys.add(key); }
      if (message.executable && message.target?.name) {
        const deps = join(dirname(message.executable), "deps");
        const size = existsSync(message.executable) ? lstatSync(message.executable).size : -1;
        const stem = `${message.target.name.replaceAll("-", "_")}-`;
        for (const entry of existsSync(deps) ? readdirSync(deps) : [])
          if (entry.startsWith(stem) && extname(entry) !== ".d" && lstatSync(join(deps, entry)).size === size) keys.add(unitKey(relative(relativeTo, join(deps, entry))));
      }
    }
    if (message.reason === "build-script-executed" && message.out_dir) { const key = unitKey(relative(relativeTo, message.out_dir)); if (key) keys.add(key); }
  }
  if (run.status !== 0) console.error(run.stderr.slice(-4000));
  return { status: run.status, artifacts, stale, keys };
}
const ARCHIVE = `${posix(TARGET)}/cache-demo-archive.tar.zst`;
const DOUBLES = (prefix, dir) =>
  ["--package onepipeline-testfakes", "--package oneagentgraph --bin oneagentgraph --locked", "--package oneharness --bin oneharness --locked", "--package onevcs --bin onevcs --locked"]
    .map((selection) => `${prefix} cargo build --offline ${selection} --target-dir "${dir}" --message-format=json`)
    .join("; ");
const LLVM = `eval "$(cargo llvm-cov show-env --export-prefix 2>/dev/null)"`;
const COV_DIR = `${posix(TARGET)}/llvm-cov-target`;
const AT_TEST_TIME = `${LLVM}; export RUSTFLAGS="-D warnings"; ${DOUBLES("", COV_DIR)}`;
const COMMON = {
  clippy: { recipe: "just _crate-lint", units: "cargo clippy --all-targets --locked --message-format=json -- -D warnings" },
  release: { recipe: "just _stop-verdict-build", units: `RUSTFLAGS="-D warnings" cargo build --release --locked --bin onepipeline --message-format=json` },
  "doubles-plain": { recipe: "just _e2e-doubles-build plain", units: DOUBLES("unset RUSTFLAGS;", posix(TARGET)) },
};
const LINES = {
  linux: {
    "instrumented-tests": { recipe: `ONEPIPELINE_TEST_ARCHIVE="${ARCHIVE}" just _crate-test-archive`, units: `${LLVM}; export RUSTFLAGS="-D warnings"; cargo test --no-run --locked --message-format=json` },
    "doubles-instrumented": { recipe: "just _e2e-doubles-build instrumented", units: AT_TEST_TIME },
    "debug-build": { recipe: "just _crate-build", units: `RUSTFLAGS="-D warnings" cargo build --locked --all-targets --message-format=json` },
    release: COMMON.release,
    doc: { recipe: "just _crate-doc", units: `RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --locked --message-format=json` },
    clippy: COMMON.clippy,
    "doubles-plain": COMMON["doubles-plain"],
    "doubles-plain-deny": { recipe: "just _e2e-doubles-build plain-deny-warnings", units: DOUBLES(`export RUSTFLAGS="-D warnings";`, posix(TARGET)) },
  },
  cross: {
    "plain-tests": { recipe: `ONEPIPELINE_TEST_ARCHIVE="${ARCHIVE}" just _crate-test-quick-archive`, units: "cargo test --no-run --locked --message-format=json" },
    clippy: COMMON.clippy,
    release: COMMON.release,
    "doubles-plain": COMMON["doubles-plain"],
  },
};
// rust-cache v2.9.2's post step: src/cleanup.ts's cleanTargetDir, cleanRegistry and
// workspace.ts's packages outside the workspace root, ported because its dist exits when no
// cache service is configured. Nothing is saved, so its real post step cannot be run here.
function packages() {
  const root = process.cwd();
  const meta = JSON.parse(sh("cargo metadata --all-features --format-version 1", { CARGO_ENCODED_RUSTFLAGS: "" }).stdout);
  return meta.packages.filter((p) => !p.manifest_path.startsWith(root)).map((p) => ({
    name: p.name, version: p.version,
    targets: p.targets.filter((t) => t.kind.some((k) => k === "lib" || k === "proc-macro")).map((t) => t.name),
  }));
}
const rm = (path) => rmSync(path, { recursive: true, force: true });
function rmExcept(dir, keep) {
  if (!existsSync(dir)) return;
  for (const name of readdirSync(dir)) {
    if (keep.has(name)) continue;
    const at = name.lastIndexOf("-");
    if (!keep.has(at === -1 ? name : name.slice(0, at))) rm(join(dir, name));
  }
}
function cleanTargetDir(dir, pkgs) {
  for (const entry of readdirSync(dir, { withFileTypes: true })) {
    const full = join(dir, entry.name);
    if (entry.isDirectory()) {
      const profile = entry.name === "tests" || ["build", ".fingerprint", "deps"].some((name) => existsSync(join(full, name)));
      if (profile) cleanProfile(full, pkgs); else cleanTargetDir(full, pkgs);
    } else if (entry.name !== "CACHEDIR.TAG") rm(full);
  }
}
function cleanProfile(dir, pkgs) {
  if (basename(dir) === "tests") {
    for (const name of ["target", "trybuild"]) if (existsSync(join(dir, name))) cleanTargetDir(join(dir, name), pkgs);
    return rmExcept(dir, new Set(["target", "trybuild"]));
  }
  rmExcept(dir, new Set(["build", ".fingerprint", "deps"]));
  const keepPkg = new Set(pkgs.flatMap((p) => [p.name, ...p.targets.map((t) => t.replaceAll("-", "_"))]));
  rmExcept(join(dir, "build"), keepPkg);
  rmExcept(join(dir, ".fingerprint"), keepPkg);
  rmExcept(join(dir, "deps"), new Set(pkgs.flatMap((p) => [p.name, ...p.targets].flatMap((n) => { const name = n.replaceAll("-", "_"); return [name, `lib${name}`]; }))));
}
function cleanRegistry(pkgs) {
  rm(join(CARGO_HOME, "credentials.toml"));
  const reg = join(CARGO_HOME, "registry");
  const names = new Set(pkgs.map((p) => p.name));
  const cleanIndex = (dir) => {
    let empty = true;
    for (const entry of readdirSync(dir, { withFileTypes: true })) {
      const full = join(dir, entry.name);
      if (entry.isDirectory()) { if (cleanIndex(full)) rm(full); else empty = false; }
      else if (names.has(entry.name)) empty = false; else rm(full);
    }
    return empty;
  };
  for (const entry of readdirSync(join(reg, "index"), { withFileTypes: true })) {
    if (!entry.isDirectory()) continue;
    const full = join(reg, "index", entry.name);
    if (existsSync(join(full, ".git"))) rm(join(full, ".cache")); else cleanIndex(full);
  }
  const sys = new Set(pkgs.filter((p) => p.name.endsWith("-sys")).map((p) => `${p.name}-${p.version}`));
  for (const dir of readdirSync(join(reg, "src"))) for (const name of readdirSync(join(reg, "src", dir))) if (!sys.has(name)) rm(join(reg, "src", dir, name));
  const crates = new Set(pkgs.map((p) => `${p.name}-${p.version}.crate`));
  for (const dir of readdirSync(join(reg, "cache"))) for (const name of readdirSync(join(reg, "cache", dir))) if (!crates.has(name)) rm(join(reg, "cache", dir, name));
}

if (verb === "doubles") {
  const asked = ask(AT_TEST_TIME, TARGET);
  out("instrumented-doubles", "status", asked.status, "artifacts", asked.artifacts, "stale", asked.stale.length, asked.stale.join(" ").slice(0, 400));
} else if (verb === "capture-archive") {
  const dir = join(TARGET, "ci-capture-target");
  const files = walk(dir);
  out("capture-archive", "files", files.length, "apparent", sum(files), "compressed", compressed(files));
  // The capture's own compile: its image, its flags and its clone, as JSON.
  const copy = `${process.env.RUNNER_TEMP}/ci-capture`;
  const inside = "/__w/onepipeline/onepipeline";
  const image = "rust:1-bookworm";
  // screenshots/capture.sh's three builds, as it runs them, each answering in JSON.
  const builds = ["--bin onepipeline", "--package onevcs --bin onevcs", "--package onepipeline-testfakes"]
    .map((selection) => `RUSTFLAGS="-D warnings" cargo build --release --locked ${selection} --message-format=json`)
    .join("; ");
  const toolchain = `$(sed -n 's/^channel *= *"\\([^"]*\\)".*/\\1/p' rust-toolchain.toml)`;
  const script = `docker run --rm -e CARGO_PROFILE_RELEASE_LTO=false -e CARGO_PROFILE_RELEASE_CODEGEN_UNITS=256 -e CARGO_PROFILE_RELEASE_OPT_LEVEL=0 -e CARGO_INCREMENTAL=0 -v "${copy}":"${inside}" -v "${dir}":"${inside}/target" -w "${inside}" -e GIT_CONFIG_COUNT=1 -e GIT_CONFIG_KEY_0=safe.directory -e GIT_CONFIG_VALUE_0="${inside}" -e RUSTUP_TOOLCHAIN="${toolchain}" ${image} sh -e -c 'rustup toolchain install "$RUSTUP_TOOLCHAIN" --profile minimal >&2; ${builds}; chown -R ${process.getuid()}:${process.getgid()} .'`;
  const asked = ask(script, dir, `${inside}/target`);
  out("capture-rerun", "status", asked.status, "artifacts", asked.artifacts, "recompiled", asked.stale.length, asked.stale.join(" ").slice(0, 400));
  const disk = new Set(walk(dir).map((f) => f.path).filter((p) => /\/(deps|build)\//.test(p)).map((p) => unitKey(p)).filter(Boolean));
  const unnamed = [...disk].filter((key) => !asked.keys.has(key));
  out("capture-units", "on-disk", disk.size, "named-by-its-build", asked.keys.size, "unnamed", unnamed.length, unnamed.slice(0, 20).join(" "));
} else if (verb === "clean-and-archive") {
  const pkgs = packages();
  rm(ARCHIVE);
  cleanTargetDir(TARGET, pkgs);
  cleanRegistry(pkgs);
  const target = walk(TARGET);
  const cargo = ["bin", ".crates.toml", ".crates2.json", "registry", "git"].flatMap((name) => {
    const full = join(CARGO_HOME, name);
    if (!existsSync(full)) return [];
    return lstatSync(full).isDirectory() ? walk(CARGO_HOME, full) : [{ root: CARGO_HOME, path: name, size: lstatSync(full).size }];
  });
  out("family-archive", leg, "packages", pkgs.length, "target-files", target.length, "target-compressed", compressed(target), "cargo-compressed", compressed(cargo), "whole-compressed", compressed([...target, ...cargo]));
  out("family-holds-capture", target.filter((f) => f.path.startsWith("ci-capture-target/")).length);
  const groups = new Map();
  for (const f of target) { const top = f.path.split("/").slice(0, 2).join("/"); groups.set(top, (groups.get(top) ?? 0) + f.size); }
  for (const [name, bytes] of [...groups].sort((a, b) => b[1] - a[1]).slice(0, 12)) out("group", name, bytes);
} else if (verb === "rerun") {
  const workspace = members();
  for (const [name, line] of Object.entries(LINES[leg])) {
    const asked = ask(line.units, TARGET);
    const deps = asked.stale.filter((s) => s.startsWith("dep:"));
    out("rerun", name, "status", asked.status, "artifacts", asked.artifacts, "dependency-recompiled", deps.length, "workspace-recompiled", asked.stale.length - deps.length, deps.join(" ").slice(0, 400));
    const started = Date.now();
    const recipe = sh(`set -o pipefail; ${line.recipe}`);
    out("rerun-recipe", name, "status", recipe.status, "seconds", Math.round((Date.now() - started) / 100) / 10);
    if (recipe.status !== 0) console.error(recipe.stderr.slice(-2000));
  }
  rm(ARCHIVE);
  void workspace;
} else {
  console.error(`measure: no verb ${verb}`);
  process.exit(2);
}
