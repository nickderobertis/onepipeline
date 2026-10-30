// What the npm journeys share: the host's own platform, the cargo build they
// stage without contending with the rest of the gate, and the pack-then-install a
// user's `npm install` amounts to.

import { execFileSync } from "node:child_process";
import { dirname, join, resolve } from "node:path";
import assert from "node:assert/strict";
import { fileURLToPath } from "node:url";
import { spawnable } from "./spawn-support.mjs";

export const REPO_ROOT = resolve(dirname(fileURLToPath(import.meta.url)), "..", "..");

/// The Rust target triple for the host this test runs on — the one platform
/// package that can carry a binary this machine can execute.
export function hostTarget() {
  const key = `${process.platform}-${process.arch}`;
  const targets = {
    "linux-x64": "x86_64-unknown-linux-gnu",
    "linux-arm64": "aarch64-unknown-linux-gnu",
    "darwin-arm64": "aarch64-apple-darwin",
    "win32-x64": "x86_64-pc-windows-msvc",
  };
  const target = targets[key];
  assert.ok(target, `no prebuilt npm package exists for ${key}`);
  return target;
}

// The staged release builds compile the crate for an explicit `--target`, which
// takes minutes from cold. In the shared `target/` they would hold its build lock
// all that time, and a test elsewhere in the gate building into `target/debug`
// (`tests/build_config.rs`) timed out waiting behind them. Their own directory
// keeps them out of that lock and still reuses its build on the next run.
export const STAGED_TARGET_DIR = join(REPO_ROOT, "target", "npm-staged");

// The debug build `publish-gate.test.mjs` packs, under the flags of every other
// debug build the gate makes (`just _crate-build`, the stop-verdict tier). Cargo
// keys a build on its flags, so an unflagged one compiled the crate and every
// dependency into a second tree; `workspace-nx.test.mjs` holds this one to the
// tree `_crate-build` leaves.
export const GATE_DEBUG_BUILD = {
  args: ["build", "--locked", "--quiet"],
  env: { RUSTFLAGS: "-D warnings" },
};

export function run(command, args, options = {}) {
  return execFileSync(
    ...spawnable(command, args, {
      cwd: REPO_ROOT,
      encoding: "utf8",
      ...options,
      env: { ...process.env, CARGO_TARGET_DIR: STAGED_TARGET_DIR, ...options.env },
    }),
  );
}

/// Pack a package directory into the tarball the registry would serve.
///
/// Installing the directory instead would symlink it, and node resolves a
/// symlinked package's dependencies from its *realpath* — so the launcher would
/// look for its platform package beside the build output rather than in the
/// project's node_modules, and never find it. A tarball is both the real shape
/// and the one `release.yml` publishes.
export function pack(dir, into) {
  const packed = JSON.parse(
    run("npm", ["pack", "--json", "--pack-destination", into, dir], {
      stdio: ["ignore", "pipe", "ignore"],
    }),
  );
  assert.equal(packed.length, 1, "npm pack must produce exactly one tarball");
  return join(into, packed[0].filename);
}

/// Install the given packed packages into a throwaway project.
/// `--omit=optional` keeps npm from reaching the registry for the launcher's
/// per-platform pins, which exist only once a release has published them: the
/// platform package under test is passed explicitly instead when it is wanted.
export function installInto(project, packages) {
  run("npm", [
    "install",
    "--prefix",
    project,
    "--no-audit",
    "--no-fund",
    "--omit=optional",
    ...packages,
  ]);
}
