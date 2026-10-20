// The Linux release, end to end on this host: `just release-linux` builds,
// packages, test-installs, publishes and verifies against local registries, and
// `just release-publish` checks every platform is out and publishes the rest.
//
// Nothing in the release is doubled. The recipes run the real release/release.py,
// which runs the real manylinux build (Docker), `uv`, `pip`, `npm` and `cargo`;
// the registries and GitHub they talk to are local servers
// (`support/release-doubles.mjs`, `npm/test/registry-support.mjs`) that the
// journeys can inspect and tell to fail. arm64 Linux, macOS and Windows
// artifacts, which this host cannot build, are stood in for by files with their
// planned names, so the availability check has every platform to find or miss.
//
// Outside the deterministic gate: the build needs Docker, the manylinux image
// and a Rust toolchain from the network, and minutes of release compile. Run it
// with `just release-journeys`; this project is a network tier, so no `check`
// reaches it.

import { spawn, execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
import {
  chmodSync,
  existsSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { after, before, describe, it } from "node:test";
import assert from "node:assert/strict";
import {
  CratesDouble,
  GitHubDouble,
  PyPIDouble,
} from "../../release/tests/support/release-doubles.mjs";
import { Registry } from "../../npm/test/registry-support.mjs";

const ROOT = process.cwd();
const VERSION = readFileSync(join(ROOT, "Cargo.toml"), "utf8").match(
  /^\[package\][\s\S]*?^version = "([^"]+)"/m,
)[1];
const TAG = `v${VERSION}`;
const TARGET = "x86_64-unknown-linux-gnu";
const SCRATCH = process.env.ONEPIPELINE_NODE_SCRATCH_DIR ?? tmpdir();
// Inside the checkout, because the manylinux container mounts only the checkout.
const OUT = join(ROOT, "target", "release-journeys", `run-${process.pid}`);
// Shared across runs so the crate's own builds are incremental after the first.
const CARGO_TARGET = join(ROOT, "target", "release-journeys", "cargo");

function run(command, args, { env = {}, cwd = ROOT } = {}) {
  return new Promise((resolve, reject) => {
    const child = spawn(command, args, { cwd, env: { ...process.env, ...env } });
    let stdout = "";
    let stderr = "";
    child.stdout.on("data", (chunk) => {
      stdout += chunk;
    });
    child.stderr.on("data", (chunk) => {
      stderr += chunk;
    });
    child.on("error", reject);
    child.on("close", (code) => resolve({ code, stdout, stderr }));
  });
}

function said(result) {
  return `exit ${result.code}\n--- stdout\n${result.stdout.slice(-6000)}\n--- stderr\n${result.stderr.slice(-6000)}`;
}

function sha(buffer) {
  return createHash("sha256").update(buffer).digest("hex");
}

/// The executable each Linux artifact carries, read out of the file itself.
function payloads(manifest) {
  const file = (kind) => manifest.artifacts.find((a) => a.kind === kind).path;
  const into = mkdtempSync(join(SCRATCH, "payloads-"));
  execFileSync("python3", ["-m", "zipfile", "-e", file("wheel"), join(into, "wheel")]);
  mkdirSync(join(into, "archive"));
  execFileSync("tar", ["-xzf", file("archive"), "-C", join(into, "archive")]);
  mkdirSync(join(into, "npm"));
  execFileSync("tar", ["-xzf", file("npm"), "-C", join(into, "npm")]);
  return {
    into,
    wheel: join(into, "wheel", `onepipeline_cli-${VERSION}.data`, "scripts", "onepipeline"),
    archive: join(into, "archive", "onepipeline"),
    npm: join(into, "npm", "package", "bin", "onepipeline"),
  };
}

function glibcFloor(binary) {
  const symbols = execFileSync("objdump", ["-T", binary], { encoding: "utf8" });
  return [...symbols.matchAll(/GLIBC_(\d+)\.(\d+)/g)]
    .map((m) => [Number(m[1]), Number(m[2])])
    .sort((a, b) => a[0] - b[0] || a[1] - b[1])
    .at(-1)
    .join(".");
}

/// Every double, fresh, and the environment a release job hands its recipe.
async function services() {
  const github = await new GitHubDouble().start();
  github.addRelease(TAG);
  const pypi = await new PyPIDouble().start();
  const crates = await new CratesDouble().start();
  const npm = new Registry();
  await npm.start();
  // The npm double refuses everything or nothing; a publish alone is refused by
  // answering the next PUT before it reaches it.
  const served = npm.server.listeners("request")[0];
  npm.server.removeAllListeners("request");
  npm.refuseNextPublish = false;
  npm.server.on("request", (req, res) => {
    if (req.method === "PUT" && npm.refuseNextPublish) {
      npm.refuseNextPublish = false;
      req.resume();
      // A 4xx, because npm retries a 5xx itself and would succeed the second time.
      res.writeHead(403, { "content-type": "application/json" });
      res.end('{"error":"You do not have permission to publish this package"}');
      return;
    }
    served(req, res);
  });
  const home = mkdtempSync(join(SCRATCH, "npm-home-"));
  const env = {
    ...npm.npmEnv(home),
    ...github.env(),
    ...pypi.env(),
    ...crates.env(),
    RELEASE_TAG: TAG,
    PYPI_PUBLISH: "true",
    NPM_PUBLISH: "true",
    NODE_AUTH_TOKEN: "npm-not-a-real-token",
    ONEPIPELINE_RELEASE_VERIFY_BUDGET: "6",
    PUBLISH_NPM_AWAIT_BUDGET: "20",
    PUBLISH_NPM_AWAIT_INTERVAL: "1",
    PIP_DISABLE_PIP_VERSION_CHECK: "1",
    UV_CACHE_DIR: join(home, "uv-cache"),
  };
  const stop = async () => {
    await Promise.all([github.stop(), pypi.stop(), crates.stop(), npm.stop()]);
    rmSync(home, { recursive: true, force: true });
  };
  return { github, pypi, crates, npm, env, home, stop };
}

