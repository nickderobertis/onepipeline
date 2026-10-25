// measure.sh driven end to end over a two-crate cargo workspace, never over this repository.
//
// The fixture carries measure.sh verbatim, this justfile's own `pool-maintenance` and
// `_crate-build` recipes, and crates named as this workspace's are, so the alternative
// configuration's package overrides land on them. It builds in seconds, which is what keeps
// this journey in the project's `test` target while the real measurement stays out of it.

import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import {
  copyFileSync,
  existsSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  rmSync,
  symlinkSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { delimiter, dirname, join } from "node:path";
import { after, before, describe, it } from "node:test";
import { fileURLToPath } from "node:url";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");
const justfile = readFileSync(join(root, "justfile"), "utf8");
const pinned = /^prune-superseded-version := "([^"]+)"$/m.exec(justfile)?.[1];

/** The text of one recipe of this repository's justfile, up to the blank line ending it. */
function recipe(name) {
  const body = justfile.split(`\n${name}:\n`)[1]?.split("\n\n")[0];
  assert.ok(body, `the justfile no longer has a \`${name}\` recipe`);
  return `${name}:\n${body}\n`;
}

/** Where `tool` resolves on this process's PATH, or undefined. */
function which(tool) {
  const found = spawnSync("bash", ["-c", `command -v ${tool}`], { encoding: "utf8" });
  return found.status === 0 ? found.stdout.trim() : undefined;
}

/** A fresh fixture workspace holding measure.sh and the declared recipes. */
function fixture() {
  const dir = mkdtempSync(join(tmpdir(), "pool-maintenance-measure-"));
  const write = (path, text) => {
    mkdirSync(dirname(join(dir, path)), { recursive: true });
    writeFileSync(join(dir, path), text);
  };
  copyFileSync(join(root, "rust-toolchain.toml"), join(dir, "rust-toolchain.toml"));
  write("pool-maintenance/measure.sh", readFileSync(join(root, "pool-maintenance/measure.sh")));
  write("justfile", `${recipe("_crate-build")}\n${recipe("pool-maintenance")}`);
  write(
    "Cargo.toml",
    '[package]\nname = "onepipeline"\nversion = "0.0.0"\nedition = "2021"\n\n[workspace]\nmembers = ["testfakes"]\n',
  );
  write(
    "src/lib.rs",
    "pub fn one() -> u8 {\n    1\n}\n\n#[test]\nfn one_is_one() {\n    assert_eq!(one(), 1);\n}\n",
  );
  write(
    "testfakes/Cargo.toml",
    '[package]\nname = "onepipeline-testfakes"\nversion = "0.0.0"\nedition = "2021"\n',
  );
  write("testfakes/src/lib.rs", "pub fn two() -> u8 {\n    2\n}\n");
  const lock = spawnSync("cargo", ["generate-lockfile", "--offline", "--quiet"], {
    cwd: dir,
    encoding: "utf8",
  });
  assert.equal(lock.status, 0, `cargo could not lock the fixture:\n${lock.stderr}`);
  return dir;
}

/** measure.sh run in `dir` with `env` layered over a cargo environment of its own. */
function measure(dir, env = {}) {
  const clean = { ...process.env };
  for (const name of ["ONEBUDGETSPEC_RESULT", "CARGO_TARGET_DIR", "CARGO_BUILD_TARGET_DIR"]) {
    delete clean[name];
  }
  return spawnSync("bash", [join(dir, "pool-maintenance", "measure.sh")], {
    cwd: dir,
    encoding: "utf8",
    env: { ...clean, ...env },
  });
}

const lines = (dir, name) =>
  readFileSync(join(dir, "target/pool-maintenance-budget", `${name}.txt`), "utf8")
    .split("\n")
    .filter(Boolean);

describe("measure.sh over a small cargo workspace", () => {
  const dirs = [];
  after(() => {
    for (const dir of dirs) rmSync(dir, { recursive: true, force: true });
  });

  describe("with the pinned pruner on PATH", () => {
    let dir;
    let run;
    let result;
    before(() => {
      const installed = spawnSync("cargo-prune-superseded", ["--version"], { encoding: "utf8" });
      assert.equal(
        installed.stdout?.trim(),
        `cargo-prune-superseded ${pinned}`,
        `this journey needs cargo-prune-superseded ${pinned} on PATH: run 'just bootstrap', then re-run`,
      );
      dir = fixture();
      dirs.push(dir);
      result = join(dir, "result.json");
      run = measure(dir, { ONEBUDGETSPEC_RESULT: result });
    });

    it("exits 0 after the timed pass's per-target line", () => {
      assert.equal(run.status, 0, run.stderr);
      assert.match(run.stderr, /\(d\) timed pass\ncargo-prune-superseded: target: removed [1-9]/);
    });

    it("reports the timed pass alone, with the backlog pass as uncounted detail", () => {
      const reported = JSON.parse(readFileSync(result, "utf8"));
      assert.equal(typeof reported.value, "number");
      const removed = Number(/builds removed: (\d+)/.exec(reported.detail)?.[1]);
      assert.ok(removed > 0, reported.detail);
      assert.match(reported.detail, /; untimed backlog pass seconds, not counted: [\d.]+$/);
      assert.match(reported.detail, /seconds walking: [\d.]+; seconds removing: [\d.]+/);
    });

    it("removes only the alternative build's entries and keeps every ordinary one", () => {
      const alternative = new Set(lines(dir, "alternative"));
      const removed = lines(dir, "removed");
      assert.ok(removed.length > 0, "the timed pass removed nothing");
      for (const entry of removed) {
        assert.ok(alternative.has(entry), `${entry} was not the alternative build's`);
        assert.ok(!existsSync(join(dir, entry)), `${entry} is still on disk`);
      }
      const ordinary = lines(dir, "ordinary");
      assert.ok(ordinary.length > 0, "the ordinary build left no entries");
      for (const entry of ordinary) assert.ok(existsSync(join(dir, entry)), `${entry} was removed`);
      assert.deepEqual(lines(dir, "ordinary-missing"), []);
    });
  });

  it("refuses without the pruner, saying how to install it, and reports no figure", () => {
    const dir = fixture();
    dirs.push(dir);
    const bin = join(dir, "bin");
    mkdirSync(bin);
    for (const tool of ["just", "node"]) {
      const path = which(tool);
      assert.ok(path, `${tool} is not on PATH`);
      symlinkSync(path, join(bin, tool));
    }
    const path = [bin, ...(process.env.PATH ?? "").split(delimiter)]
      .filter((entry) => !existsSync(join(entry, "cargo-prune-superseded")))
      .join(delimiter);
    const result = join(dir, "result.json");
    const run = measure(dir, { PATH: path, ONEBUDGETSPEC_RESULT: result });
    assert.notEqual(run.status, 0, run.stderr);
    assert.match(run.stderr, /cargo-prune-superseded is not on PATH: run 'just bootstrap'/);
    assert.ok(!existsSync(result), "a figure was reported for a pass that never ran");
  });
});
