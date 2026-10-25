// `just _ensure-prune-superseded` driven end to end, with doubles for the network it would reach.
//
// The fixture justfile carries this justfile's shell setting, its pinned version and the recipe
// verbatim. `cargo` and `cargo-prune-superseded` are scripts on PATH: the double for the installed
// pruner reports a version, and the double for cargo logs its arguments and exits as told, so the
// skip, the install and the refusal are each seen without a crates.io download.

import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import {
  chmodSync,
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
import { after, describe, it } from "node:test";
import { fileURLToPath } from "node:url";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");
const justfile = readFileSync(join(root, "justfile"), "utf8");
const pin = /^prune-superseded-version := "([^"]+)"$/m.exec(justfile);
const shell = /^set shell := .*$/m.exec(justfile);
const name = "_ensure-prune-superseded";
const body = justfile.split(`\n${name}:\n`)[1]?.split("\n\n")[0];

/** Where `tool` resolves on this process's PATH, or undefined. */
function which(tool) {
  const found = spawnSync("bash", ["-c", `command -v ${tool}`], { encoding: "utf8" });
  return found.status === 0 ? found.stdout.trim() : undefined;
}

/**
 * The recipe run in a fresh fixture. `installed` is the version the pruner on PATH reports, or
 * undefined for none on PATH; `cargoExit` is the status the cargo double exits with.
 */
function ensure(dirs, { installed, cargoExit }) {
  assert.ok(pin && shell && body, `the justfile no longer has \`${name}\` and its pin`);
  const dir = mkdtempSync(join(tmpdir(), "pool-maintenance-ensure-"));
  dirs.push(dir);
  writeFileSync(join(dir, "justfile"), `${shell[0]}\n${pin[0]}\n\n${name}:\n${body}\n`);
  const bin = join(dir, "bin");
  mkdirSync(bin);
  const script = (tool, text) => {
    writeFileSync(join(bin, tool), `#!/usr/bin/env bash\n${text}\n`);
    chmodSync(join(bin, tool), 0o755);
  };
  const log = join(dir, "cargo.log");
  script("cargo", `printf '%s\\n' "$*" >> '${log}'\nexit ${cargoExit}`);
  if (installed !== undefined) {
    script("cargo-prune-superseded", `echo "cargo-prune-superseded ${installed}"`);
  }
  const just = which("just");
  assert.ok(just, "just is not on PATH");
  symlinkSync(just, join(bin, "just"));
  const path = (process.env.PATH ?? "")
    .split(delimiter)
    .filter(
      (entry) =>
        !existsSync(join(entry, "cargo-prune-superseded")) && !existsSync(join(entry, "cargo")),
    );
  const run = spawnSync("just", [name], {
    cwd: dir,
    encoding: "utf8",
    env: { ...process.env, PATH: [bin, ...path].join(delimiter) },
  });
  const calls = existsSync(log) ? readFileSync(log, "utf8").split("\n").filter(Boolean) : [];
  return { run, calls };
}

describe(`just ${name}`, () => {
  const dirs = [];
  after(() => {
    for (const dir of dirs) rmSync(dir, { recursive: true, force: true });
  });
  const install = `install --locked cargo-prune-superseded --version ${pin?.[1]} --quiet`;

  it("skips the install when the pinned release is the one on PATH", () => {
    const { run, calls } = ensure(dirs, { installed: pin?.[1], cargoExit: 1 });
    assert.equal(run.status, 0, run.stderr);
    assert.deepEqual(calls, []);
  });

  it("installs the pinned release over a different one on PATH", () => {
    const { run, calls } = ensure(dirs, { installed: "0.0.1", cargoExit: 0 });
    assert.equal(run.status, 0, run.stderr);
    assert.deepEqual(calls, [install]);
  });

  it("installs the pinned release when none is on PATH", () => {
    const { run, calls } = ensure(dirs, { installed: undefined, cargoExit: 0 });
    assert.equal(run.status, 0, run.stderr);
    assert.deepEqual(calls, [install]);
  });

  it("refuses, naming the release, when the install fails", () => {
    const { run, calls } = ensure(dirs, { installed: "0.0.1", cargoExit: 101 });
    assert.notEqual(run.status, 0);
    assert.deepEqual(calls, [install]);
    assert.match(
      run.stderr,
      new RegExp(
        `cargo-prune-superseded ${pin?.[1].replaceAll(".", "\\.")} could not be installed; 'just pool-maintenance' refuses without it`,
      ),
    );
  });
});
