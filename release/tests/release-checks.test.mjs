// The release's own checks, driven through the recipes and scripts the release
// jobs run, against a local GitHub (`support/release-doubles.mjs`):
//
//   * the guard: an ineligible release makes every build job's recipe and
//     `publish` exit before anything is built or uploaded;
//   * the linked engines: a lock that links an engine twice makes `publish` exit
//     before anything is uploaded;
//   * the outcome record: what it writes for a release that succeeded and one
//     that failed;
//   * release-plz.yml's auto-merge step: what it asks GitHub for, and that a
//     refusal fails it.
//
// Each of these needs only bash, Python, Node and `just`, so this runs in the
// deterministic gate; the journeys that build and publish real artifacts are
// `release-linux.journey.mjs`.

import { execFileSync, spawn } from "node:child_process";
import { createHash } from "node:crypto";
import {
  cpSync,
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
import { evaluate, workflows } from "../../.github/workflow-graph.mjs";
import { CratesDouble, GitHubDouble, PyPIDouble } from "./support/release-doubles.mjs";
import { Registry } from "../../npm/test/registry-support.mjs";

const ROOT = process.cwd();
const VERSION = readFileSync(join(ROOT, "Cargo.toml"), "utf8").match(
  /^\[package\][\s\S]*?^version = "([^"]+)"/m,
)[1];
const TAG = `v${VERSION}`;

function scratch(name) {
  return mkdtempSync(join(process.env.ONEPIPELINE_NODE_SCRATCH_DIR ?? tmpdir(), `${name}-`));
}

/// Run a command to completion without blocking this process's event loop,
/// which the doubles answering it run on.
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

/// The npm settings `npmEnv` points npm at a double with, without the rest of
/// `process.env` it also returns: spread over a release environment, that copy
/// would put a runner's own `GITHUB_API_URL` and `GITHUB_TOKEN` back over the
/// GitHub double's, and the recipe would ask the real API.
function npmConfig(npm, home) {
  return Object.fromEntries(
    Object.entries(npm.npmEnv(home)).filter(([name]) => /^npm_config_/i.test(name)),
  );
}

function said(result) {
  return `exit ${result.code}\n--- stdout\n${result.stdout}\n--- stderr\n${result.stderr}`;
}

/// The environment a release job hands its recipe, every registry switched on.
/// Registries a describe block stands up for every run in it, so nothing here
/// asks the public ones.
let ambient = {};

function releaseEnv(github, out, overrides = {}) {
  return {
    ...ambient,
    ...github.env(),
    RELEASE_TAG: TAG,
    PYPI_PUBLISH: "true",
    PYPI_TOKEN: "pypi-not-a-real-token",
    NPM_PUBLISH: "true",
    NODE_AUTH_TOKEN: "npm-not-a-real-token",
    CARGO_REGISTRY_TOKEN: "",
    ONEPIPELINE_RELEASE_OUT: out,
    ...overrides,
  };
}

describe("an ineligible release", () => {
  const workflow = workflows(ROOT)["release.yml"];
  /// Every step of release.yml that runs a release recipe: [job, step].
  const steps = Object.entries(workflow.jobs).flatMap(([job, spec]) =>
    spec.steps.filter((step) => /^just release-/.test(step.run ?? "")).map((step) => [job, step]),
  );
  const CREDENTIALS = ["PYPI_TOKEN", "NODE_AUTH_TOKEN", "CARGO_REGISTRY_TOKEN"];

  /// The environment Actions hands one step: the workflow's and the step's own
  /// `env:`, evaluated for this release, and what every runner sets.
  function stepEnv(github, out, step, { tag, vars, secrets }) {
    const context = {
      github: { event: { release: { tag_name: tag } } },
      vars,
      secrets: { GITHUB_TOKEN: "not-a-real-token", ...secrets },
      needs: Object.fromEntries(
        ["build-linux", "build-macos", "build-windows"].map((job) => [
          job,
          { result: "success", outputs: { published: "success", verified: "success" } },
        ]),
      ),
    };
    const env = {};
    for (const [key, value] of Object.entries({ ...workflow.env, ...step.env })) {
      env[key] = String(value).replace(/\$\{\{(.*?)\}\}/g, (_, e) =>
        String(evaluate(e, context) ?? ""),
      );
    }
    const { GITHUB_TOKEN: _, ...runner } = github.env();
    return { ...runner, ...env, ONEPIPELINE_RELEASE_OUT: out };
  }

  let github;
  before(async () => {
    github = await new GitHubDouble().start();
    github.addRelease(TAG);
  });
  after(() => github.stop());

  it("hands the publishing credentials to the publish steps alone", () => {
    for (const [job, step] of steps) {
      const carried = CREDENTIALS.filter((name) => Object.hasOwn(step.env ?? {}, name));
      if (/ publish$/.test(step.run)) {
        assert.ok(
          carried.length > 0,
          `${job}'s ${step.run} carries the credentials it publishes with`,
        );
      } else {
        assert.deepEqual(carried, [], `${job}'s ${step.run} carries no publishing credential`);
      }
    }
  });

  const allOn = { PYPI_PUBLISH: "true", NPM_PUBLISH: "true" };
  const allSecrets = { PYPI_TOKEN: "pypi", NPM_TOKEN: "npm", CARGO_REGISTRY_TOKEN: "crates" };
  // A tag that names no version, or another version, refuses every recipe step
  // but the outcome record, which records a refused release too; a registry
  // switched on with no credential refuses the steps that publish.
  const recipes = (step) => step.run !== "just release-outcome";
  const cases = [
    [
      "a tag that is not a version",
      { tag: "nightly", vars: allOn, secrets: allSecrets },
      /'nightly' is not a vX\.Y\.Z version tag/,
      recipes,
    ],
    [
      "a tag naming another version",
      { tag: "v0.0.1", vars: allOn, secrets: allSecrets },
      /does not name Cargo\.toml's version/,
      recipes,
    ],
    [
      "PyPI switched on with no token",
      { tag: TAG, vars: allOn, secrets: { ...allSecrets, PYPI_TOKEN: "" } },
      /PYPI_PUBLISH is on but PYPI_TOKEN is empty/,
      (step) => Object.hasOwn(step.env ?? {}, "PYPI_TOKEN"),
    ],
    [
      "npm switched on with no token",
      { tag: TAG, vars: allOn, secrets: { ...allSecrets, NPM_TOKEN: "" } },
      /NPM_PUBLISH is on but NODE_AUTH_TOKEN is empty/,
      (step) => Object.hasOwn(step.env ?? {}, "NODE_AUTH_TOKEN"),
    ],
  ];
  for (const [name, release, reason, applies] of cases) {
    it(`${name}: every step it applies to exits before building or uploading`, async () => {
      const ran = [];
      for (const [job, step] of steps.filter(([, step]) => applies(step))) {
        const out = scratch("release-out");
        try {
          const before = github.requests.length;
          const result = await run("bash", ["-eo", "pipefail", "-c", step.run], {
            env: stepEnv(github, out, step, release),
          });
          assert.equal(result.code, 1, `${job}: ${said(result)}`);
          assert.match(result.stderr, reason, `${job}: ${said(result)}`);
          assert.match(result.stderr, /^ACTION: /m, "a refusal names what to do next");
          assert.deepEqual(
            github.requests.slice(before),
            [],
            `${job}: refused before asking GitHub anything`,
          );
          for (const platform of ["linux", "macos", "windows", "common"]) {
            assert.ok(!existsSync(join(out, platform, "build.json")), `${job}: nothing was built`);
          }
          ran.push(`${job}: ${step.run}`);
        } finally {
          rmSync(out, { recursive: true, force: true });
        }
      }
      assert.ok(
        ran.some((r) => r.startsWith("build-linux") && r.endsWith(" publish")),
        ran.join("\n"),
      );
      assert.ok(
        ran.some((r) => r.startsWith("publish: just release-publish")),
        ran.join("\n"),
      );
    });
  }
});

