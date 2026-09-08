// The npm publish order, driven end to end against a registry that lags.
//
// `release.yml`'s publish job publishes the five platform packages and then the
// launcher that pins them. That order was always written; what was missing is
// that `npm publish` exiting 0 does not mean the registry *serves* the version.
// On v0.23.0 three of the five became resolvable seventy-five seconds after the
// publish job had already finished, `verify-npm` installed the launcher four
// seconds after that job ended, npm silently skipped the optional dependencies
// it could not resolve, and the launcher could not start. The evidence is quoted
// in `scripts/publish-npm.sh`.
//
// So the property under test is not "the loop is in the right order" — it always
// was. It is: **the launcher is not offered before every platform package its
// own manifest pins is resolvable**. That needs a registry that can acknowledge
// a publish and serve it later, which is exactly what
// `npm/test/registry-support.mjs` is.
//
// Nothing here is stubbed except the registry itself, which is the environment
// rather than the layer under test: the packages come from the real
// `scripts/npm-build.mjs` around the real compiled binary, the publishing is the
// real `scripts/publish-npm.sh` driving the real `npm` CLI, and the resolution
// is a real `npm install` followed by running what it put on PATH.
//
// What can only be observed against npmjs.org itself — how long that registry
// actually lags, and whether a given release's legs went green — is named in
// README.md's "Release outcome" section and proven by `release.yml`'s own
// verify jobs on every release.

import { execFile } from "node:child_process";
import { copyFileSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { after, before, describe, it } from "node:test";
import assert from "node:assert/strict";
import { promisify } from "node:util";
import { fileURLToPath } from "node:url";
import { Registry } from "./registry-support.mjs";

const REPO_ROOT = resolve(dirname(fileURLToPath(import.meta.url)), "..", "..");
const execFileAsync = promisify(execFile);

/// Everything here spawns npm, which talks to a registry served by *this*
/// process — so every wait is asynchronous. A synchronous spawn would block the
/// event loop the registry runs on, and npm would hang against a socket nothing
/// was accepting.
async function run(command, args, options = {}) {
  const { stdout } = await execFileAsync(command, args, {
    cwd: REPO_ROOT,
    encoding: "utf8",
    maxBuffer: 32 * 1024 * 1024,
    ...options,
  });
  return stdout;
}

/// Run and return the failure instead of throwing it, for the journeys whose
/// subject is a refusal.
async function attempt(command, args, options = {}) {
  try {
    return { code: 0, stdout: await run(command, args, options), stderr: "" };
  } catch (error) {
    return {
      code: error.code ?? error.status ?? 1,
      stdout: error.stdout ?? "",
      stderr: error.stderr ?? "",
    };
  }
}

/// The Rust target triple for the host, the one platform package that can carry
/// a binary this machine can execute.
function hostTarget() {
  const key = `${process.platform}-${process.arch}`;
  const target = {
    "linux-x64": "x86_64-unknown-linux-gnu",
    "linux-arm64": "aarch64-unknown-linux-gnu",
    "darwin-x64": "x86_64-apple-darwin",
    "darwin-arm64": "aarch64-apple-darwin",
    "win32-x64": "x86_64-pc-windows-msvc",
  }[key];
  assert.ok(target, `no prebuilt npm package exists for ${key}`);
  return target;
}

/// `scripts/npm-build.mjs`'s TARGETS table, read out of the script rather than
/// restated: triple -> npm platform package name. `platform-matrix.test.mjs`
/// holds that table to the launcher's manifest and to the release matrices, so
/// reading it here is reading all four.
function declaredTargets() {
  const source = readFileSync(join(REPO_ROOT, "scripts", "npm-build.mjs"), "utf8");
  const start = source.indexOf("const TARGETS = {");
  assert.notEqual(start, -1, "no TARGETS table in scripts/npm-build.mjs");
  const table = source.slice(start, source.indexOf("\n};", start));
  const targets = new Map();
  for (const [, triple, platform, arch] of table.matchAll(
    /"([^"]+)": \{ platform: "([^"]+)", arch: "([^"]+)"/g,
  )) {
    targets.set(triple, `onepipeline-cli-${platform}-${arch}`);
  }
  assert.ok(targets.size > 0, "scripts/npm-build.mjs declares no targets");
  return targets;
}