/// How many times each file name was written to a double.
function writesOf(s) {
  const counts = {};
  const count = (name) => {
    counts[name] = (counts[name] ?? 0) + 1;
  };
  for (const r of s.github.writes()) {
    if (r.method === "POST" && r.path.startsWith("/uploads/"))
      count(new URLSearchParams(r.query).get("name"));
  }
  for (const r of s.pypi.writes()) if (r.path === "/legacy/") count("pypi-upload");
  for (const e of s.npm.timeline) if (e.kind === "accepted") count(e.identity);
  for (const r of s.crates.writes()) if (r.path === "/api/v1/crates/new") count("crate-upload");
  return counts;
}

/// Publish a package directory to the npm double the way a build job would.
async function npmPublish(s, dir) {
  const result = await run("npm", ["publish", dir, "--access", "public"], { env: s.env });
  assert.equal(result.code, 0, said(result));
}

/// An npm platform package for `target` carrying `binary`, as npm-build.mjs makes it.
async function platformPackage(target, binary, version = VERSION) {
  const out = mkdtempSync(join(SCRATCH, "npm-platform-"));
  const result = await run("node", [
    "scripts/npm-build.mjs",
    "platform",
    "--target",
    target,
    "--binary",
    binary,
    "--version",
    version,
    "--out",
    out,
  ]);
  assert.equal(result.code, 0, said(result));
  return result.stdout.trim();
}

/// What the arm64 Linux, macOS and Windows build jobs would have published, by planned name.
async function otherPlatformsPublished(s, linux) {
  const binary = linux.artifacts.find((a) => a.kind === "archive").path;
  for (const platform of ["linux-arm64", "macos", "windows"]) {
    const plan = JSON.parse(
      execFileSync("just", [`release-${platform}`, "plan"], { encoding: "utf8" }),
    );
    for (const artifact of plan.artifacts) {
      if (artifact.destination === "github-release") {
        s.github.release(TAG).assets.push({
          id: s.github.nextId++,
          name: artifact.file,
          content: Buffer.from(artifact.file),
        });
      } else if (artifact.destination === "pypi") {
        s.pypi.serve(artifact.file, Buffer.from(artifact.file));
      } else {
        await npmPublish(s, await platformPackage(artifact.target, binary));
      }
    }
  }
}

let built;
let linux;

before(async () => {
  rmSync(OUT, { recursive: true, force: true });
  // One build for every journey below: the dry run is the build phase plus a
  // publication that writes nothing, so its files are what the live journeys
  // publish.
  const s = await services();
  try {
    built = await run("just", ["release-linux", "dry-run"], {
      env: { ...s.env, ONEPIPELINE_RELEASE_OUT: OUT },
    });
    assert.equal(built.code, 0, said(built));
    assert.deepEqual(writesOf(s), {}, "the dry run published nothing");
    assert.deepEqual(s.github.writes(), []);
  } finally {
    await s.stop();
  }
  linux = JSON.parse(readFileSync(join(OUT, "linux", "build.json"), "utf8"));
});

after(() => {
  rmSync(OUT, { recursive: true, force: true });
});

