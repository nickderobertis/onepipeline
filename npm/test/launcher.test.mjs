// The npm distribution's real journeys: assemble the packages the release
// publishes, install them the way a user does, and run what npm put on PATH.
//
// Nothing here is stubbed. `scripts/npm-build.mjs` assembles the real packages
// around the real compiled binary, `npm install` resolves them, and the launcher
// resolves the platform package and execs the binary. The one thing this cannot
// do locally is publish to the registry; `.github/workflows/release.yml` and
// `published-smoke.yml` cover that against the real npm.

import { execFileSync, spawnSync } from "node:child_process";
import { mkdirSync, mkdtempSync, readFileSync, existsSync, writeFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { after, before, describe, it } from "node:test";
import assert from "node:assert/strict";
import { fileURLToPath } from "node:url";

const REPO_ROOT = resolve(dirname(fileURLToPath(import.meta.url)), "..", "..");

/// The Rust target triple for the host this test runs on — the one platform
/// package that can carry a binary this machine can execute.
function hostTarget() {
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

function run(command, args, options = {}) {
  return execFileSync(command, args, {
    cwd: REPO_ROOT,
    encoding: "utf8",
    ...options,
  });
}

/// Pack a package directory into the tarball the registry would serve.
///
/// Installing the directory instead would symlink it, and node resolves a
/// symlinked package's dependencies from its *realpath* — so the launcher would
/// look for its platform package beside the build output rather than in the
/// project's node_modules, and never find it. A tarball is both the real shape
/// and the one `release.yml` publishes.
function pack(dir, into) {
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
function installInto(project, packages) {
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

/// Invoke the installed launcher, returning its exit code, stdout, and stderr.
function launch(project, args) {
  const bin = join(project, "node_modules", ".bin", "onepipeline");
  try {
    const stdout = run(bin, args, { cwd: project, stdio: ["ignore", "pipe", "pipe"] });
    return { code: 0, stdout, stderr: "" };
  } catch (error) {
    return {
      code: error.status,
      stdout: error.stdout ?? "",
      stderr: error.stderr ?? "",
    };
  }
}

describe("the npm distribution", () => {
  let work;
  let dist;
  let launcherDir;
  let platformDir;
  let launcherTgz;
  let platformTgz;
  let version;

  before(() => {
    version = JSON.parse(
      run("cargo", ["metadata", "--no-deps", "--format-version", "1", "--locked"]),
    ).packages.find((pkg) => pkg.name === "onepipeline").version;

    // The real binary the package will carry. Debug rather than release: this
    // proves the packaging, and a release build would cost the gate minutes.

    work = mkdtempSync(join(tmpdir(), "onepipeline-npm-"));
    dist = join(work, "dist");
    run(process.platform === "win32" ? "python" : "python3", [
      "scripts/build-staged-release.py",
      "--target",
      hostTarget(),
      "--profile",
      "dev",
      "--install",
      "--out",
      dist,
    ]);
    const record = JSON.parse(readFileSync(join(dist, `${hostTarget()}.json`), "utf8"));
    platformDir = join(dist, "npm", `onepipeline-cli-${process.platform}-${process.arch}`);
    platformTgz = record.npm;
    launcherDir = run("node", ["scripts/npm-build.mjs", "launcher", "--out", dist]).trim();

    const tarballs = join(work, "tgz");
    mkdirSync(tarballs);

    launcherTgz = pack(launcherDir, tarballs);
  });

  after(() => {
    if (work) rmSync(work, { recursive: true, force: true });
  });

  it("stamps the crate's version into the launcher and its platform pins", () => {
    const manifest = JSON.parse(readFileSync(join(launcherDir, "package.json"), "utf8"));
    assert.equal(manifest.version, version, "the launcher takes its version from Cargo.toml");
    for (const [name, pin] of Object.entries(manifest.optionalDependencies)) {
      assert.equal(pin, version, `${name} must be pinned to the release version`);
    }
    const platform = JSON.parse(readFileSync(join(platformDir, "package.json"), "utf8"));
    assert.equal(platform.version, version);
    assert.deepEqual(platform.os, [process.platform]);
    assert.deepEqual(platform.cpu, [process.arch]);
  });

  it("reuses the wheel payload byte for byte in the archive and npm tarball", () => {
    const result = run(process.platform === "win32" ? "python" : "python3", [
      "scripts/build-staged-release.py",
      "--verify",
      join(dist, `${hostTarget()}.json`),
    ]);
    assert.match(result, /verified binary reuse/);
  });

  it("delivers real native packages through the declared flat Darwin publication transport", () => {
    const workflow = readFileSync(join(REPO_ROOT, ".github/workflows/release.yml"), "utf8");
    const record = JSON.parse(readFileSync(join(dist, `${hostTarget()}.json`), "utf8"));
    for (const [artifact, field, publication] of [
      ["wheels-darwin", "wheel", "dist"],
      ["npm-darwin", "npm", "npm-artifacts"],
    ]) {
      const matches = [
        ...workflow.matchAll(new RegExp(`name: ${artifact}\\n\\s+path: ([^\\n]+)`, "g")),
      ];
      assert.equal(matches.length, 1, `${artifact} has one declared upload source`);
      const downloaded = join(work, "transport", publication);
      const files = JSON.parse(
        run(process.platform === "win32" ? "python" : "python3", [
          "npm/test/artifact-transport.py",
          matches[0][1].trim(),
          dist,
          downloaded,
        ]),
      );
      assert.equal(files.length, 1, "this native journey builds one real host target");
      const expected = join(downloaded, record[field].split(/[\\/]/).at(-1));
      assert.ok(existsSync(expected), `${artifact} must reach the publisher's flat directory`);
      assert.deepEqual(readFileSync(expected), readFileSync(record[field]));
    }
    assert.match(workflow, /packages-dir: dist/);
    assert.match(workflow, /npm-artifacts\/\*\.tgz/);
  });

  it("refuses a staged record whose payload hash disagrees with the real packages", () => {
    const record = JSON.parse(readFileSync(join(dist, `${hostTarget()}.json`), "utf8"));
    record.binary_sha256 = "0".repeat(64);
    const bad = join(work, "invalid-record.json");
    writeFileSync(bad, JSON.stringify(record));
    assert.throws(
      () =>
        run(
          process.platform === "win32" ? "python" : "python3",
          ["scripts/build-staged-release.py", "--verify", bad],
          { stdio: ["ignore", "pipe", "pipe"] },
        ),
      /payload hash mismatch/,
    );
  });

  for (const [mutation, reason] of [
    ...Array.from({ length: 6 }, (_, i) => [`version-invalid-${i}`, "invalid staged version"]),
    ["wheel-primary", "omitted the primary executable"],
    ["npm-missing", "omitted required members"],
    ["hash-syntax", "invalid payload hash"],
    ["inventory-empty", "invalid executable inventory"],
    ["inventory-type", "invalid executable inventory"],
    ["inventory-unsafe", "invalid executable inventory"],
    ["wheel-encrypted-flagged", "encrypted"],
    ["archive-truncated", "Compressed file ended"],
    ["wheel-duplicate", "invalid wheel executable inventory"],
    ["wheel-path", "invalid wheel executable inventory"],
    ["sbom-object", "invalid SBOM"],
    ["npm-object", "invalid npm manifest object"],
    ["npm-directory", "unexpected npm archive member"],
    ["npm-name", "npm target identity mismatch"],
    ["npm-os", "npm target identity mismatch"],
    ["npm-cpu", "npm target identity mismatch"],
    ["npm-unsafe", "unsafe or duplicate"],
    ["npm-duplicate", "unsafe or duplicate"],
    ["npm-link", "unexpected npm archive member"],
    ["npm-unexpected", "unexpected npm archive member"],
    ["npm-executable", "unexpected executable npm archive member"],
    ["archive-doc-directory", "not a regular file"],
    ["archive-doc-symlink", "not a regular file"],
    ["archive-doc-hardlink", "not a regular file"],
    ["archive-directory", "not a regular file"],
    ["schema", "unsupported staged record schema"],
    ["unknown", "missing or unknown fields"],
    ["missing", "missing or unknown fields"],
    ["field-type", "invalid staged record field"],
    ["target", "unsupported target"],
    ["tag", "tag/version mismatch"],
    ["inventory", "inventory differs"],
    ["wheel-version", "wheel name/version mismatch"],
    ["wheel-arch", "wheel architecture tag mismatch"],
    ["sbom", "invalid SBOM"],
    ["no-sbom", "missing SBOM"],
    ["no-metadata", "wheel is missing its .dist-info/METADATA"],
    ["no-wheel-tags", "wheel is missing its .dist-info/WHEEL"],
    ["wheel-traversal", "unsafe wheel member: ../escape"],
    ["wheel-symlink", "wheel member is not a regular file"],
    ["wheel-dist-info", "exactly one .dist-info directory"],
    ["macho", "Mach-O architecture mismatch"],
    ["archive-layout", "archive layout mismatch"],
    ["archive-payload", "archive executable differs"],
    ["npm-payload", "npm executable differs"],
    ["npm-version", "npm version mismatch"],
    ["archive-name", "archive name mismatch"],
    ["checksum", "archive checksum mismatch"],
  ])
    it(`rejects real staged artifact corruption: ${mutation}`, () => {
      const python = process.platform === "win32" ? "python" : "python3";
      const bad = run(python, [
        "npm/test/staged-mutation.py",
        join(dist, `${hostTarget()}.json`),
        mutation,
        join(work, `mutation-${mutation}`),
      ]).trim();
      assert.throws(
        () =>
          run(python, ["scripts/build-staged-release.py", "--verify", bad], {
            stdio: ["ignore", "pipe", "pipe"],
          }),
        (error) => {
          assert.equal(error.status, 1);
          assert.ok(error.stderr.includes(reason), error.stderr);
          assert.match(error.stderr, /next:.*regenerate/);
          return true;
        },
      );
    });

  it("rejects traversal targets before creating directories or invoking the packager", () => {
    const out = join(work, "unsafe-target");
    assert.throws(
      () =>
        run(
          process.platform === "win32" ? "python" : "python3",
          ["scripts/build-staged-release.py", "--target", "../../escape", "--out", out],
          { stdio: ["ignore", "pipe", "pipe"] },
        ),
      /unsupported target/,
    );
    assert.equal(existsSync(out), false);
    assert.equal(existsSync(join(work, "escape")), false);
  });

  it("rejects an unsafe Cargo version before creating output directories", () => {
    const badRepo = join(work, "bad-version-repo");
    mkdirSync(join(badRepo, "scripts"), { recursive: true });
    writeFileSync(
      join(badRepo, "scripts/npm-build.mjs"),
      readFileSync(join(REPO_ROOT, "scripts/npm-build.mjs")),
    );
    writeFileSync(
      join(badRepo, "Cargo.toml"),
      '[package]\nname="onepipeline"\nversion="../../escape"\n',
    );
    const out = join(badRepo, "out");
    assert.throws(
      () =>
        run(
          process.platform === "win32" ? "python" : "python3",
          [
            join(REPO_ROOT, "scripts/build-staged-release.py"),
            "--target",
            hostTarget(),
            "--out",
            out,
          ],
          { cwd: badRepo, stdio: ["ignore", "pipe", "pipe"] },
        ),
      /invalid staged version/,
    );
    assert.equal(existsSync(out), false);
  });

  it("propagates a real installer runtime failure with a recovery action", () => {
    const out = join(work, "failed-install");
    assert.throws(
      () =>
        run(
          process.platform === "win32" ? "python" : "python3",
          [
            "scripts/build-staged-release.py",
            "--target",
            hostTarget(),
            "--profile",
            "dev",
            "--install",
            "--out",
            out,
            "--python",
            join(work, "missing-python-runtime"),
          ],
          { stdio: ["ignore", "pipe", "pipe"] },
        ),
      (error) => {
        assert.equal(error.status, 1);
        assert.match(error.stderr, /missing-python-runtime/);
        assert.match(error.stderr, /next:.*regenerate/);
        return true;
      },
    );
  });

  it("surfaces an installed-wheel smoke failure from self-consistent corrupt version metadata", () => {
    const python = process.platform === "win32" ? "python" : "python3";
    const bad = run(python, [
      "npm/test/staged-mutation.py",
      join(dist, `${hostTarget()}.json`),
      "installed-wheel-version",
      join(work, "wrong-installed-version"),
    ]).trim();
    assert.throws(
      () =>
        run(
          python,
          [
            "scripts/build-staged-release.py",
            "--verify",
            bad,
            "--install",
            "--out",
            join(work, "wheel-smoke-failure"),
          ],
          { stdio: ["ignore", "pipe", "pipe"] },
        ),
      (error) => {
        assert.equal(error.status, 1);
        assert.match(error.stderr, /staged wheel.*reports.*not 'onepipeline 0.0.0'/);
        assert.match(error.stderr, /ACTION:.*reinstall pinned/);
        assert.match(error.stderr, /next:.*regenerate/);
        return true;
      },
    );
  });

  it("surfaces an installed-npm smoke failure when the selected real runtime cannot run the launcher", () => {
    const python = process.platform === "win32" ? "python" : "python3";
    const incompatible = run(python, ["-c", "import sys; print(sys.executable)"]).trim();
    assert.throws(
      () =>
        run(
          python,
          [
            "scripts/build-staged-release.py",
            "--verify",
            join(dist, `${hostTarget()}.json`),
            "--install",
            "--out",
            join(work, "npm-smoke-failure"),
            "--node",
            incompatible,
          ],
          { stdio: ["ignore", "pipe", "pipe"] },
        ),
      (error) => {
        assert.equal(error.status, 1);
        assert.match(error.stderr, /staged npm.*--version.*did not run/);
        assert.match(error.stderr, /ACTION:.*check the platform package/);
        assert.match(error.stderr, /next:.*regenerate/);
        return true;
      },
    );
  });

  it("puts a working `onepipeline` on PATH", () => {
    const project = mkdtempSync(join(work, "app-"));
    installInto(project, [launcherTgz, platformTgz]);

    const reported = launch(project, ["--version"]);
    assert.equal(reported.code, 0, reported.stderr);
    assert.match(reported.stdout, new RegExp(version.replace(/\./g, "\\.")));
  });

  it("propagates the binary's exit code rather than its own", () => {
    const project = mkdtempSync(join(work, "app-"));
    installInto(project, [launcherTgz, platformTgz]);

    // Reading a run nobody recorded exits 2, the contract's "refused". A
    // launcher that collapsed every failure to 1 would hide the contract's own
    // codes from a caller.
    const refused = launch(project, ["next", "run-1"]);
    assert.equal(refused.code, 2, refused.stderr);
    assert.match(refused.stderr, /no such run 'run-1'/);
  });

  it("says what to do when the platform package is missing", () => {
    const project = mkdtempSync(join(work, "app-"));
    installInto(project, [launcherTgz]);

    const failed = launch(project, ["--version"]);
    assert.equal(failed.code, 1);
    assert.match(failed.stderr, /platform package onepipeline-cli-/);
    assert.match(failed.stderr, /Reinstall with optional/);
    assert.match(failed.stderr, /pip install onepipeline-cli/);
  });

  it("refuses Intel macOS by name rather than looking for a package no release ships", () => {
    const project = mkdtempSync(join(work, "app-"));
    installInto(project, [launcherTgz]);
    const manifest = JSON.parse(
      readFileSync(join(project, "node_modules", "onepipeline-cli", "package.json"), "utf8"),
    );
    assert.ok(
      !Object.keys(manifest.optionalDependencies).some((name) => name.includes("darwin-x64")),
      "the launcher must not pin an Intel macOS package",
    );
    // This host is not an Intel Mac, so the installed launcher runs under the
    // identity Node reports there; everything after that is the real launcher.
    const shim = join(project, "intel-mac.cjs");
    writeFileSync(
      shim,
      'Object.defineProperty(process, "platform", { value: "darwin" });\n' +
        'Object.defineProperty(process, "arch", { value: "x64" });\n',
    );
    const launcher = join(project, "node_modules", "onepipeline-cli", "bin", "onepipeline.js");
    const refused = spawnSync(process.execPath, ["--require", shim, launcher, "--version"], {
      cwd: project,
      encoding: "utf8",
    });
    assert.equal(refused.status, 1, refused.stderr);
    assert.equal(refused.stdout, "");
    assert.match(refused.stderr, /Intel macOS \(darwin-x64\) is no longer supported/);
    assert.match(refused.stderr, /cargo install onepipeline --locked/);
    assert.doesNotMatch(refused.stderr, /platform package|unsupported platform/);
  });
});

it("refuses to stage an Intel macOS target, which no release builds any more", () => {
  const work = mkdtempSync(
    join(process.env.ONEPIPELINE_NODE_SCRATCH_DIR ?? tmpdir(), "intel-mac-"),
  );
  const out = join(work, "stage");
  try {
    const refused = spawnSync(
      process.platform === "win32" ? "python" : "python3",
      ["scripts/build-staged-release.py", "--target", "x86_64-apple-darwin", "--out", out],
      { cwd: REPO_ROOT, encoding: "utf8" },
    );
    assert.equal(refused.status, 1, refused.stderr);
    assert.match(refused.stderr, /unsupported target; choose one of: .*aarch64-apple-darwin/);
    assert.doesNotMatch(refused.stderr, /choose one of: .*x86_64-apple-darwin/);
    assert.equal(existsSync(out), false);
  } finally {
    rmSync(work, { recursive: true, force: true });
  }
});

it("refuses a mismatched release tag before building artifacts", () => {
  const work = mkdtempSync(
    join(process.env.ONEPIPELINE_NODE_SCRATCH_DIR ?? tmpdir(), "tag-mismatch-"),
  );
  const out = join(work, "stage");
  try {
    assert.throws(
      () =>
        run(
          process.platform === "win32" ? "python" : "python3",
          [
            "scripts/build-staged-release.py",
            "--target",
            hostTarget(),
            "--out",
            out,
            "--profile",
            "dev",
            "--tag",
            "v0.0.0",
          ],
          { stdio: ["ignore", "pipe", "pipe"] },
        ),
      (error) => {
        assert.equal(error.status, 1);
        assert.match(error.stderr, /release tag differs from Cargo.toml version/);
        assert.match(error.stderr, /next:.*regenerate/);
        return true;
      },
    );
    assert.equal(existsSync(out), false);
  } finally {
    rmSync(work, { recursive: true, force: true });
  }
});
