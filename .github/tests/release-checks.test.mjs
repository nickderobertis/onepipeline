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

import { spawn } from "node:child_process";
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
import { parse } from "yaml";
import { GitHubDouble } from "./support/release-doubles.mjs";

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

function said(result) {
  return `exit ${result.code}\n--- stdout\n${result.stdout}\n--- stderr\n${result.stderr}`;
}

/// The environment a release job hands its recipe, every registry switched on.
function releaseEnv(github, out, overrides = {}) {
  return {
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
  let github;
  before(async () => {
    github = await new GitHubDouble().start();
    github.addRelease(TAG);
  });
  after(() => github.stop());

  // The cases release.yml's `guard` job and the tag check every verify job
  // carried, and the credential checks every publish job opened with.
  const cases = [
    [
      "a tag that is not a version",
      { RELEASE_TAG: "nightly" },
      /'nightly' is not a vX\.Y\.Z version tag/,
    ],
    [
      "a tag naming another version",
      { RELEASE_TAG: "v0.0.1" },
      /does not name Cargo\.toml's version/,
    ],
    [
      "PyPI switched on with no token",
      { PYPI_TOKEN: "" },
      /PYPI_PUBLISH is on but PYPI_TOKEN is empty/,
    ],
    [
      "npm switched on with no token",
      { NODE_AUTH_TOKEN: "" },
      /NPM_PUBLISH is on but NODE_AUTH_TOKEN is empty/,
    ],
  ];
  const invocations = [
    ["release-linux", "build"],
    ["release-macos", "build"],
    ["release-windows", "build"],
    ["release-linux", "publish"],
    ["release-macos", "publish"],
    ["release-windows", "publish"],
    ["release-publish", "publish"],
  ];
  for (const [name, overrides, reason] of cases) {
    for (const [recipe, phase] of invocations) {
      it(`${name}: just ${recipe} ${phase} exits before building or uploading`, async () => {
        const out = scratch("release-out");
        try {
          const before = github.requests.length;
          const result = await run("just", [recipe, phase], {
            env: releaseEnv(github, out, overrides),
          });
          assert.equal(result.code, 1, said(result));
          assert.match(result.stderr, reason, said(result));
          assert.match(result.stderr, /^ACTION: /m, "a refusal names what to do next");
          assert.deepEqual(
            github.requests.slice(before),
            [],
            "the refusal came before anything asked GitHub, let alone wrote to it",
          );
          for (const platform of ["linux", "macos", "windows", "common"]) {
            assert.ok(
              !existsSync(join(out, platform, "build.json")),
              "nothing was built for a release that was refused",
            );
          }
        } finally {
          rmSync(out, { recursive: true, force: true });
        }
      });
    }
  }
});

describe("a lock that links an engine twice", () => {
  let github;
  let copy;
  before(async () => {
    github = await new GitHubDouble().start();
    github.addRelease(TAG);
    // The repository's own files with one change: Cargo.lock carries a second,
    // older copy of an engine release.yml's `linked-engines` job reported.
    copy = scratch("release-split");
    for (const path of [
      "scripts",
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

  it("is not refused when the lock links each engine once", async () => {
    const result = await run("bash", ["scripts/linked-engines.sh", "--format", "split"]);
    assert.equal(result.code, 0, said(result));
    assert.match(result.stdout, /linked engines resolve once each/);
  });
});

describe("the release outcome record", () => {
  let github;
  let out;
  before(async () => {
    github = await new GitHubDouble().start();
    github.addRelease(TAG);
    out = scratch("release-outcome");
  });
  after(async () => {
    await github.stop();
    rmSync(out, { recursive: true, force: true });
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

  it("refuses a job result GitHub does not report rather than guessing", async () => {
    const result = await run("just", ["release-outcome"], {
      env: releaseEnv(github, out, { LINUX_RESULT: "succeeded" }),
    });
    assert.equal(result.code, 1, said(result));
    assert.match(result.stderr, /'succeeded' is not a job or step result GitHub reports/);
  });
});

describe("release-plz.yml's auto-merge step", () => {
  const workflow = parse(readFileSync(join(ROOT, ".github/workflows/release-plz.yml"), "utf8"));
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
