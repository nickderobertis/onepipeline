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
import { evaluate } from "../workflow-graph.mjs";
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
  const workflow = parse(readFileSync(join(ROOT, ".github/workflows/release.yml"), "utf8"));
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

  it("fails visibly when GitHub does not answer at all", async () => {
    const github = await new GitHubDouble().start();
    const env = github.env();
    await github.stop();
    const result = await run("bash", ["-eo", "pipefail", "-c", step.run], { env });
    assert.equal(result.code, 1, said(result));
    assert.match(result.stderr, /GitHub did not answer at http:\/\/127\.0\.0\.1/);
  });

  it("fails visibly on a pull request list that is not one", async () => {
    const github = await new GitHubDouble().start();
    try {
      for (const body of [
        '{"message":"an object"}',
        "not json",
        '[{"number":"12","node_id":7,"head":{"ref":"release-plz-a"}}]',
        '[{"number":12,"node_id":"PR_node_12","head":{"ref":"release-plz-a","repo":{"full_name":5}}}]',
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