describe("the npm publish order", () => {
  let work;
  let version;
  let hostBinary;
  /// triple -> { dir, tgz, package }
  const platforms = new Map();
  let launcherDir;
  let launcherTgz;
  let registry;
  let env;

  /// The host binary the packages carry, stripped where the host can strip.
  ///
  /// A debug `onepipeline` is a quarter of a gigabyte of symbols, and this suite
  /// packs it, base64s it through a registry and installs it several times. A
  /// *copy* with its debug sections removed is the same compiled program — same
  /// code, same `--version` — at a size the journeys can afford. Where `strip`
  /// is not on PATH the original is used, so nothing here depends on it.
  async function payload(binary) {
    const copy = join(work, process.platform === "win32" ? "onepipeline.exe" : "onepipeline");
    copyFileSync(join(REPO_ROOT, binary), copy);
    try {
      await run("strip", [copy]);
    } catch {
      // No `strip` here, or it refused this object: the unstripped copy is what
      // the packages carry, which is slower and equally correct.
    }
    return copy;
  }

  /// Pack a package directory into the tarball a registry serves, which is the
  /// shape `release.yml` hands `scripts/publish-npm.sh`.
  async function pack(dir, into) {
    const packed = JSON.parse(
      await run("npm", ["pack", "--json", "--pack-destination", into, dir], {
        stdio: ["ignore", "pipe", "ignore"],
      }),
    );
    assert.equal(packed.length, 1, "npm pack must produce exactly one tarball");
    return join(into, packed[0].filename);
  }

  before(
    async () => {
      version = JSON.parse(
        await run("cargo", ["metadata", "--no-deps", "--format-version", "1", "--locked"]),
      ).packages.find((pkg) => pkg.name === "onepipeline").version;

      // The real binary the host's package carries. Debug rather than release,
      // for the reason `launcher.test.mjs` builds debug: this proves the
      // packaging and the publish order, and a release build would cost the gate
      // minutes.
      await run("cargo", ["build", "--locked", "--quiet"]);
      work = mkdtempSync(join(tmpdir(), "onepipeline-publish-"));
      hostBinary = await payload(
        join("target", "debug", process.platform === "win32" ? "onepipeline.exe" : "onepipeline"),
      );

      const dist = join(work, "dist");
      const tarballs = join(work, "tgz");
      mkdirSync(tarballs);

      // A stand-in for the four platforms this machine cannot execute. The
      // packaging is the real script either way — same manifest, same os/cpu,
      // same layout — but these four are packed, base64'd through a registry and
      // installed several times below, and a quarter of a gigabyte of foreign
      // debug symbols proves nothing the host's own package does not. The host's
      // is the real compiled binary, and it is the one the launcher execs.
      const standIn = join(work, "stand-in-binary");
      writeFileSync(standIn, "not this machine's architecture\n");

      for (const [triple, name] of declaredTargets()) {
        const dir = (
          await run("node", [
            "scripts/npm-build.mjs",
            "platform",
            "--target",
            triple,
            "--binary",
            triple === hostTarget() ? hostBinary : standIn,
            "--out",
            dist,
          ])
        ).trim();
        platforms.set(triple, { dir, name, tgz: await pack(dir, tarballs) });
      }

      launcherDir = (
        await run("node", ["scripts/npm-build.mjs", "launcher", "--out", dist])
      ).trim();
      launcherTgz = await pack(launcherDir, tarballs);
    },
    { timeout: 900_000 },
  );

  after(() => {
    if (work) rmSync(work, { recursive: true, force: true });
  });

  /// A registry of this test's own, and the npm environment that reaches it.
  async function freshRegistry() {
    if (registry) await registry.stop();
    registry = new Registry();
    await registry.start();
    env = registry.npmEnv(mkdtempSync(join(work, "npmrc-")));
    return registry;
  }

  after(async () => {
    if (registry) await registry.stop();
  });

  /// The publish step `release.yml` runs, verbatim in shape: every platform
  /// tarball, then the launcher.
  async function publishAsTheReleaseDoes(extraEnv = {}) {
    for (const { tgz } of platforms.values()) {
      await run("bash", ["scripts/publish-npm.sh", tgz], { env: { ...env, ...extraEnv } });
    }
    return attempt("bash", ["scripts/publish-npm.sh", launcherTgz], {
      env: { ...env, ...extraEnv },
    });
  }

  /// Install the launcher from the registry into a throwaway project and run it.
  async function installAndLaunch(args) {
    const project = mkdtempSync(join(work, "app-"));
    await run("npm", ["install", "--prefix", project, `onepipeline-cli@${version}`], { env });
    const bin = join(project, "node_modules", ".bin", "onepipeline");
    return attempt(bin, args, { cwd: project });
  }

  it("assembles a real package for every platform the launcher's manifest declares", () => {
    const manifest = JSON.parse(readFileSync(join(launcherDir, "package.json"), "utf8"));
    const declared = Object.keys(manifest.optionalDependencies).sort();
    assert.deepEqual(
      [...platforms.values()].map((p) => p.name).sort(),
      declared,
      "a platform the launcher pins was never assembled",
    );
    for (const { dir, name } of platforms.values()) {
      const platform = JSON.parse(readFileSync(join(dir, "package.json"), "utf8"));
      assert.equal(platform.name, name);
      assert.equal(platform.version, version, `${name} must carry the release version`);
      assert.equal(manifest.optionalDependencies[name], version, `${name} must be pinned exactly`);
      assert.equal(platform.files.length, 1, `${name} must ship exactly its binary`);
      // The payload is in the package, not a promise of one. For the host's
      // package that payload is the compiled binary, and the journeys below
      // install it and run it.
      assert.ok(
        readFileSync(join(dir, platform.files[0])).length > 0,
        `${name} carries no binary at ${platform.files[0]}`,
      );
    }
  });

  it("does not offer the launcher until the registry serves every package it pins", async () => {
    const reg = await freshRegistry();
    // The registry acknowledges these and serves them later — which is what
    // npmjs.org did to darwin-arm64, darwin-x64 and linux-arm64 on v0.23.0.
    //
    // Two lags, and they are what make this journey an assertion rather than a
    // description. A lag on a package published early has already elapsed by the
    // time the launcher is reached, and would leave this green with nothing
    // waiting at all — measured: with the readback removed from
    // `scripts/publish-npm.sh`, a 4-second lag on packages published first left
    // this passing. So the lags sit on the *last two* packages the loop
    // publishes, where each is longer than what remains of the loop after it:
    // without the readback the launcher reaches the registry first, and this
    // fails on exactly the ordering the release broke.
    const order = [...platforms.values()].map(({ name }) => name);
    reg.lagFor(order.at(-2), 4000);
    reg.lagFor(order.at(-1), 12_000);

    const published = await publishAsTheReleaseDoes();
    assert.equal(published.code, 0, published.stderr);

    const offered = reg.acceptedAt(`onepipeline-cli@${version}`);
    assert.ok(offered, "the launcher never reached the registry");
    for (const { name } of platforms.values()) {
      const resolvable = reg.visibleAt(`${name}@${version}`);
      assert.ok(
        resolvable !== undefined && resolvable <= offered,
        resolvable === undefined
          ? `the launcher was offered while ${name} was still not resolvable — an install ` +
              "between the two silently skips it and puts a launcher with no binary on PATH"
          : `the launcher was offered ${offered - resolvable}ms before ${name} was resolvable`,
      );
    }

    // And what a user gets is the binary, not a shim with nothing to exec.
    const reported = await installAndLaunch(["--version"]);
    assert.equal(reported.code, 0, reported.stderr);
    assert.match(reported.stdout, new RegExp(version.replace(/\./g, "\\.")));
  });

  it("would leave a launcher that cannot start if it were offered early", async () => {
    const reg = await freshRegistry();
    const host = platforms.get(hostTarget());
    reg.lagFor(host.name, 60_000);

    // Publish the platform packages the gate would have waited for, then offer
    // the launcher *without* the gate — `npm publish` directly, which is what
    // exiting 0 on an accepted upload used to amount to.
    for (const { tgz } of platforms.values()) {
      await run("npm", ["publish", tgz, "--access", "public"], { env });
    }
    await run("npm", ["publish", launcherTgz, "--access", "public"], { env });

    const failed = await installAndLaunch(["--version"]);
    assert.notEqual(failed.code, 0, "the launcher started against a package npm never installed");
    assert.match(failed.stderr, new RegExp(`platform package ${host.name} is not installed`));
  });

  it("refuses rather than offering a launcher the registry cannot resolve", async () => {
    const reg = await freshRegistry();
    const host = platforms.get(hostTarget());
    reg.lagFor(host.name, 60_000);
    for (const { tgz, name } of platforms.values()) {
      if (name !== host.name) await run("npm", ["publish", tgz, "--access", "public"], { env });
    }
    await run("npm", ["publish", host.tgz, "--access", "public"], { env });

    const refused = await attempt("bash", ["scripts/publish-npm.sh", launcherTgz], {
      env: { ...env, PUBLISH_NPM_AWAIT_BUDGET: "2", PUBLISH_NPM_AWAIT_INTERVAL: "1" },
    });
    assert.notEqual(refused.code, 0, "an unresolvable pin was published over");
    assert.match(refused.stderr, new RegExp(`does not serve '${host.name}@${version}'`));
    assert.equal(
      reg.acceptedAt(`onepipeline-cli@${version}`),
      undefined,
      "the launcher was published despite the refusal",
    );
  });
});