describe("the Linux recipe, publishing in its dry-run mode", () => {
  it("builds each Linux target once and packages archive, wheel and npm package from that one binary", () => {
    assert.deepEqual(
      linux.artifacts.map((a) => a.target),
      [TARGET, TARGET, TARGET, TARGET],
    );
    assert.match(
      built.stdout,
      new RegExp(
        `^release: built ${TARGET} once, packaged its wheel, archive and npm package from that binary, and test-installed all three`,
        "m",
      ),
      "one build per Linux target",
    );
    const p = payloads(linux);
    try {
      const bytes = readFileSync(p.wheel);
      assert.ok(bytes.length > 1_000_000, "the wheel carries the real binary");
      assert.equal(
        sha(readFileSync(p.archive)),
        sha(bytes),
        "the archive carries the wheel's binary",
      );
      assert.equal(
        sha(readFileSync(p.npm)),
        sha(bytes),
        "the npm package carries the wheel's binary",
      );
      for (const binary of [p.wheel, p.archive, p.npm]) {
        assert.equal(
          glibcFloor(binary),
          "2.17",
          `${binary} needs no glibc above the wheel's floor`,
        );
      }
    } finally {
      rmSync(p.into, { recursive: true, force: true });
    }
  });

  it("names every file as the plan does, which is v0.74.1's names at this version", () => {
    const plan = JSON.parse(execFileSync("just", ["release-linux", "plan"], { encoding: "utf8" }));
    assert.deepEqual(
      linux.artifacts.map((a) => a.file),
      plan.artifacts.map((a) => a.file),
    );
    const published = JSON.parse(
      readFileSync(join(ROOT, ".github/fixtures/release-v0.74.1-published.json"), "utf8"),
    );
    const at = (name) => name.replaceAll(published.version, VERSION);
    for (const artifact of linux.artifacts) {
      const was =
        artifact.kind === "npm"
          ? `onepipeline-cli-linux-x64-${VERSION}.tgz`
          : [...published.release_assets, ...published.pypi]
              .map(at)
              .find((name) => name === artifact.file);
      assert.equal(artifact.file, was, `${artifact.file} is a file v0.74.1 published`);
    }
    const wheel = linux.artifacts.find((a) => a.kind === "wheel").path;
    const tags = execFileSync(
      "python3",
      [
        "-c",
        "import zipfile,sys; w=zipfile.ZipFile(sys.argv[1]); print(''.join(l for l in w.read(next(n for n in w.namelist() if n.endswith('.dist-info/WHEEL'))).decode().splitlines(True) if l.startswith('Tag: ')))",
        wheel,
      ],
      { encoding: "utf8" },
    );
    assert.deepEqual(
      tags.trim().split("\n").sort(),
      ["Tag: py3-none-manylinux_2_17_x86_64", "Tag: py3-none-manylinux2014_x86_64"].sort(),
    );
  });

  it("test-installs those files and runs what each installed", () => {
    assert.match(built.stdout, /and test-installed all three/);
    // What each install put in place, run again here: the build ran its smoke
    // over these, and they are the installs of the very files it publishes.
    const installed = join(OUT, "linux", "install", TARGET);
    for (const binary of [
      join(installed, "python", "bin", "onepipeline"),
      join(installed, "npm", "node_modules", "onepipeline-cli-linux-x64", "bin", "onepipeline"),
      join(installed, "archive", "onepipeline"),
    ]) {
      assert.equal(
        execFileSync(binary, ["--version"], { encoding: "utf8" }).trim(),
        `onepipeline ${VERSION}`,
        binary,
      );
    }
    for (const artifact of linux.artifacts) {
      assert.match(
        built.stdout,
        new RegExp(
          `would publish ${artifact.file.replaceAll(".", "\\.")} to ${artifact.destination}`,
        ),
      );
    }
  });

  it("with a test-install that fails, exits non-zero without invoking any publish command", async () => {
    const s = await services();
    const out = join(ROOT, "target", "release-journeys", `failing-${process.pid}`);
    const constraints = join(s.home, "constraints.txt");
    // A pip that may install no onepipeline-cli but 0.0.0: the wheel the build
    // made cannot be installed, which is a test-install failure like any other.
    writeFileSync(constraints, "onepipeline-cli==0.0.0\n");
    try {
      for (const phase of ["dry-run", "build"]) {
        const failed = await run("just", ["release-linux", phase], {
          env: { ...s.env, ONEPIPELINE_RELEASE_OUT: out, PIP_CONSTRAINT: constraints },
        });
        assert.equal(failed.code, 1, said(failed));
        assert.match(failed.stderr, /test-install failed, so nothing is published/, said(failed));
        assert.doesNotMatch(failed.stdout, /would publish|uploaded/);
        assert.ok(!existsSync(join(out, "linux", "build.json")), "no build is left to publish");
      }
      // And the publish step the job would run next has nothing to publish.
      const publish = await run("just", ["release-linux", "publish"], {
        env: { ...s.env, ONEPIPELINE_RELEASE_OUT: out },
      });
      assert.equal(publish.code, 1, said(publish));
      assert.match(publish.stderr, /no build in/);
      assert.deepEqual(writesOf(s), {}, "no registry was written to");
      assert.deepEqual(s.github.writes(), []);
    } finally {
      await s.stop();
      rmSync(out, { recursive: true, force: true });
    }
  });
});

