// The reuse spike's hosted compile-cost demonstration, run as its commands: the archive
// timer over a real small cargo workspace with the real cargo-nextest, and the
// demonstration commit written into a checkout, read for the cache writes a run of it
// must not make.
import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { after, before, test } from "node:test";
import { fileURLToPath } from "node:url";

const root = join(dirname(fileURLToPath(import.meta.url)), "..", "..");
const measure = join(root, "scripts", "crate-split", "measure-archive.mjs");
const demo = join(root, "scripts", "crate-split", "demo-commit.mjs");

let scratch;
before(() => {
  scratch = mkdtempSync(join(tmpdir(), "reuse-demo-"));
});
after(() => rmSync(scratch, { recursive: true, force: true }));

function write(dir, files) {
  for (const [path, body] of Object.entries(files)) {
    mkdirSync(dirname(join(dir, path)), { recursive: true });
    writeFileSync(join(dir, path), body);
  }
}

function crate(name, lib) {
  const dir = join(scratch, name);
  write(dir, {
    "Cargo.toml": `[package]\nname = "${name}"\nversion = "0.1.0"\nedition = "2021"\n\n[workspace]\n`,
    "src/lib.rs": lib,
    "tests/it.rs": `#[test]\nfn adds() { assert_eq!(${name.replace(/-/g, "_")}::add(1, 2), 3); }\n`,
  });
  // A lock, so the archive's `--locked` build has the one it requires.
  spawnSync("cargo", ["generate-lockfile", "--offline"], { cwd: dir });
  return dir;
}

const run = (dir, mode, env = {}) =>
  spawnSync(process.execPath, [measure, mode], {
    cwd: dir,
    encoding: "utf8",
    env: {
      ...process.env,
      RUNNER_TEMP: dir,
      RUSTFLAGS: "",
      CARGO_TARGET_DIR: join(dir, "target"),
      ...env,
    },
  });

test("the archive timer builds the plain archive and reports its wall and unit seconds, workspace apart", () => {
  const dir = crate("timed-crate", "pub fn add(a: i32, b: i32) -> i32 { a + b }\n");
  const result = run(dir, "quick");
  assert.equal(result.status, 0, result.stderr);
  const line = result.stdout.split("\n").find((l) => l.startsWith("measure-archive: mode="));
  assert.match(
    line,
    /^measure-archive: mode=quick wall_s=\d+\.\d units=\d+ unit_s=\d+\.\d workspace_units=(\d+) workspace_unit_s=\d+\.\d dependency_unit_s=\d+\.\d workspace=timed-crate$/,
  );
  // The lib and the integration test are the workspace's units.
  assert.ok(Number(line.match(/workspace_units=(\d+)/)[1]) >= 2, line);
  assert.match(result.stdout, /^measure-archive: unit timed-crate /m);
  assert.ok(existsSync(join(dir, "tests.tar.zst")));
});

test("the coverage archive is cargo-llvm-cov's instrumented build, under -D warnings", () => {
  const dir = crate("covered-crate", "pub fn add(a: i32, b: i32) -> i32 { a + b }\n");
  const result = run(dir, "coverage");
  assert.equal(result.status, 0, result.stderr);
  assert.match(
    result.stdout,
    /^measure-archive: mode=coverage wall_s=\d+\.\d .* workspace=covered-crate$/m,
  );
  assert.ok(existsSync(join(dir, "tests.tar.zst")));
  // -D warnings is the timer's, as test-linux's: an unused item fails the build.
  const warned = crate(
    "warned-crate",
    "fn unused() {}\npub fn add(a: i32, b: i32) -> i32 { a + b }\n",
  );
  const failed = run(warned, "coverage");
  assert.notEqual(failed.status, 0);
  assert.match(failed.stderr, /the coverage archive build failed/);
});

test("an archive that does not build fails with cargo's status and says how long it ran", () => {
  const dir = crate("broken-crate", "pub fn add(a: i32, b: i32) -> i32 { a + }\n");
  const result = run(dir, "quick");
  assert.notEqual(result.status, 0);
  assert.match(result.stderr, /measure-archive: the quick archive build failed after \d+\.\d s/);
  assert.doesNotMatch(result.stdout, /measure-archive: mode=/);
});