describe("a release phase refuses what it cannot trust", () => {
  let github;
  before(async () => {
    github = await new GitHubDouble().start();
    github.addRelease(TAG);
  });
  after(() => github.stop());

  it("leaves a non-empty directory it did not make alone, and builds nothing into it", async () => {
    const out = scratch("release-foreign");
    mkdirSync(join(out, "linux"));
    writeFileSync(join(out, "linux", "somebody-elses.txt"), "keep me");
    try {
      const result = await run("just", ["release-linux", "build"], {
        env: releaseEnv(github, out),
      });
      assert.equal(result.code, 1, said(result));
      assert.match(result.stderr, /is not empty and is not a directory this script made/);
      assert.equal(readFileSync(join(out, "linux", "somebody-elses.txt"), "utf8"), "keep me");
    } finally {
      rmSync(out, { recursive: true, force: true });
    }
  });

  it("refuses a build manifest it did not write, before asking GitHub anything", async () => {
    const out = scratch("release-manifest");
    mkdirSync(join(out, "linux"));
    const plan = JSON.parse((await run("just", ["release-linux", "plan"])).stdout);
    const good = {
      schema_version: 1,
      platform: "linux",
      tag: TAG,
      version: VERSION,
      artifacts: plan.artifacts,
    };
    try {
      for (const [manifest, reason] of [
        ["{not json", /no build in/],
        [{ ...good, schema_version: 2 }, /is not a build this script wrote/],
        [{ ...good, extra: true }, /is not a build this script wrote/],
        [{ ...good, artifacts: "everything" }, /lists no artifacts/],
        [{ ...good, artifacts: plan.artifacts.slice(1) }, /are not the plan's/],
        [
          {
            ...good,
            artifacts: plan.artifacts.map((a) => ({ ...a, path: a.file, sha256: "nope" })),
          },
          /is not the plan's .* with its path and hash/,
        ],
      ]) {
        writeFileSync(
          join(out, "linux", "build.json"),
          typeof manifest === "string" ? manifest : JSON.stringify(manifest),
        );
        const before = github.requests.length;
        const result = await run("just", ["release-linux", "publish"], {
          env: releaseEnv(github, out),
        });
        assert.equal(result.code, 1, said(result));
        assert.match(result.stderr, reason, said(result));
        assert.deepEqual(github.requests.slice(before), []);
      }
    } finally {
      rmSync(out, { recursive: true, force: true });
    }
  });

  it("refuses to verify against a Release that is missing, unreachable, malformed or incomplete", async () => {
    const out = scratch("release-verify");
    try {
      const verify = (overrides) =>
        run("just", ["release-linux", "verify"], { env: releaseEnv(github, out, overrides) });
      let result = await verify({
        RELEASE_TAG: TAG,
        GITHUB_REPOSITORY: "nickderobertis/elsewhere",
      });
      assert.equal(result.code, 1, said(result));
      assert.match(result.stderr, /GitHub answered 404 for the Release tagged/);
      result = await verify({ GITHUB_API_URL: "http://127.0.0.1:9" });
      assert.equal(result.code, 1, said(result));
      assert.match(result.stderr, /did not answer/);
      github.assetsBody = '{"not":"a list"}';
      result = await verify({});
      github.assetsBody = null;
      assert.equal(result.code, 1, said(result));
      assert.match(result.stderr, /the Release assets is not the shape the service documents/);
      result = await verify({});
      assert.equal(result.code, 1, said(result));
      assert.match(
        result.stderr,
        /the Release serves no onepipeline-v\S+-x86_64-unknown-linux-gnu\.tar\.gz/,
      );
      github.release(TAG).assets.push({
        id: 9001,
        name: `onepipeline-v${VERSION}-x86_64-unknown-linux-gnu.tar.gz`,
        content: Buffer.from("x"),
      });
      result = await verify({});
      github.release(TAG).assets = [];
      assert.equal(result.code, 1, said(result));
      assert.match(
        result.stderr,
        /the Release serves no onepipeline-v\S+-x86_64-unknown-linux-gnu\.sha256/,
      );
    } finally {
      rmSync(out, { recursive: true, force: true });
    }
  });
});

describe("a Release that names another host", () => {
  it("is refused before the token or a download goes there", async () => {
    // A local crate registry, and npm off in every run's own overrides (where
    // `releaseEnv` cannot turn it back on), so the outcome's registry questions
    // stay on this machine.
    const crates = await new CratesDouble().start();
    ambient = { ONEPIPELINE_CRATE_REGISTRY: `${crates.url}/index/` };
    const github = await new GitHubDouble().start();
    const release = github.addRelease(TAG);
    const out = scratch("release-hosts");
    const outcomeEnv = (overrides) =>
      releaseEnv(github, out, { NPM_PUBLISH: "false", ...overrides });
    try {
      const plain = await run("just", ["release-outcome"], {
        env: outcomeEnv({ LINUX_RESULT: "success", GITHUB_API_URL: "http://192.0.2.1" }),
      });
      assert.equal(plain.code, 1, said(plain));
      assert.match(plain.stderr, /http:\/\/192\.0\.2\.1 is not an HTTPS URL/);
      // A token is never carried through a redirect, wherever it points.
      github.redirectNext("GET", /\/releases\/tags\//, `${github.url}/elsewhere`);
      const redirected = await run("just", ["release-outcome"], {
        env: outcomeEnv({ LINUX_RESULT: "success" }),
      });
      assert.equal(redirected.code, 1, said(redirected));
      assert.match(redirected.stderr, /redirected a request carrying a token to/);
      assert.ok(
        !github.requests.some((r) => r.path === "/elsewhere"),
        "the redirect was not followed",
      );
      github.uploadOrigin = "https://uploads.example.com";
      let result = await run("just", ["release-outcome"], {
        env: outcomeEnv({ LINUX_RESULT: "success" }),
      });
      assert.equal(result.code, 1, said(result));
      assert.match(
        result.stderr,
        /the Release's upload URL https:\/\/uploads\.example\.com\S* is not GitHub's/,
      );
      assert.deepEqual(github.writes(), [], "nothing was sent to the other host's URL");
      github.uploadOrigin = null;
      github.downloadOrigin = "http://192.0.2.1";
      for (const name of [
        `onepipeline-v${VERSION}-x86_64-unknown-linux-gnu.tar.gz`,
        `onepipeline-v${VERSION}-x86_64-unknown-linux-gnu.sha256`,
      ]) {
        release.assets.push({ id: github.nextId++, name, content: Buffer.from(name) });
      }
      result = await run("just", ["release-linux", "verify"], {
        env: releaseEnv(github, out, { PYPI_PUBLISH: "false", NPM_PUBLISH: "false" }),
      });
      assert.equal(result.code, 1, said(result));
      assert.match(
        result.stderr,
        /is not an HTTPS URL|download URL http:\/\/192\.0\.2\.1\S* is not GitHub's/,
      );
    } finally {
      ambient = {};
      await crates.stop();
      await github.stop();
      rmSync(out, { recursive: true, force: true });
    }
  });
});

describe("verification refuses a downloaded archive that is not the documents and the executable", () => {
  /// A Windows zip archive and its checksum on a fresh Release, built from
  /// `members` ([name, "file" | "symlink" | "directory"]), then verified.
  async function verifyWindowsZip(members) {
    const github = await new GitHubDouble().start();
    const release = github.addRelease(TAG);
    const content = Buffer.from(
      execFileSync("python3", ["-c", ZIP, JSON.stringify(members)], { encoding: "latin1" }),
      "latin1",
    );
    const file = `onepipeline-v${VERSION}-x86_64-pc-windows-msvc.zip`;
    release.assets.push({ id: github.nextId++, name: file, content });
    release.assets.push({
      id: github.nextId++,
      name: `onepipeline-v${VERSION}-x86_64-pc-windows-msvc.sha256`,
      content: Buffer.from(`${createHash("sha256").update(content).digest("hex")}  ${file}\n`),
    });
    const out = scratch("release-verify-zip");
    try {
      return await run("just", ["release-windows", "verify"], {
        env: releaseEnv(github, out, { PYPI_PUBLISH: "false", NPM_PUBLISH: "false" }),
      });
    } finally {
      await github.stop();
      rmSync(out, { recursive: true, force: true });
    }
  }
  const ZIP = `
import io, json, sys, zipfile
buffer = io.BytesIO()
with zipfile.ZipFile(buffer, 'w') as bundle:
    for name, kind in json.loads(sys.argv[1]):
        info = zipfile.ZipInfo(name + ('/' if kind == 'directory' else ''))
        info.external_attr = {'file': 0o100644, 'symlink': 0o120777, 'directory': 0o040755}[kind] << 16
        bundle.writestr(info, '' if kind == 'directory' else ('README.md' if kind == 'symlink' else name))
sys.stdout.buffer.write(buffer.getvalue())
`;
  const documents = [
    ["README.md", "file"],
    ["LICENSE", "file"],
    ["CHANGELOG.md", "file"],
  ];

  it("follows a download's redirect without the token, and refuses one to plaintext", async () => {
    const github = await new GitHubDouble().start();
    const release = github.addRelease(TAG);
    const content = archive([["README.md"], ["LICENSE"], ["CHANGELOG.md"]]);
    const file = `onepipeline-v${VERSION}-x86_64-unknown-linux-gnu.tar.gz`;
    const id = github.nextId++;
    release.assets.push({ id, name: file, content });
    release.assets.push({
      id: github.nextId++,
      name: `onepipeline-v${VERSION}-x86_64-unknown-linux-gnu.sha256`,
      content: Buffer.from(`${createHash("sha256").update(content).digest("hex")}  ${file}\n`),
    });
    const out = scratch("release-redirect");
    const env = releaseEnv(github, out, { PYPI_PUBLISH: "false", NPM_PUBLISH: "false" });
    try {
      // Storage answers GitHub's download URL with a redirect: followed, and the
      // archive it serves is the one judged (this one has no executable).
      github.redirectNext(
        "GET",
        new RegExp(`^/download/${id}/`),
        `${github.url}/moved/${id}/${file}`,
      );
      let result = await run("just", ["release-linux", "verify"], { env });
      assert.equal(result.code, 1, said(result));
      assert.ok(
        github.requests.some((r) => r.path.startsWith(`/moved/${id}/`)),
        "the redirect was followed",
      );
      assert.match(result.stderr, /\.tar\.gz is not \[.*\], each a regular file/);
      github.redirectNext(
        "GET",
        new RegExp(`^/download/${id}/`),
        `http://192.0.2.1/moved/${id}/${file}`,
      );
      result = await run("just", ["release-linux", "verify"], { env });
      assert.equal(result.code, 1, said(result));
      assert.match(result.stderr, /http:\/\/192\.0\.2\.1\S* is not an HTTPS URL/);
    } finally {
      await github.stop();
      rmSync(out, { recursive: true, force: true });
    }
  });

  it("refuses a repository that is not an owner and a name, before asking GitHub", async () => {
    const github = await new GitHubDouble().start();
    const out = scratch("release-repo");
    try {
      const result = await run("just", ["release-linux", "verify"], {
        env: releaseEnv(github, out, { GITHUB_REPOSITORY: "owner/name/releases?x=1" }),
      });
      assert.equal(result.code, 1, said(result));
      assert.match(
        result.stderr,
        /GITHUB_REPOSITORY 'owner\/name\/releases\?x=1' is not an owner\/name/,
      );
      const step = await run("bash", ["release/release-pr-auto-merge.sh"], {
        env: { ...github.env(), GITHUB_REPOSITORY: "owner/{a,b}" },
      });
      assert.equal(step.code, 1, said(step));
      assert.match(step.stderr, /is not an owner\/name/);
      assert.deepEqual(github.requests, []);
    } finally {
      await github.stop();
      rmSync(out, { recursive: true, force: true });
    }
  });

  it("extracts a Windows zip that is the documents and onepipeline.exe, then smokes what it unpacked", async () => {
    const result = await verifyWindowsZip([...documents, ["onepipeline.exe", "file"]]);
    // A Windows executable cannot run here, so the smoke refuses to stand in
    // another onepipeline for it: reaching that refusal is the proof the
    // archive was accepted and unpacked.
    assert.equal(result.code, 1, said(result));
    assert.doesNotMatch(result.stderr, /each a regular file|put no onepipeline executable/);
    assert.match(
      result.stderr,
      /PATH resolves onepipeline to .*, not the one Release archive \S+\.zip installed in/,
    );
  });

  for (const [name, members] of [
    ["omits onepipeline.exe", documents],
    ["links onepipeline.exe to another file", [...documents, ["onepipeline.exe", "symlink"]]],
    ["carries onepipeline.exe as a directory", [...documents, ["onepipeline.exe", "directory"]]],
  ]) {
    it(`refuses a Windows zip that ${name}`, async () => {
      const result = await verifyWindowsZip(members);
      assert.equal(result.code, 1, said(result));
      assert.match(result.stderr, /\.zip is not \[.*\], each a regular file/, said(result));
    });
  }

  /// A tar.gz and its checksum on the Release, built from `members` ([name, tar flag]).
  function archive(members) {
    const dir = scratch("archive");
    const list = [];
    for (const [name, kind] of members) {
      if (kind === "symlink") execFileSync("ln", ["-s", "README.md", join(dir, name)]);
      else writeFileSync(join(dir, name), `${name}\n`);
      list.push(name);
    }
    const bytes = execFileSync("tar", ["-czf", "-", "-C", dir, ...list]);
    rmSync(dir, { recursive: true, force: true });
    return bytes;
  }

  for (const [name, members] of [
    ["carries no executable", [["README.md"], ["LICENSE"], ["CHANGELOG.md"]]],
    [
      "links its executable to another file",
      [["README.md"], ["LICENSE"], ["CHANGELOG.md"], ["onepipeline", "symlink"]],
    ],
  ]) {
    it(`refuses an archive that ${name}, before running anything from it`, async () => {
      const github = await new GitHubDouble().start();
      const release = github.addRelease(TAG);
      const content = archive(members);
      const file = `onepipeline-v${VERSION}-x86_64-unknown-linux-gnu.tar.gz`;
      release.assets.push({ id: github.nextId++, name: file, content });
      release.assets.push({
        id: github.nextId++,
        name: `onepipeline-v${VERSION}-x86_64-unknown-linux-gnu.sha256`,
        content: Buffer.from(`${createHash("sha256").update(content).digest("hex")}  ${file}\n`),
      });
      const out = scratch("release-verify");
      try {
        const result = await run("just", ["release-linux", "verify"], {
          env: releaseEnv(github, out, { PYPI_PUBLISH: "false", NPM_PUBLISH: "false" }),
        });
        assert.equal(result.code, 1, said(result));
        assert.match(result.stderr, /\.tar\.gz is not \[.*\], each a regular file/);
        assert.doesNotMatch(result.stdout + result.stderr, /surface smoke test/);
      } finally {
        await github.stop();
        rmSync(out, { recursive: true, force: true });
      }
    });
  }
});

describe("publish asks every registry before it writes anything", () => {
  /// A Release carrying every platform's planned assets, as the build jobs leave it.
  async function everyPlatformOnTheRelease() {
    const github = await new GitHubDouble().start();
    const release = github.addRelease(TAG);
    for (const platform of ["linux", "macos", "windows"]) {
      for (const artifact of JSON.parse((await run("just", [`release-${platform}`, "plan"])).stdout)
        .artifacts) {
        if (artifact.destination === "github-release") {
          release.assets.push({
            id: github.nextId++,
            name: artifact.file,
            content: Buffer.from(artifact.file),
          });
        }
      }
    }
    return github;
  }

  async function publishWith(github, overrides) {
    const before = github.writes().length;
    const result = await run("just", ["release-publish", "publish"], {
      env: releaseEnv(github, scratch("release-common"), {
        PYPI_PUBLISH: "false",
        NPM_PUBLISH: "false",
        ...overrides,
      }),
    });
    assert.equal(result.code, 1, said(result));
    assert.equal(github.writes().length, before, "the Release was not written to, notes included");
    return result;
  }

  it("refuses a PyPI or crate registry reached over plaintext, before sending it anything", async () => {
    const github = await everyPlatformOnTheRelease();
    try {
      for (const overrides of [
        { PYPI_PUBLISH: "true", ONEPIPELINE_PYPI_JSON_URL: "http://192.0.2.1/pypi" },
        {
          CARGO_REGISTRY_TOKEN: "crates-not-a-real-token",
          ONEPIPELINE_CRATE_REGISTRY: "http://192.0.2.1/index/",
        },
      ]) {
        const result = await publishWith(github, overrides);
        assert.match(result.stderr, /http:\/\/192\.0\.2\.1\S* is not an HTTPS URL/, said(result));
      }
    } finally {
      await github.stop();
    }
  });

  it("refuses when PyPI fails or garbles its answer", async () => {
    const github = await everyPlatformOnTheRelease();
    const pypi = await new PyPIDouble().start();
    try {
      pypi.refuseNext("GET", /\/json$/, 500);
      let result = await publishWith(github, { ...pypi.env(), PYPI_PUBLISH: "true" });
      assert.match(result.stderr, /PyPI answered 500/);
      pypi.refuseNext("GET", /\/json$/, 200, "not a release");
      result = await publishWith(github, { ...pypi.env(), PYPI_PUBLISH: "true" });
      assert.match(result.stderr, /is not the shape the service documents/);
    } finally {
      await Promise.all([github.stop(), pypi.stop()]);
    }
  });

  it("refuses when npm cannot be asked", async () => {
    const github = await everyPlatformOnTheRelease();
    const npm = new Registry();
    await npm.start();
    const home = scratch("npm-home");
    try {
      npm.refuseWith(401);
      const result = await publishWith(github, { ...npmConfig(npm, home), NPM_PUBLISH: "true" });
      assert.match(result.stderr, /npm could not be asked about onepipeline-cli-/);
    } finally {
      await Promise.all([github.stop(), npm.stop()]);
      rmSync(home, { recursive: true, force: true });
    }
  });

  it("refuses when the crates.io index fails or garbles its answer", async () => {
    const github = await everyPlatformOnTheRelease();
    const crates = await new CratesDouble().start();
    try {
      crates.refuseNext("GET", /^\/index\//, 500);
      let result = await publishWith(github, crates.env());
      assert.match(result.stderr, /the crates\.io index answered 500/);
      crates.refuseNext("GET", /^\/index\//, 200, "no version here");
      result = await publishWith(github, crates.env());
      assert.match(result.stderr, /the index entry for onepipeline is not the shape/);
      crates.refuseNext("GET", /^\/index\//, 200, "", true);
      result = await publishWith(github, crates.env());
      assert.match(result.stderr, /answered 200 with no entry for onepipeline/);
      assert.equal(crates.writes().length, 0);
    } finally {
      await Promise.all([github.stop(), crates.stop()]);
    }
  });
});

describe("a lock that links an engine twice", () => {
  let github;
  let copy;
  before(async () => {
    github = await new GitHubDouble().start();
    github.addRelease(TAG);
    // The repository's own files with one change: Cargo.lock carries a second,
    // older copy of a linked engine.
    copy = scratch("release-split");
    for (const path of [
      "scripts",
      "release",
      "Cargo.toml",
      "justfile",
      "README.md",
      "LICENSE",
      "CHANGELOG.md",
    ]) {
      cpSync(join(ROOT, path), join(copy, path), { recursive: true });
    }
    writeFileSync(
      join(copy, "Cargo.lock"),
      `${readFileSync(join(ROOT, "Cargo.lock"), "utf8")}\n[[package]]\nname = "onevcs"\nversion = "0.0.1"\nsource = "registry+https://github.com/rust-lang/crates.io-index"\n`,
    );
  });
  after(async () => {
    await github.stop();
    rmSync(copy, { recursive: true, force: true });
  });

  it("makes `publish` exit naming the engine, before any upload", async () => {
    const result = await run("just", ["release-publish", "publish"], {
      cwd: copy,
      env: releaseEnv(github, join(copy, "out"), {
        CARGO_REGISTRY_TOKEN: "crates-not-a-real-token",
      }),
    });
    assert.equal(result.code, 1, said(result));
    assert.match(result.stderr, /resolves an engine at more than one version/, said(result));
    assert.match(result.stderr, /onevcs: .*0\.0\.1/, said(result));
    assert.deepEqual(github.writes(), [], "nothing was written to the Release");
    assert.equal(github.release(TAG).assets.length, 0);
  });

  it("keeps the last publish's results through a dry run", async () => {
    const out = scratch("release-dry-results");
    mkdirSync(join(out, "common"), { recursive: true });
    const results = join(out, "common", "publish-results.json");
    writeFileSync(results, '{"kept": true}');
    try {
      const dry = await run("just", ["release-publish", "dry-run"], {
        env: releaseEnv(github, out),
      });
      assert.equal(dry.code, 0, said(dry));
      assert.equal(readFileSync(results, "utf8"), '{"kept": true}');
    } finally {
      rmSync(out, { recursive: true, force: true });
    }
  });

  it("is not refused when the lock links each engine once, and `publish`'s dry run then writes nothing", async () => {
    const before = github.requests.length;
    const dry = await run("just", ["release-publish", "dry-run"], {
      env: releaseEnv(github, scratch("release-dry"), {
        CARGO_REGISTRY_TOKEN: "crates-not-a-real-token",
      }),
    });
    assert.equal(dry.code, 0, said(dry));
    for (const file of [`onepipeline-${VERSION}.crate`, `onepipeline-cli-${VERSION}.tgz`]) {
      assert.ok(dry.stdout.includes(`would publish ${file}`), said(dry));
    }
    assert.match(dry.stdout, /dry run: published nothing platform-independent/);
    assert.deepEqual(github.requests.slice(before), [], "a dry run does not even read the Release");

    const result = await run("bash", ["scripts/linked-engines.sh", "--format", "split"]);
    assert.equal(result.code, 0, said(result));
    assert.match(result.stdout, /linked engines resolve once each/);
  });
});

describe("the release outcome record", () => {
  let github;
  let out;
  let crates;
  let npm;
  let pypi;
  let home;
  before(async () => {
    github = await new GitHubDouble().start();
    github.addRelease(TAG);
    out = scratch("release-outcome");
    crates = await new CratesDouble().start();
    pypi = await new PyPIDouble().start();
    npm = new Registry();
    await npm.start();
    home = scratch("npm-home");
    ambient = {
      ...npmConfig(npm, home),
      ...pypi.env(),
      ONEPIPELINE_CRATE_REGISTRY: `${crates.url}/index/`,
    };
  });
  after(async () => {
    ambient = {};
    await Promise.all([github.stop(), crates.stop(), pypi.stop(), npm.stop()]);
    rmSync(out, { recursive: true, force: true });
    rmSync(home, { recursive: true, force: true });
  });

  it("on a fresh runner with no results, records a launcher npm already serves as shipped, not verified", async () => {
    rmSync(join(out, "common", "publish-results.json"), { force: true });
    const launcher = execFileSync(
      "node",
      ["scripts/npm-build.mjs", "launcher", "--out", scratch("launcher")],
      {
        cwd: ROOT,
        encoding: "utf8",
      },
    ).trim();
    const published = await run("npm", ["publish", launcher, "--access", "public"], {
      env: { ...ambient },
    });
    assert.equal(published.code, 0, said(published));
    const jobs = {};
    for (const key of ["LINUX", "MACOS", "WINDOWS"]) {
      Object.assign(jobs, {
        [`${key}_RESULT`]: "success",
        [`${key}_PUBLISHED`]: "success",
        [`${key}_VERIFIED`]: "success",
      });
    }
    const result = await run("just", ["release-outcome"], { env: releaseEnv(github, out, jobs) });
    assert.equal(result.code, 0, said(result));
    assert.deepEqual(states(recorded())["npm:onepipeline-cli"], [
      "shipped-unverified",
      "success",
      "skipped",
    ]);
    // Unserved again, so the cases after this one see a registry without it.
    npm.packages.delete("onepipeline-cli");
  });

  it("on a fresh runner with no results, records what the registries already serve as shipped", async () => {
    rmSync(join(out, "common", "publish-results.json"), { force: true });
    crates.crates.set("onepipeline", [
      {
        line: {
          name: "onepipeline",
          vers: VERSION,
          deps: [],
          cksum: "0".repeat(64),
          features: {},
          yanked: false,
        },
        content: Buffer.from("crate"),
      },
    ]);
    try {
      const jobs = {};
      for (const key of ["LINUX", "MACOS", "WINDOWS"]) jobs[`${key}_RESULT`] = "success";
      const result = await run("just", ["release-outcome"], {
        env: releaseEnv(github, out, { ...jobs, NPM_PUBLISH: "false" }),
      });
      assert.equal(result.code, 0, said(result));
      assert.deepEqual(states(recorded())["crate:onepipeline"], [
        "shipped-unverified",
        "success",
        "skipped",
      ]);
    } finally {
      crates.crates.clear();
    }
  });

  function recorded() {
    const asset = github.release(TAG).assets.filter((a) => a.name === "release-outcome.json");
    assert.equal(asset.length, 1, "the Release carries exactly one outcome record");
    return JSON.parse(asset[0].content.toString("utf8"));
  }

  function states(record) {
    return Object.fromEntries(
      record.targets.map((t) => [t.id, [t.outcome, t.published, t.verified]]),
    );
  }

  it("records a release every job of which succeeded as shipped and verified", async () => {
    const results = join(out, "common");
    mkdirSync(results, { recursive: true });
    writeFileSync(
      join(results, "publish-results.json"),
      JSON.stringify({
        crate: { published: "success", verified: "success" },
        "npm-launcher": { published: "success", verified: "success" },
      }),
    );
    const jobs = {};
    for (const key of ["LINUX", "MACOS", "WINDOWS"]) {
      Object.assign(jobs, {
        [`${key}_RESULT`]: "success",
        [`${key}_PUBLISHED`]: "success",
        [`${key}_VERIFIED`]: "success",
      });
    }
    const result = await run("just", ["release-outcome"], { env: releaseEnv(github, out, jobs) });
    assert.equal(result.code, 0, said(result));
    const record = recorded();
    assert.equal(record.version, VERSION);
    assert.equal(record.outcome, "shipped-verified");
    assert.equal(record.run_url, "https://github.com/nickderobertis/onepipeline/actions/runs/4242");
    assert.deepEqual(states(record), {
      "crate:onepipeline": ["shipped-verified", "success", "success"],
      "pypi:onepipeline-cli": ["shipped-verified", "success", "success"],
      "npm:onepipeline-cli": ["shipped-verified", "success", "success"],
    });
  });

  it("records a release whose Linux build failed, replacing the earlier record", async () => {
    // Linux failed while building, so it published nothing, and `publish`'s
    // publish step never ran, so it left no results.
    rmSync(join(out, "common", "publish-results.json"), { force: true });
    const jobs = {
      LINUX_RESULT: "failure",
      LINUX_PUBLISHED: "skipped",
      LINUX_VERIFIED: "skipped",
      MACOS_RESULT: "success",
      MACOS_PUBLISHED: "success",
      MACOS_VERIFIED: "success",
      WINDOWS_RESULT: "success",
      WINDOWS_PUBLISHED: "success",
      WINDOWS_VERIFIED: "success",
    };
    const result = await run("just", ["release-outcome"], { env: releaseEnv(github, out, jobs) });
    assert.equal(result.code, 0, said(result));
    const record = recorded();
    assert.notEqual(record.outcome, "shipped-verified");
    assert.deepEqual(states(record), {
      "crate:onepipeline": ["nothing-shipped", "skipped", "skipped"],
      "pypi:onepipeline-cli": ["shipped-unverified", "success", "failure"],
      "npm:onepipeline-cli": ["shipped-unverified", "success", "failure"],
    });
  });

  it("records nothing shipped only when no build job reached its publish step", async () => {
    const failedBuilding = {};
    for (const key of ["LINUX", "MACOS", "WINDOWS"]) {
      Object.assign(failedBuilding, {
        [`${key}_RESULT`]: "failure",
        [`${key}_PUBLISHED`]: "skipped",
        [`${key}_VERIFIED`]: "skipped",
      });
    }
    let result = await run("just", ["release-outcome"], {
      env: releaseEnv(github, out, failedBuilding),
    });
    assert.equal(result.code, 0, said(result));
    assert.equal(recorded().outcome, "nothing-shipped");
    assert.deepEqual(states(recorded())["pypi:onepipeline-cli"], [
      "nothing-shipped",
      "skipped",
      "skipped",
    ]);
    // A cancelled job reports no step outcomes, and may have stopped between
    // two uploads, so it is not proof that nothing shipped.
    const cancelled = {};
    for (const key of ["LINUX", "MACOS", "WINDOWS"]) cancelled[`${key}_RESULT`] = "cancelled";
    result = await run("just", ["release-outcome"], { env: releaseEnv(github, out, cancelled) });
    assert.equal(result.code, 0, said(result));
    assert.deepEqual(states(recorded())["pypi:onepipeline-cli"], [
      "shipped-unverified",
      "cancelled",
      "skipped",
    ]);
  });

  function allJobs(published, verified) {
    const jobs = {};
    for (const key of ["LINUX", "MACOS", "WINDOWS"]) {
      Object.assign(jobs, {
        [`${key}_RESULT`]: "success",
        [`${key}_PUBLISHED`]: published[key] ?? "success",
        [`${key}_VERIFIED`]: verified[key] ?? "success",
      });
    }
    return jobs;
  }

  function launcherResults(published, verified) {
    mkdirSync(join(out, "common"), { recursive: true });
    writeFileSync(
      join(out, "common", "publish-results.json"),
      JSON.stringify({
        crate: { published: "success", verified: "success" },
        "npm-launcher": { published, verified },
      }),
    );
  }

  it("records a build job whose publish step failed as shipped but not verified", async () => {
    launcherResults("success", "success");
    const result = await run("just", ["release-outcome"], {
      env: releaseEnv(github, out, { ...allJobs({ WINDOWS: "failure" }, { WINDOWS: "skipped" }) }),
    });
    assert.equal(result.code, 0, said(result));
    assert.deepEqual(states(recorded())["pypi:onepipeline-cli"], [
      "shipped-unverified",
      "success",
      "failure",
    ]);
  });

  it("records a release everything of which published but one verification failed", async () => {
    launcherResults("success", "failure");
    const result = await run("just", ["release-outcome"], {
      env: releaseEnv(github, out, allJobs({}, { MACOS: "failure" })),
    });
    assert.equal(result.code, 0, said(result));
    const record = recorded();
    assert.equal(record.outcome, "shipped-unverified");
    assert.deepEqual(states(record)["pypi:onepipeline-cli"], [
      "shipped-unverified",
      "success",
      "failure",
    ]);
    assert.deepEqual(states(record)["npm:onepipeline-cli"], [
      "shipped-unverified",
      "success",
      "failure",
    ]);
    assert.deepEqual(states(record)["crate:onepipeline"], [
      "shipped-verified",
      "success",
      "success",
    ]);
  });

  it("records every publisher succeeding but a verification cancelled or skipped as not verified", async () => {
    for (const verification of ["cancelled", "skipped"]) {
      launcherResults("success", "success");
      const result = await run("just", ["release-outcome"], {
        env: releaseEnv(github, out, allJobs({}, { LINUX: verification })),
      });
      assert.equal(result.code, 0, said(result));
      const record = recorded();
      assert.equal(record.outcome, "shipped-unverified");
      for (const id of ["pypi:onepipeline-cli", "npm:onepipeline-cli"]) {
        assert.deepEqual(states(record)[id], ["shipped-unverified", "success", verification], id);
      }
    }
  });

  it("on a re-run whose build stops before publishing, records what an earlier run shipped as shipped", async () => {
    launcherResults("success", "success");
    // The earlier run shipped Linux's wheel and npm package; this run's Linux
    // build failed before its publish step, so it reports nothing published.
    pypi.serve(
      `onepipeline_cli-${VERSION}-py3-none-manylinux_2_17_x86_64.manylinux2014_x86_64.whl`,
      "wheel",
    );
    const platform = scratch("linux-x64-package");
    writeFileSync(
      join(platform, "package.json"),
      JSON.stringify({ name: "onepipeline-cli-linux-x64", version: VERSION }),
    );
    const published = await run("npm", ["publish", platform, "--access", "public"], {
      env: { ...ambient },
    });
    assert.equal(published.code, 0, said(published));
    try {
      const failed = {
        LINUX_RESULT: "failure",
        LINUX_PUBLISHED: "skipped",
        LINUX_VERIFIED: "skipped",
      };
      const result = await run("just", ["release-outcome"], {
        env: releaseEnv(github, out, { ...allJobs({}, {}), ...failed }),
      });
      assert.equal(result.code, 0, said(result));
      const record = recorded();
      assert.equal(record.outcome, "shipped-unverified");
      for (const id of ["pypi:onepipeline-cli", "npm:onepipeline-cli"]) {
        assert.deepEqual(states(record)[id], ["shipped-unverified", "success", "skipped"], id);
      }
    } finally {
      pypi.files.clear();
      npm.packages.delete("onepipeline-cli-linux-x64");
    }
  });

  it("records a registry switched off as skipped", async () => {
    launcherResults("skipped", "skipped");
    const result = await run("just", ["release-outcome"], {
      env: releaseEnv(github, out, { ...allJobs({}, {}), PYPI_PUBLISH: "false", NPM_PUBLISH: "" }),
    });
    assert.equal(result.code, 0, said(result));
    const record = recorded();
    assert.deepEqual(states(record)["pypi:onepipeline-cli"], [
      "nothing-shipped",
      "skipped",
      "skipped",
    ]);
    assert.deepEqual(states(record)["npm:onepipeline-cli"], [
      "nothing-shipped",
      "skipped",
      "skipped",
    ]);
  });

  it("replaces the record on a re-run after GitHub refused to", async () => {
    launcherResults("success", "success");
    const env = releaseEnv(github, out, allJobs({}, {}));
    for (const [method, pattern] of [
      ["DELETE", /\/releases\/assets\/\d+$/],
      ["POST", /^\/uploads\//],
    ]) {
      github.refuseNext(method, pattern, 500, "Internal Server Error");
      const refused = await run("just", ["release-outcome"], { env });
      assert.equal(refused.code, 1, said(refused));
      assert.match(
        refused.stderr,
        /GitHub answered 500 (replacing|uploading) release-outcome\.json/,
      );
      const rerun = await run("just", ["release-outcome"], { env });
      assert.equal(rerun.code, 0, said(rerun));
      assert.equal(recorded().outcome, "shipped-verified");
    }
  });

  it("keeps what an earlier publish recorded when a re-run is refused before publishing", async () => {
    launcherResults("success", "success");
    const rerun = await run("just", ["release-publish", "publish"], {
      env: releaseEnv(github, out, { RELEASE_TAG: "v0.0.1" }),
    });
    assert.equal(rerun.code, 1, said(rerun));
    const result = await run("just", ["release-outcome"], {
      env: releaseEnv(github, out, allJobs({}, {})),
    });
    assert.equal(result.code, 0, said(result));
    assert.deepEqual(states(recorded())["crate:onepipeline"], [
      "shipped-verified",
      "success",
      "success",
    ]);
  });

  it("keeps an earlier publish's result for a registry a re-run has switched off", async () => {
    launcherResults("success", "success");
    // The crate shipped on the earlier run; this one has no token and stops at
    // the availability check, as a re-run with a platform still missing does.
    const rerun = await run("just", ["release-publish", "publish"], {
      env: releaseEnv(github, out, {
        CARGO_REGISTRY_TOKEN: "",
        PYPI_PUBLISH: "false",
        NPM_PUBLISH: "false",
      }),
    });
    assert.equal(rerun.code, 1, said(rerun));
    assert.match(rerun.stderr, /not every platform is published/);
    const results = JSON.parse(readFileSync(join(out, "common", "publish-results.json"), "utf8"));
    assert.deepEqual(results.crate, { published: "success", verified: "success" });
    const result = await run("just", ["release-outcome"], {
      env: releaseEnv(github, out, allJobs({}, {})),
    });
    assert.equal(result.code, 0, said(result));
    assert.deepEqual(states(recorded())["crate:onepipeline"], [
      "shipped-verified",
      "success",
      "success",
    ]);
  });

  it("refuses a publish-results file it cannot read rather than reading it as nothing published", async () => {
    mkdirSync(join(out, "common"), { recursive: true });
    for (const content of ["{not json", JSON.stringify({ crate: { published: "yes" } })]) {
      writeFileSync(join(out, "common", "publish-results.json"), content);
      const before = github.writes().length;
      const result = await run("just", ["release-outcome"], {
        env: releaseEnv(github, out, allJobs({}, {})),
      });
      assert.equal(result.code, 1, said(result));
      assert.match(result.stderr, /publish-results\.json is not (JSON|the shape)/, said(result));
      assert.equal(github.writes().length, before, "no record replaced the last one");
    }
    rmSync(join(out, "common", "publish-results.json"));
  });

  it("refuses a job result GitHub does not report rather than guessing", async () => {
    const result = await run("just", ["release-outcome"], {
      env: releaseEnv(github, out, { LINUX_RESULT: "succeeded" }),
    });
    assert.equal(result.code, 1, said(result));
    assert.match(
      result.stderr,
      /LINUX_RESULT is 'succeeded', not a job or step result GitHub reports/,
    );
  });
});

describe("release-plz.yml's auto-merge step", () => {
  const workflow = workflows(ROOT)["release-plz.yml"];
  const step = workflow.jobs["release-plz"].steps.find((s) =>
    /^Merge the release PR automatically/.test(s.name ?? ""),
  );

  /// Run the step's own command with the environment its `env:` names.
  function runStep(github) {
    assert.ok(step?.run, "release-plz.yml has the auto-merge step");
    assert.match(step.env.GITHUB_TOKEN, /^\$\{\{ secrets\.RELEASE_PLZ_TOKEN \}\}$/);
    return run("bash", ["-eo", "pipefail", "-c", step.run], { env: github.env() });
  }

  it("requests auto-merge, conditional on required checks, on release-plz's PR and no other", async () => {
    const github = await new GitHubDouble().start();
    try {
      github.addPull(11, "feat/something");
      github.addPull(12, "release-plz-2026-10-17T00-00-00Z");
      github.addPull(13, "release-plz-from-a-fork", { fork: true });
      const result = await runStep(github);
      assert.equal(result.code, 0, said(result));
      assert.equal(github.graphql.length, 1, "exactly one request to merge");
      const [request] = github.graphql;
      // Auto-merge, which GitHub carries out only once the required checks
      // pass — never the immediate `mergePullRequest`.
      assert.match(request.query, /enablePullRequestAutoMerge/);
      assert.doesNotMatch(request.query, /mergePullRequest\(/);
      assert.equal(request.variables.id, "PR_node_12");
      assert.deepEqual(
        github.pulls.map((p) => [p.number, p.autoMerge ?? null]),
        [
          [11, null],
          [12, "SQUASH"],
          [13, null],
        ],
      );
      assert.match(result.stdout, /Auto-merge requested on release PR #12/);
    } finally {
      await github.stop();
    }
  });

  it("fails visibly when GitHub refuses the request", async () => {
    const github = await new GitHubDouble().start();
    try {
      github.addPull(12, "release-plz-2026-10-17T00-00-00Z");
      github.autoMergeRefusal = "Auto merge is not allowed for this repository";
      const result = await runStep(github);
      assert.equal(result.code, 1, said(result));
      assert.match(result.stderr, /::error::GitHub refused auto-merge on release PR #12/);
      assert.match(result.stderr, /Auto merge is not allowed for this repository/);
      assert.match(result.stderr, /^ACTION: /m);
    } finally {
      await github.stop();
    }
  });

  it("fails visibly when GitHub cannot list the pull requests", async () => {
    const github = await new GitHubDouble().start();
    try {
      github.refuseNext("GET", /\/pulls$/, 403, "Resource not accessible by integration");
      const result = await runStep(github);
      assert.equal(result.code, 1, said(result));
      assert.match(result.stderr, /GitHub answered 403 listing open pull requests/);
      assert.equal(github.graphql.length, 0);
    } finally {
      await github.stop();
    }
  });

  it("fails visibly when GitHub does not answer at all", async () => {
    const github = await new GitHubDouble().start();
    const env = github.env();
    await github.stop();
    const result = await run("bash", ["-eo", "pipefail", "-c", step.run], { env });
    assert.equal(result.code, 1, said(result));
    assert.match(result.stderr, /GitHub did not answer at http:\/\/127\.0\.0\.1/);
  });

  it("refuses to send the token to a plaintext API", async () => {
    const github = await new GitHubDouble().start();
    try {
      for (const name of ["GITHUB_API_URL", "GITHUB_GRAPHQL_URL"]) {
        const result = await run("bash", ["-eo", "pipefail", "-c", step.run], {
          env: { ...github.env(), [name]: "http://192.0.2.1" },
        });
        assert.equal(result.code, 1, said(result));
        assert.match(
          result.stderr,
          /http:\/\/192\.0\.2\.1 is not an HTTPS URL, and RELEASE_PLZ_TOKEN is sent to it/,
        );
      }
      assert.deepEqual(github.requests, []);
    } finally {
      await github.stop();
    }
  });

  it("reads the host off the whole authority, not a prefix", async () => {
    const github = await new GitHubDouble().start();
    try {
      const result = await run("bash", ["-eo", "pipefail", "-c", step.run], {
        env: { ...github.env(), GITHUB_API_URL: "http://localhost:80@192.0.2.1" },
      });
      assert.equal(result.code, 1, said(result));
      assert.match(result.stderr, /is not an HTTPS URL, and RELEASE_PLZ_TOKEN is sent to it/);
      assert.deepEqual(github.requests, []);
    } finally {
      await github.stop();
    }
  });

  it("finds the release PR past the first page, and refuses a second one there", async () => {
    const github = await new GitHubDouble().start();
    try {
      for (let number = 1; number <= 140; number += 1) github.addPull(number, `feat/${number}`);
      github.addPull(141, "release-plz-2026-10-17T00-00-00Z");
      let result = await runStep(github);
      assert.equal(result.code, 0, said(result));
      assert.equal(github.graphql.at(-1).variables.id, "PR_node_141");
      for (let number = 142; number <= 230; number += 1) github.addPull(number, `feat/${number}`);
      github.addPull(231, "release-plz-another");
      result = await runStep(github);
      assert.equal(result.code, 1, said(result));
      assert.match(result.stderr, /2 open release PRs \(#141, #231\)/);
    } finally {
      await github.stop();
    }
  });

  it("fails visibly on a pull request list that is not one", async () => {
    const github = await new GitHubDouble().start();
    try {
      for (const body of [
        '{"message":"an object"}',
        "not json",
        '[{"number":"12","node_id":7,"head":{"ref":"release-plz-a"}}]',
        '[{"number":12,"node_id":"PR_node_12","head":{"ref":"release-plz-a","repo":{"full_name":5}}}]',
        '[{"number":12,"node_id":"","head":{"ref":"release-plz-a","repo":{"full_name":"nickderobertis/onepipeline"}}}]',
      ]) {
        github.pullsBody = body;
        const result = await runStep(github);
        assert.equal(result.code, 1, `${body}: ${said(result)}`);
        assert.match(result.stderr, /not the JSON array of pull requests it documents/);
      }
      assert.equal(github.graphql.length, 0, "nothing was asked to merge");
    } finally {
      await github.stop();
    }
  });

  it("fails visibly when the merge request is refused or answered with nonsense", async () => {
    const github = await new GitHubDouble().start();
    try {
      github.addPull(12, "release-plz-a");
      github.refuseNext("POST", /^\/graphql$/, 502, "Bad Gateway");
      let result = await runStep(github);
      assert.equal(result.code, 1, said(result));
      assert.match(result.stderr, /GitHub refused auto-merge on release PR #12 \(HTTP 502\)/);
      github.graphqlBody = '{"data":{"enablePullRequestAutoMerge":null}}';
      result = await runStep(github);
      assert.equal(result.code, 1, said(result));
      assert.match(result.stderr, /GitHub refused auto-merge on release PR #12 \(HTTP 200\)/);
    } finally {
      await github.stop();
    }
  });

  it("does nothing when no release PR is open, and refuses to guess between two", async () => {
    const github = await new GitHubDouble().start();
    try {
      github.addPull(11, "feat/something");
      let result = await runStep(github);
      assert.equal(result.code, 0, said(result));
      assert.match(result.stdout, /No open release PR to auto-merge/);
      github.addPull(12, "release-plz-a");
      github.addPull(14, "release-plz-b");
      result = await runStep(github);
      assert.equal(result.code, 1, said(result));
      assert.match(result.stderr, /2 open release PRs \(#12, #14\)/);
      assert.equal(github.graphql.length, 0, "nothing was asked to merge");
    } finally {
      await github.stop();
    }
  });
});