describe("publishing and verifying the Linux artifacts", () => {
  let s;
  before(async () => {
    s = await services();
  });
  after(() => s.stop());
  const env = () => ({ ...s.env, ONEPIPELINE_RELEASE_OUT: OUT });

  it("a failed upload exits non-zero, and the re-run publishes only what is missing", async () => {
    s.github.refuseNext("POST", /^\/uploads\//, 502, "Bad Gateway");
    const failed = await run("just", ["release-linux", "publish"], { env: env() });
    assert.equal(failed.code, 1, said(failed));
    assert.match(failed.stderr, /GitHub answered 502 uploading onepipeline-v/);
    const before = writesOf(s);
    const rerun = await run("just", ["release-linux", "publish"], { env: env() });
    assert.equal(rerun.code, 0, said(rerun));
    const after = writesOf(s);
    // The refused upload is the one write the re-run repeats.
    const archive = `onepipeline-v${VERSION}-${TARGET}.tar.gz`;
    const checksum = `onepipeline-v${VERSION}-${TARGET}.sha256`;
    assert.deepEqual(before, { [archive]: 1 }, "the first run stopped at the refused upload");
    assert.deepEqual(after, {
      [archive]: 2,
      [checksum]: 1,
      "pypi-upload": 1,
      [`onepipeline-cli-linux-x64@${VERSION}`]: 1,
    });
    // A third run finds everything already served and writes nothing.
    const again = await run("just", ["release-linux", "publish"], { env: env() });
    assert.equal(again.code, 0, said(again));
    assert.deepEqual(writesOf(s), after, "a re-run of a complete publication writes nothing");
  });

  it("publishes exactly the files its build produced, at the release's version", () => {
    const file = (kind) => readFileSync(linux.artifacts.find((a) => a.kind === kind).path);
    const assets = Object.fromEntries(s.github.release(TAG).assets.map((a) => [a.name, a.content]));
    assert.deepEqual(Object.keys(assets).sort(), [
      `onepipeline-v${VERSION}-${TARGET}.sha256`,
      `onepipeline-v${VERSION}-${TARGET}.tar.gz`,
    ]);
    assert.equal(sha(assets[`onepipeline-v${VERSION}-${TARGET}.tar.gz`]), sha(file("archive")));
    assert.equal(sha(assets[`onepipeline-v${VERSION}-${TARGET}.sha256`]), sha(file("checksum")));
    const wheel = linux.artifacts.find((a) => a.kind === "wheel");
    assert.deepEqual([...s.pypi.files.keys()], [wheel.file]);
    assert.equal(sha(s.pypi.files.get(wheel.file).content), sha(file("wheel")));
    assert.equal(s.pypi.files.get(wheel.file).version, VERSION);
    assert.deepEqual(s.npm.visible("onepipeline-cli-linux-x64"), [VERSION]);
    const held = s.npm.packages.get("onepipeline-cli-linux-x64").versions.get(VERSION);
    assert.equal(sha(held.tarball), sha(file("npm")));
    assert.equal(s.npm.packages.size, 1, "the launcher is publish's, not this job's");
  });

  it("installs the version back from every registry and runs it", async () => {
    const verified = await run("just", ["release-linux", "verify"], { env: env() });
    assert.equal(verified.code, 0, said(verified));
    assert.match(
      verified.stdout,
      new RegExp(
        `^release: every linux artifact of ${VERSION.replaceAll(".", "\\.")} installs from its registry and runs$`,
        "m",
      ),
    );
    // Each install read the file back from its registry.
    assert.ok(
      s.github.requests.some((r) => r.path.startsWith("/download/")),
      "the archive was downloaded",
    );
    assert.ok(
      s.pypi.requests.some((r) => r.path.startsWith("/files/")),
      "pip fetched the wheel",
    );
  });
});

describe("a refused PyPI upload", () => {
  it("fails the publish step, and the re-run uploads the wheel without repeating what landed", async () => {
    const s = await services();
    try {
      const env = { ...s.env, ONEPIPELINE_RELEASE_OUT: OUT };
      // A 4xx, because uv retries a 5xx itself.
      s.pypi.refuseNext(
        "POST",
        /^\/legacy\/$/,
        403,
        "Invalid or non-existent authentication information.",
      );
      const refused = await run("just", ["release-linux", "publish"], { env });
      assert.equal(refused.code, 1, said(refused));
      assert.equal(s.pypi.files.size, 0);
      const landed = writesOf(s);
      assert.equal(landed[`onepipeline-v${VERSION}-${TARGET}.tar.gz`], 1);
      const rerun = await run("just", ["release-linux", "publish"], { env });
      assert.equal(rerun.code, 0, said(rerun));
      const after = writesOf(s);
      assert.equal(
        after[`onepipeline-v${VERSION}-${TARGET}.tar.gz`],
        1,
        "the archive was not uploaded again",
      );
      assert.equal(after[`onepipeline-v${VERSION}-${TARGET}.sha256`], 1);
      assert.equal(s.pypi.files.size, 1, "the wheel landed on the re-run");
      assert.equal(after[`onepipeline-cli-linux-x64@${VERSION}`], 1);
    } finally {
      await s.stop();
    }
  });
});

describe("registry verification refuses what a user could not run", () => {
  /// A double serving the Linux artifacts, with one of them replaced.
  async function serving({ wheel = true, npmBinary = null, npmVersion = VERSION } = {}) {
    const s = await services();
    for (const artifact of linux.artifacts) {
      if (artifact.destination === "github-release") {
        s.github.release(TAG).assets.push({
          id: s.github.nextId++,
          name: artifact.file,
          content: readFileSync(artifact.path),
        });
      } else if (artifact.destination === "pypi" && wheel) {
        s.pypi.serve(artifact.file, readFileSync(artifact.path));
      }
    }
    const p = payloads(linux);
    try {
      let binary = p.npm;
      if (npmBinary !== null) {
        binary = join(p.into, "replacement");
        writeFileSync(binary, npmBinary);
        chmodSync(binary, 0o755);
      }
      await npmPublish(s, await platformPackage(TARGET, binary, npmVersion));
    } finally {
      rmSync(p.into, { recursive: true, force: true });
    }
    return s;
  }

  async function verify(s) {
    return run("just", ["release-linux", "verify"], {
      env: { ...s.env, ONEPIPELINE_RELEASE_OUT: OUT },
    });
  }

  it("fails when a registry serves another version's binary", async () => {
    const s = await serving({ npmBinary: "#!/bin/sh\necho 'onepipeline 9.9.9'\n" });
    try {
      const result = await verify(s);
      assert.equal(result.code, 1, said(result));
      assert.match(result.stderr, /npm onepipeline-cli-linux-x64.*reports 'onepipeline 9\.9\.9'/);
    } finally {
      await s.stop();
    }
  });

  it("fails when a registry serves a package that cannot run", async () => {
    const s = await serving({ npmBinary: "this is not an executable\n" });
    try {
      const result = await verify(s);
      assert.equal(result.code, 1, said(result));
      assert.match(result.stderr, /npm onepipeline-cli-linux-x64.*'--version' did not run/);
    } finally {
      await s.stop();
    }
  });

  it("fails when the Release's archive carries no executable, rather than running another one", async () => {
    const s = await serving();
    try {
      const dir = mkdtempSync(join(SCRATCH, "empty-archive-"));
      writeFileSync(join(dir, "README.md"), "no binary here\n");
      const archive = s.github.release(TAG).assets.find((a) => a.name.endsWith(".tar.gz"));
      archive.content = execFileSync("tar", ["-czf", "-", "-C", dir, "README.md"]);
      s.github.release(TAG).assets.find((a) => a.name.endsWith(".sha256")).content = Buffer.from(
        `${sha(archive.content)}  ${archive.name}\n`,
      );
      rmSync(dir, { recursive: true, force: true });
      const result = await verify(s);
      assert.equal(result.code, 1, said(result));
      assert.match(result.stderr, /\.tar\.gz is not \[.*\], each a regular file/);
    } finally {
      await s.stop();
    }
  });

  it("fails when the Release's archive does not match its checksum", async () => {
    const s = await serving();
    try {
      const archive = s.github.release(TAG).assets.find((a) => a.name.endsWith(".tar.gz"));
      archive.content = Buffer.concat([archive.content, Buffer.from("tampered")]);
      const result = await verify(s);
      assert.equal(result.code, 1, said(result));
      assert.match(result.stderr, /on the Release does not match its onepipeline-v\S+\.sha256/);
    } finally {
      await s.stop();
    }
  });

  it("fails when a registry serves no package for this platform", async () => {
    const s = await serving({ wheel: false });
    try {
      // Another platform's wheel, at this version, is not this platform's.
      s.pypi.serve(
        `onepipeline_cli-${VERSION}-py3-none-win_amd64.whl`,
        Buffer.from("not this platform"),
      );
      const result = await verify(s);
      assert.equal(result.code, 1, said(result));
      assert.match(result.stderr, /PyPI onepipeline-cli .* for x86_64-unknown-linux-gnu/);
    } finally {
      await s.stop();
    }
  });
});

describe("publication refuses a build that is not the plan, untouched", () => {
  /// A copy of the build, changed by `tamper`, published against fresh doubles.
  async function publishTampered(tamper) {
    const s = await services();
    const out = join(ROOT, "target", "release-journeys", `tampered-${process.pid}`);
    rmSync(out, { recursive: true, force: true });
    try {
      mkdirSync(out, { recursive: true });
      execFileSync("cp", ["-a", join(OUT, "linux"), join(out, "linux")]);
      const manifest = join(out, "linux", "build.json");
      const build = JSON.parse(
        readFileSync(manifest, "utf8").replaceAll(join(OUT, "linux"), join(out, "linux")),
      );
      tamper(build, join(out, "linux"));
      writeFileSync(manifest, JSON.stringify(build));
      const result = await run("just", ["release-linux", "publish"], {
        env: { ...s.env, ONEPIPELINE_RELEASE_OUT: out },
      });
      assert.deepEqual(writesOf(s), {}, "nothing was published");
      return result;
    } finally {
      await s.stop();
      rmSync(out, { recursive: true, force: true });
    }
  }
  const of = (build, kind) => build.artifacts.find((a) => a.kind === kind);

  it("refuses a build of another version", async () => {
    const result = await publishTampered((build) => {
      build.version = "0.0.1";
      build.tag = "v0.0.1";
    });
    assert.equal(result.code, 1, said(result));
    assert.match(result.stderr, /is linux v0\.0\.1, not linux v/);
  });

  it("refuses a file the test-install never ran", async () => {
    const result = await publishTampered((build) => {
      writeFileSync(of(build, "archive").path, "a different archive");
    });
    assert.equal(result.code, 1, said(result));
    assert.match(result.stderr, /changed after it was built and test-installed/);
  });

  it("refuses a file the plan does not name", async () => {
    const result = await publishTampered((build) => {
      of(build, "wheel").file = `onepipeline_cli-${VERSION}-py3-none-manylinux_2_39_x86_64.whl`;
    });
    assert.equal(result.code, 1, said(result));
    assert.match(result.stderr, /are not the plan's/);
  });

  it("refuses a file whose name on disk is not the one the plan gives it", async () => {
    const result = await publishTampered((build, dir) => {
      const archive = of(build, "archive");
      const renamed = join(dir, "archives", "onepipeline-latest.tar.gz");
      execFileSync("cp", [archive.path, renamed]);
      archive.path = renamed;
    });
    assert.equal(result.code, 1, said(result));
    assert.match(result.stderr, /onepipeline-latest\.tar\.gz is not the planned onepipeline-v/);
  });

  it("refuses a Linux binary above the planned glibc floor", async () => {
    const result = await publishTampered((build, dir) => {
      // This host's own `true` links the host's glibc, far above 2.17.
      const wheel = of(build, "wheel");
      const into = mkdtempSync(join(SCRATCH, "wheel-"));
      execFileSync("python3", ["-m", "zipfile", "-e", wheel.path, into]);
      execFileSync("cp", [
        "/usr/bin/true",
        join(into, `onepipeline_cli-${VERSION}.data`, "scripts", "onepipeline"),
      ]);
      const rebuilt = join(dir, "rebuilt.whl");
      execFileSync("python3", [
        "-c",
        "import shutil,sys; shutil.make_archive(sys.argv[1], 'zip', sys.argv[2])",
        rebuilt,
        into,
      ]);
      execFileSync("mv", [`${rebuilt}.zip`, wheel.path]);
      wheel.sha256 = sha(readFileSync(wheel.path));
      rmSync(into, { recursive: true, force: true });
    });
    assert.equal(result.code, 1, said(result));
    assert.match(result.stderr, /requires GLIBC_2\.\d+, above the planned floor 2\.17/);
  });
});

describe("a registry switched off", () => {
  it("is neither published to nor verified, and the Release assets still are", async () => {
    const s = await services();
    try {
      const env = {
        ...s.env,
        ONEPIPELINE_RELEASE_OUT: OUT,
        PYPI_PUBLISH: "false",
        NPM_PUBLISH: "",
      };
      const published = await run("just", ["release-linux", "publish"], { env });
      assert.equal(published.code, 0, said(published));
      assert.match(published.stdout, /skipped PyPI \(PYPI_PUBLISH off\), npm \(NPM_PUBLISH off\)/);
      assert.deepEqual(Object.keys(writesOf(s)).sort(), [
        `onepipeline-v${VERSION}-${TARGET}.sha256`,
        `onepipeline-v${VERSION}-${TARGET}.tar.gz`,
      ]);
      assert.equal(s.pypi.files.size, 0);
      assert.equal(s.npm.packages.size, 0);
      const verified = await run("just", ["release-linux", "verify"], { env });
      assert.equal(verified.code, 0, said(verified));
      assert.match(
        verified.stdout,
        /installs from its registry and runs; skipped PyPI \(PYPI_PUBLISH off\), npm \(NPM_PUBLISH off\)/,
      );
      assert.ok(
        s.github.requests.some((r) => r.path.startsWith("/download/")),
        "the archive was downloaded",
      );
      assert.deepEqual(s.pypi.requests, [], "PyPI was not asked");
      const typo = await run("just", ["release-linux", "publish"], {
        env: { ...env, NPM_PUBLISH: "yes" },
      });
      assert.equal(typo.code, 1, said(typo));
      assert.match(typo.stderr, /NPM_PUBLISH is 'yes', which neither turns publishing on nor off/);
    } finally {
      await s.stop();
    }
  });
});

describe("publish's own verification", () => {
  it("fails when the launcher it published does not run, and records that it shipped", async () => {
    const s = await services();
    try {
      // Every platform out, but npm's linux-x64 package carries another version.
      for (const artifact of linux.artifacts) {
        if (artifact.destination === "github-release") {
          s.github.release(TAG).assets.push({
            id: s.github.nextId++,
            name: artifact.file,
            content: readFileSync(artifact.path),
          });
        } else if (artifact.destination === "pypi") {
          s.pypi.serve(artifact.file, readFileSync(artifact.path));
        }
      }
      const fake = join(s.home, "onepipeline");
      writeFileSync(fake, "#!/bin/sh\necho 'onepipeline 9.9.9'\n", { mode: 0o755 });
      await npmPublish(s, await platformPackage(TARGET, fake));
      await otherPlatformsPublished(s, linux);
      const out = mkdtempSync(join(SCRATCH, "launcher-verify-"));
      const env = { ...s.env, CARGO_REGISTRY_TOKEN: "", ONEPIPELINE_RELEASE_OUT: out };
      const result = await run("just", ["release-publish", "publish"], { env });
      assert.equal(result.code, 1, said(result));
      assert.match(result.stderr, /npm onepipeline-cli .*reports 'onepipeline 9\.9\.9'/);
      assert.deepEqual(s.npm.visible("onepipeline-cli"), [VERSION], "the launcher was published");
      const results = JSON.parse(readFileSync(join(out, "common", "publish-results.json"), "utf8"));
      assert.deepEqual(results["npm-launcher"], { published: "success", verified: "failure" });
      const jobs = {};
      for (const key of ["LINUX", "LINUX_ARM64", "MACOS", "WINDOWS"])
        jobs[`${key}_RESULT`] = "success";
      const recorded = await run("just", ["release-outcome"], { env: { ...env, ...jobs } });
      assert.equal(recorded.code, 0, said(recorded));
      const record = JSON.parse(
        s.github
          .release(TAG)
          .assets.find((a) => a.name === "release-outcome.json")
          .content.toString(),
      );
      const npm = record.targets.find((t) => t.id === "npm:onepipeline-cli");
      assert.deepEqual(
        [npm.outcome, npm.published, npm.verified],
        ["shipped-unverified", "success", "failure"],
      );
      rmSync(out, { recursive: true, force: true });
    } finally {
      await s.stop();
    }
  });
});

describe("publish: the platform-independent artifacts", () => {
  let s;
  let checkout;
  const env = (extra = {}) => ({
    ...s.env,
    ONEPIPELINE_RELEASE_OUT: join(checkout, "target", "release-artifacts"),
    CARGO_TARGET_DIR: CARGO_TARGET,
    // Release-profile LTO is what makes a release binary small; the install
    // proves the crate builds and runs, so it is skipped to keep this minutes.
    CARGO_PROFILE_RELEASE_LTO: "false",
    CARGO_PROFILE_RELEASE_CODEGEN_UNITS: "16",
    ...extra,
  });

  before(async () => {
    s = await services();
    // `cargo publish` packages a committed tree, so publish runs from a clean
    // checkout of this commit, as the release job's does.
    checkout = mkdtempSync(join(SCRATCH, "publish-checkout-"));
    execFileSync("git", ["worktree", "add", "--detach", join(checkout, "tree"), "HEAD"], {
      cwd: ROOT,
    });
    checkout = join(checkout, "tree");
    const published = await run("just", ["release-linux", "publish"], {
      env: { ...s.env, ONEPIPELINE_RELEASE_OUT: OUT },
    });
    assert.equal(published.code, 0, said(published));
  });
  after(async () => {
    await s.stop();
    execFileSync("git", ["worktree", "remove", "--force", checkout], { cwd: ROOT });
  });

  it("refuses to publish anything while any platform is missing", async () => {
    const before = writesOf(s);
    const result = await run("just", ["release-publish", "publish"], { cwd: checkout, env: env() });
    assert.equal(result.code, 1, said(result));
    assert.match(
      result.stderr,
      /not every platform is published, so nothing platform-independent is/,
    );
    for (const missing of [
      `onepipeline-v${VERSION}-aarch64-unknown-linux-gnu.tar.gz on the Release`,
      `npm:onepipeline-cli-linux-arm64@${VERSION} on npm`,
      `onepipeline-v${VERSION}-aarch64-apple-darwin.tar.gz on the Release`,
      `onepipeline_cli-${VERSION}-py3-none-win_amd64.whl on PyPI`,
      `npm:onepipeline-cli-darwin-arm64@${VERSION} on npm`,
    ]) {
      assert.ok(result.stderr.includes(missing), `names ${missing}:\n${result.stderr}`);
    }
    assert.ok(!result.stderr.includes(`${TARGET}.tar.gz on the Release`), "Linux is not missing");
    assert.deepEqual(writesOf(s), before, "nothing was published");
    assert.ok(!s.github.writes().some((r) => r.method === "PATCH"), "the notes were not written");
  });

  it("with every platform out, publishes the crate and the launcher, installs both, and a failed upload's re-run publishes only what is missing", async () => {
    await otherPlatformsPublished(s, linux);
    const before = writesOf(s);
    s.npm.refuseNextPublish = true;
    // The registry fails the launcher's publish: the crate is out, the launcher not.
    const failed = await run("just", ["release-publish", "publish"], { cwd: checkout, env: env() });
    assert.equal(failed.code, 1, said(failed));
    const crateOnce = writesOf(s);
    assert.equal(crateOnce["crate-upload"], 1, said(failed));
    assert.equal(crateOnce[`onepipeline-cli@${VERSION}`], undefined);
    const rerun = await run("just", ["release-publish", "publish"], { cwd: checkout, env: env() });
    assert.equal(rerun.code, 0, said(rerun));
    const after = writesOf(s);
    assert.equal(after["crate-upload"], 1, "the re-run does not publish the crate again");
    assert.equal(after[`onepipeline-cli@${VERSION}`], 1, "the re-run publishes the launcher");
    for (const [name, count] of Object.entries(before)) assert.equal(after[name], count, name);
    assert.deepEqual(
      s.crates.crates.get("onepipeline").map((v) => v.line.vers),
      [VERSION],
    );
    assert.deepEqual(s.npm.visible("onepipeline-cli"), [VERSION]);
    assert.ok(
      s.crates.requests.some((r) => /\/download$/.test(r.path)),
      "cargo installed the crate from the registry",
    );
    assert.match(rerun.stdout, /every registry serves .* for every platform/);
    assert.match(s.github.release(TAG).body, /^<!-- linked-engines -->$/m);
    assert.equal(
      (s.github.release(TAG).body.match(/<!-- linked-engines -->/g) ?? []).length,
      1,
      "the re-run replaced the linked-engines section rather than adding a second",
    );
    const results = JSON.parse(
      readFileSync(
        join(checkout, "target", "release-artifacts", "common", "publish-results.json"),
        "utf8",
      ),
    );
    assert.deepEqual(results, {
      crate: { published: "success", verified: "success" },
      "npm-launcher": { published: "success", verified: "success" },
    });
  });

  it("skips the crate, as the release always has, when CARGO_REGISTRY_TOKEN is unset", async () => {
    const before = writesOf(s);
    const result = await run("just", ["release-publish", "publish"], {
      cwd: checkout,
      env: env({ CARGO_REGISTRY_TOKEN: "" }),
    });
    assert.equal(result.code, 0, said(result));
    assert.match(result.stdout, /skipped crates\.io \(CARGO_REGISTRY_TOKEN not set\)/);
    assert.deepEqual(writesOf(s), before);
  });
});

describe("publish: the crate, and a registry that loses an artifact", () => {
  let checkout;
  before(() => {
    checkout = mkdtempSync(join(SCRATCH, "crate-checkout-"));
    execFileSync("git", ["worktree", "add", "--detach", join(checkout, "tree"), "HEAD"], {
      cwd: ROOT,
    });
    checkout = join(checkout, "tree");
  });
  after(() => {
    execFileSync("git", ["worktree", "remove", "--force", checkout], { cwd: ROOT });
  });

  /// Every platform's assets on the Release, with PyPI and npm switched off.
  async function assetsOnly() {
    const s = await services();
    for (const platform of ["linux", "linux-arm64", "macos", "windows"]) {
      const plan = JSON.parse(
        execFileSync("just", [`release-${platform}`, "plan"], { encoding: "utf8" }),
      );
      for (const artifact of plan.artifacts.filter((a) => a.destination === "github-release")) {
        s.github.release(TAG).assets.push({
          id: s.github.nextId++,
          name: artifact.file,
          content: Buffer.from(artifact.file),
        });
      }
    }
    const env = {
      ...s.env,
      PYPI_PUBLISH: "false",
      NPM_PUBLISH: "false",
      ONEPIPELINE_RELEASE_OUT: mkdtempSync(join(SCRATCH, "crate-out-")),
      CARGO_TARGET_DIR: CARGO_TARGET,
      CARGO_PROFILE_RELEASE_LTO: "false",
      CARGO_PROFILE_RELEASE_CODEGEN_UNITS: "16",
    };
    return {
      s,
      env,
      results: () =>
        JSON.parse(
          readFileSync(join(env.ONEPIPELINE_RELEASE_OUT, "common", "publish-results.json"), "utf8"),
        ),
    };
  }

  it("a refused crate upload is recorded as failed, and the re-run publishes it", async () => {
    const { s, env, results } = await assetsOnly();
    try {
      s.crates.refuseNext("PUT", /^\/api\/v1\/crates\/new$/, 403, "this token may not publish");
      const refused = await run("just", ["release-publish", "publish"], { cwd: checkout, env });
      assert.equal(refused.code, 1, said(refused));
      assert.deepEqual(results().crate, { published: "failure", verified: "skipped" });
      assert.equal(s.crates.crates.size, 0);
      const rerun = await run("just", ["release-publish", "publish"], { cwd: checkout, env });
      assert.equal(rerun.code, 0, said(rerun));
      assert.deepEqual(results().crate, { published: "success", verified: "success" });
      assert.deepEqual(
        s.crates.crates.get("onepipeline").map((v) => v.line.vers),
        [VERSION],
      );
    } finally {
      await s.stop();
    }
  });

  it("a re-run whose crate upload fails drops the earlier verification rather than reporting it", async () => {
    const { s, env, results } = await assetsOnly();
    try {
      mkdirSync(join(env.ONEPIPELINE_RELEASE_OUT, "common"), { recursive: true });
      writeFileSync(
        join(env.ONEPIPELINE_RELEASE_OUT, "common", "publish-results.json"),
        JSON.stringify({
          crate: { published: "success", verified: "success" },
          "npm-launcher": { published: "skipped", verified: "skipped" },
        }),
      );
      s.crates.refuseNext("PUT", /^\/api\/v1\/crates\/new$/, 403, "this token may not publish");
      const refused = await run("just", ["release-publish", "publish"], { cwd: checkout, env });
      assert.equal(refused.code, 1, said(refused));
      assert.deepEqual(results().crate, { published: "failure", verified: "skipped" });
      const jobs = {};
      for (const key of ["LINUX", "LINUX_ARM64", "MACOS", "WINDOWS"])
        jobs[`${key}_RESULT`] = "success";
      const recorded = await run("just", ["release-outcome"], { env: { ...env, ...jobs } });
      assert.equal(recorded.code, 0, said(recorded));
      const record = JSON.parse(
        s.github
          .release(TAG)
          .assets.find((a) => a.name === "release-outcome.json")
          .content.toString(),
      );
      const crate = record.targets.find((t) => t.id === "crate:onepipeline");
      assert.deepEqual(
        [crate.outcome, crate.published, crate.verified],
        ["shipped-unverified", "failure", "skipped"],
      );
    } finally {
      await s.stop();
    }
  });

  it("a crate the registry serves but cannot be installed is shipped and not verified", async () => {
    const { s, env, results } = await assetsOnly();
    try {
      // The index serves the version, so publish skips the upload; the file
      // behind it is not a crate.
      const content = Buffer.from("not a crate");
      s.crates.crates.set("onepipeline", [
        {
          line: {
            name: "onepipeline",
            vers: VERSION,
            deps: [],
            cksum: sha(content),
            features: {},
            yanked: false,
          },
          content,
        },
      ]);
      const result = await run("just", ["release-publish", "publish"], { cwd: checkout, env });
      assert.equal(result.code, 1, said(result));
      assert.match(result.stderr, /crates\.io onepipeline/);
      assert.deepEqual(results().crate, { published: "success", verified: "failure" });
    } finally {
      await s.stop();
    }
  });

  it("refuses when an artifact disappears from a registry before publish finishes", async () => {
    const { s, env } = await assetsOnly();
    try {
      let listings = 0;
      const lost = `onepipeline-v${VERSION}-x86_64-pc-windows-msvc.zip`;
      s.github.onAssetsListed = (release) => {
        listings += 1;
        // Present when publish first asks, gone by the time it checks again.
        if (listings === 2) release.assets = release.assets.filter((a) => a.name !== lost);
      };
      const result = await run("just", ["release-publish", "publish"], {
        cwd: checkout,
        env: { ...env, CARGO_REGISTRY_TOKEN: "" },
      });
      assert.equal(result.code, 1, said(result));
      assert.match(
        result.stderr,
        new RegExp(
          `a registry does not serve every artifact of this release: missing ${lost.replaceAll(".", "\\.")} on the Release`,
        ),
      );
    } finally {
      await s.stop();
    }
  });
});