test("an archive the timer does not know is refused before cargo runs", () => {
  const result = run(scratch, "release");
  assert.equal(result.status, 2);
  assert.match(result.stderr, /name the archive: coverage or quick/);
});

function checkout(name) {
  const dir = join(scratch, name);
  write(dir, {
    ".github/workflows/ci.yml": "name: CI\n",
    ".github/workflows/visual-docs.yml": "name: Visual docs\n",
    "Cargo.toml":
      '[package]\nname = "x"\nversion = "0.1.0"\n\n[[test]]\nname = "e2e"\npath = "tests/e2e/main.rs"\n\n[[test]]\nname = "other"\npath = "tests/other.rs"\n',
    "tests/e2e/main.rs": "mod harness;\nmod landing;\nmod store;\n",
    "scripts/crate-split/proposed.json": JSON.stringify({
      suites: {
        "e2e-landing": { paths: ["tests/e2e/landing.rs", "tests/e2e/store.rs", "graphs/**"] },
        "e2e-store": { paths: ["tests/e2e/store.rs"] },
      },
    }),
  });
  return dir;
}

const demoRun = (dir, variant) =>
  spawnSync(process.execPath, [demo, "--repo", dir, "--variant", variant], { encoding: "utf8" });

test("every demonstration replaces CI with the timed build and writes to no cache", () => {
  const dir = checkout("demo-today");
  const result = demoRun(dir, "today");
  assert.equal(result.status, 0, result.stderr);
  assert.ok(!existsSync(join(dir, ".github", "workflows", "visual-docs.yml")));
  const workflow = readFileSync(join(dir, ".github", "workflows", "ci.yml"), "utf8");
  for (const platform of ["linux", "macos", "windows"])
    assert.match(workflow, new RegExp(`^  measure-${platform}:$`, "m"));
  assert.match(workflow, /node scripts\/crate-split\/measure-archive\.mjs coverage/);
  assert.equal(
    (workflow.match(/node scripts\/crate-split\/measure-archive\.mjs quick/g) ?? []).length,
    2,
  );
  // Each Rust cache step restores and saves nothing; nothing else touches a cache.
  const restores = workflow.split("uses: Swatinem/rust-cache@").slice(1);
  assert.equal(restores.length, 3);
  for (const step of restores) assert.match(step.split("- ")[0], /save-if: false/);
  assert.doesNotMatch(
    workflow,
    /actions\/cache|setup-uv|nx-remote-cache|github-cache|upload-artifact/,
  );
  assert.match(workflow, /sample: \[1, 2\]/);
  // A GitHub expression is written for Actions, not interpolated by the generator.
  assert.match(workflow, /node-version: \$\{\{ steps\.ci-tools\.outputs\.node \}\}/);
  // The demonstration's workflow is the same bytes whichever layout it measures.
  const b = checkout("demo-b");
  assert.equal(demoRun(b, "b").status, 0);
  assert.equal(readFileSync(join(b, ".github", "workflows", "ci.yml"), "utf8"), workflow);
});

test("demonstration b replaces the one e2e binary with a target per suite", () => {
  const dir = join(scratch, "demo-b");
  const manifest = readFileSync(join(dir, "Cargo.toml"), "utf8");
  assert.doesNotMatch(manifest, /name = "e2e"\n/);
  assert.match(manifest, /name = "other"/);
  assert.match(manifest, /name = "e2e_landing"\npath = "tests\/e2e_split\/e2e_landing\.rs"/);
  const landing = readFileSync(join(dir, "tests", "e2e_split", "e2e_landing.rs"), "utf8");
  // Its modules and the harness, from where they are; a glob path names no module.
  assert.match(landing, /#\[path = "\.\.\/e2e\/harness\.rs"\]\nmod harness;/);
  assert.match(landing, /mod landing;/);
  assert.match(landing, /mod store;/);
  const store = readFileSync(join(dir, "tests", "e2e_split", "e2e_store.rs"), "utf8");
  assert.doesNotMatch(store, /mod landing;/);
});

test("a demonstration of a layout it does not know is refused", () => {
  const result = demoRun(checkout("demo-unknown"), "d");
  assert.notEqual(result.status, 0);
  assert.match(result.stderr, /--variant d is not today, b or c/);
});
