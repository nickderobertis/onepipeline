// The release's shape, read out of the workflow files, the recipes' plan mode
// and release-targets.toml together:
//
//   * release.yml is exactly build-linux, build-macos, build-windows and
//     publish, each on the runner its name says with a 30-minute timeout;
//     release-plz.yml is one job; nothing follows the Release on workflow_run;
//   * every artifact the release produces is uploaded by exactly one step of
//     exactly one job, and every declared target and covered package by the
//     jobs its `published_by` names;
//   * each build job test-installs before it publishes and verifies after;
//   * nothing builds, publishes or verifies arm64 Linux or Intel macOS;
//   * each platform's plan is what v0.74.1 published for its targets.
//
// What a step uploads is what its recipe's `plan` says it does: scripts/release.py
// refuses to build a file the plan does not name, and publishes only what it
// built, so the plan is the upload list rather than a description of one.

import { execFileSync } from "node:child_process";
import { existsSync, readFileSync, readdirSync } from "node:fs";
import { join } from "node:path";
import { describe, it } from "node:test";
import assert from "node:assert/strict";
import { parse } from "yaml";

const ROOT = process.cwd();
const WORKFLOWS = join(ROOT, ".github/workflows");
const read = (path) => readFileSync(join(ROOT, path), "utf8");
const release = parse(read(".github/workflows/release.yml"));
const releasePlz = parse(read(".github/workflows/release-plz.yml"));
const VERSION = read("Cargo.toml").match(/^\[package\][\s\S]*?^version = "([^"]+)"/m)[1];
const PLATFORMS = ["linux", "macos", "windows"];
const RUNNERS = { linux: "ubuntu-latest", macos: "macos-latest", windows: "windows-latest" };

const plans = Object.fromEntries(
  [...PLATFORMS, "common"].map((platform) => [
    platform,
    JSON.parse(
      execFileSync(
        "just",
        [platform === "common" ? "release-publish" : `release-${platform}`, "plan"],
        {
          cwd: ROOT,
          encoding: "utf8",
        },
      ),
    ),
  ]),
);

/// The release-targets.toml entries, read with Python's own TOML parser.
const targets = JSON.parse(
  execFileSync(
    "python3",
    [
      "-c",
      "import json,tomllib,sys; print(json.dumps(tomllib.load(open(sys.argv[1],'rb'))))",
      "release-targets.toml",
    ],
    { cwd: ROOT, encoding: "utf8" },
  ),
);

/// What one step uploads, by the recipe it calls: its platform's plan, or
/// `publish`'s, whose outcome record its own step uploads.
function uploadsOf(step) {
  const run = step.run ?? "";
  const found = run.match(/^just release-(linux|macos|windows) publish$/);
  if (found) return plans[found[1]].artifacts;
  if (run === "just release-publish publish") {
    return plans.common.artifacts.filter((a) => a.kind !== "outcome");
  }
  if (run === "just release-outcome")
    return plans.common.artifacts.filter((a) => a.kind === "outcome");
  return [];
}

/// Every upload of every step of every job: [job, step index, artifact].
const uploads = Object.entries(release.jobs).flatMap(([job, spec]) =>
  spec.steps.flatMap((step, index) => uploadsOf(step).map((artifact) => [job, index, artifact])),
);

describe("the CD jobs", () => {
  it("release.yml is exactly the three build jobs and publish, on their own runners", () => {
    assert.deepEqual(Object.keys(release.jobs).sort(), [
      "build-linux",
      "build-macos",
      "build-windows",
      "publish",
    ]);
    for (const platform of PLATFORMS) {
      const job = release.jobs[`build-${platform}`];
      assert.equal(
        job["runs-on"],
        RUNNERS[platform],
        `build-${platform} runs on ${RUNNERS[platform]}`,
      );
      assert.equal(job["timeout-minutes"], 30);
      assert.equal(job.needs, undefined, `build-${platform} needs no other job`);
      assert.equal(job.strategy, undefined, `build-${platform} is one job, not a matrix`);
    }
    assert.equal(release.jobs.publish["runs-on"], "ubuntu-latest");
    assert.equal(release.jobs.publish["timeout-minutes"], 30);
    assert.deepEqual([...release.jobs.publish.needs].sort(), [
      "build-linux",
      "build-macos",
      "build-windows",
    ]);
  });

  it("release-plz.yml is one job, release-plz", () => {
    assert.deepEqual(Object.keys(releasePlz.jobs), ["release-plz"]);
  });

  it("no workflow smokes the Release after it, and published-smoke.yml is gone", () => {
    assert.ok(!existsSync(join(WORKFLOWS, "published-smoke.yml")));
    for (const name of readdirSync(WORKFLOWS)) {
      const on = parse(readFileSync(join(WORKFLOWS, name), "utf8")).on ?? {};
      const followed = on.workflow_run?.workflows ?? [];
      assert.ok(
        !followed.includes(release.name),
        `${name} runs on workflow_run of ${release.name}`,
      );
    }
  });

  it("no CD job runs the repository's check recipe or a test tier", () => {
    for (const [file, workflow] of [
      ["release.yml", release],
      ["release-plz.yml", releasePlz],
    ]) {
      for (const [job, spec] of Object.entries(workflow.jobs)) {
        for (const step of spec.steps) {
          assert.doesNotMatch(
            step.run ?? "",
            /\bjust (check|gate|check-affected|test|test-quick|test-e2e|test-note|smoke-real|release-compat)\b|\bcargo (test|nextest|llvm-cov)\b|\bnx(\.sh)? (run|run-many|affected)\b/,
            `${file} ${job} runs a test tier`,
          );
        }
      }
    }
  });
});

describe("who uploads what", () => {
  it("every artifact the release produces is uploaded by exactly one step of one job", () => {
    const all = Object.values(plans).flatMap((plan) => plan.artifacts);
    assert.ok(all.length > 0);
    for (const artifact of all) {
      const by = uploads.filter(([, , a]) => a === artifact);
      assert.equal(by.length, 1, `${artifact.file} is uploaded by ${by.length} steps`);
    }
    const files = all.map((a) => a.file);
    assert.equal(new Set(files).size, files.length, "no two jobs produce the same file");
    // Each platform's wheel, archive (with its checksum) and npm package.
    for (const platform of PLATFORMS) {
      for (const target of plans[platform].targets) {
        const kinds = plans[platform].artifacts
          .filter((a) => a.target === target)
          .map((a) => a.kind);
        assert.deepEqual(kinds.sort(), ["archive", "checksum", "npm", "wheel"], target);
      }
    }
    assert.deepEqual(plans.common.artifacts.map((a) => a.kind).sort(), ["crate", "npm", "outcome"]);
  });

  it("nothing else in release.yml uploads or publishes", () => {
    for (const [job, spec] of Object.entries(release.jobs)) {
      for (const step of spec.steps) {
        assert.doesNotMatch(
          `${step.uses ?? ""} ${step.run ?? ""}`,
          /upload-artifact|download-artifact|gh-action-pypi-publish|action-gh-release|upload-rust-binary|gh release|npm publish|cargo publish|uv publish|twine|publish-npm\.sh/,
          `${job} uploads outside its recipe`,
        );
      }
    }
  });

  it("every declared target and covered package is uploaded by the jobs its published_by names", () => {
    const jobs = Object.keys(release.jobs);
    for (const target of targets.target) {
      const named = [...target.published_by.matchAll(/`([a-z-]+)`/g)]
        .map((m) => m[1])
        .filter((name) => jobs.includes(name));
      assert.ok(named.length > 0, `${target.id}'s published_by names a release.yml job`);
      const packages = [target.id, ...(target.covers ?? [])];
      const uploaders = new Set();
      for (const id of packages) {
        const by = uploads.filter(([, , a]) => a.package === id).map(([job]) => job);
        assert.ok(by.length > 0, `${id} is uploaded by some job`);
        for (const job of by) {
          assert.ok(
            named.includes(job),
            `${id} is uploaded by ${job}, which ${target.id}'s published_by does not name`,
          );
          uploaders.add(job);
        }
      }
      assert.deepEqual(
        [...uploaders].sort(),
        [...named].sort(),
        `${target.id}: published_by names exactly its uploaders`,
      );
    }
    // A wheel set spread across platforms is each build job together.
    const wheels = uploads
      .filter(([, , a]) => a.package === "pypi:onepipeline-cli")
      .map(([job]) => job);
    assert.deepEqual([...new Set(wheels)].sort(), ["build-linux", "build-macos", "build-windows"]);
    // And nothing is uploaded that the declaration does not name.
    const declared = new Set(targets.target.flatMap((t) => [t.id, ...(t.covers ?? [])]));
    for (const [, , artifact] of uploads) {
      if (artifact.package)
        assert.ok(declared.has(artifact.package), `${artifact.package} is declared`);
    }
  });

  it("publish's upload steps carry only platform-independent targets", () => {
    const published = uploads.filter(([job]) => job === "publish");
    assert.ok(published.length > 0);
    for (const [, , artifact] of published) {
      assert.equal(artifact.target, null, `${artifact.file} is platform-specific`);
    }
  });

  it("each build job test-installs before it publishes, and verifies from the registries after", () => {
    for (const platform of PLATFORMS) {
      const steps = release.jobs[`build-${platform}`].steps.map((s) => s.run ?? "");
      const at = (phase) => {
        const found = steps.flatMap((run, i) =>
          run === `just release-${platform} ${phase}` ? [i] : [],
        );
        assert.equal(found.length, 1, `build-${platform} has one ${phase} step`);
        return found[0];
      };
      assert.ok(at("build") < at("publish"), `build-${platform} test-installs before publishing`);
      assert.ok(at("publish") < at("verify"), `build-${platform} verifies after publishing`);
    }
  });
});

describe("the dropped targets", () => {
  const dropped =
    /aarch64-unknown-linux-gnu|x86_64-apple-darwin|macos-15-intel|macos-13\b|ubuntu-[0-9.]+-arm|-arm64\b.*runs-on|linux-arm64|darwin-x64/;

  it("no workflow or recipe builds, publishes or verifies arm64 Linux or Intel macOS", () => {
    for (const name of readdirSync(WORKFLOWS)) {
      const text = readFileSync(join(WORKFLOWS, name), "utf8");
      assert.doesNotMatch(text, dropped, `${name} names a dropped target or runner`);
      for (const [job, spec] of Object.entries(parse(text).jobs ?? {})) {
        assert.doesNotMatch(
          JSON.stringify(spec["runs-on"] ?? ""),
          /arm|intel|macos-13/,
          `${name} ${job}`,
        );
        for (const leg of spec.strategy?.matrix?.include ?? []) {
          assert.doesNotMatch(
            JSON.stringify(leg),
            /arm|intel|aarch64-unknown-linux/,
            `${name} ${job}`,
          );
        }
      }
    }
    assert.doesNotMatch(read("justfile"), dropped, "a recipe names a dropped target");
    for (const plan of Object.values(plans)) {
      for (const artifact of plan.artifacts) {
        assert.doesNotMatch(
          JSON.stringify(artifact),
          dropped,
          `${artifact.file} is a dropped target`,
        );
      }
    }
  });

  it("release-targets.toml declares no dropped target, and retires both npm packages", () => {
    for (const target of targets.target) {
      assert.doesNotMatch(
        JSON.stringify(target),
        /linux-arm64|darwin-x64|aarch64-unknown-linux|x86_64-apple-darwin/,
      );
    }
    assert.deepEqual(targets.retired.map((r) => r.id).sort(), [
      "npm:onepipeline-cli-darwin-x64",
      "npm:onepipeline-cli-linux-arm64",
    ]);
  });
});

// llmlint: ignore-block[plan_criteria_require_realistic_tests] The macOS and Windows build recipes need native macOS and Windows toolchains this Linux host cannot run, so inside the dispatch their proof is the recipes' plan mode checked against the last release's published artifacts; the Linux recipe is driven for real end to end, and the hosted build jobs build and run the foreign artifacts on every release.
describe("each platform's plan", () => {
  const published = JSON.parse(read(".github/fixtures/release-v0.74.1-published.json"));
  const as = (name) => name.replaceAll(published.version, VERSION);
  const droppedTargets = ["aarch64-unknown-linux-gnu", "x86_64-apple-darwin"];
  const droppedNpm = ["onepipeline-cli-linux-arm64", "onepipeline-cli-darwin-x64"];
  const droppedWheels = ["macosx_10_12_x86_64", "manylinux_2_17_aarch64"];

  for (const platform of PLATFORMS) {
    it(`${platform}: is what v0.74.1 published for ${platform}'s targets, at this version`, () => {
      const plan = plans[platform];
      assert.equal(plan.version, VERSION);
      for (const target of plan.targets) {
        const mine = plan.artifacts.filter((a) => a.target === target);
        const assets = published.release_assets
          .filter((name) => name.includes(`-${target}.`))
          .map(as);
        assert.deepEqual(
          mine
            .filter((a) => a.destination === "github-release")
            .map((a) => a.file)
            .sort(),
          assets.sort(),
          `${target}'s Release assets`,
        );
        const wheel = mine.find((a) => a.kind === "wheel");
        const platformTag = wheel.file.match(/py3-none-(.+)\.whl$/)[1];
        assert.ok(
          published.pypi.map(as).includes(wheel.file),
          `${wheel.file} is a wheel v0.74.1 published, at this version`,
        );
        assert.deepEqual(
          wheel.wheel_tags,
          platformTag.split(".").map((tag) => `py3-none-${tag}`),
          "the wheel's tags are its file name's",
        );
        const npm = mine.find((a) => a.kind === "npm");
        const was = published.npm.find((p) => `npm:${p.name}` === npm.package);
        assert.ok(was, `${npm.package} is a package v0.74.1 published`);
        assert.deepEqual([npm.os, npm.cpu], [was.os[0], was.cpu[0]]);
        assert.equal(npm.file, `${was.name}-${VERSION}.tgz`);
        if (platform === "linux") {
          assert.equal(wheel.file.includes("manylinux_2_17"), true);
          for (const artifact of mine.filter((a) => a.kind !== "checksum")) {
            assert.equal(artifact.glibc_floor, "2.17", `${artifact.file}'s glibc floor`);
          }
        }
      }
    });
  }

  it("together the plans are everything v0.74.1 published but the dropped targets", () => {
    const planned = Object.values(plans).flatMap((p) => p.artifacts);
    const files = new Set(planned.map((a) => a.file));
    for (const name of published.release_assets.map(as)) {
      if (droppedTargets.some((t) => name.includes(t))) {
        assert.ok(!files.has(name), `${name} is dropped`);
      } else {
        assert.ok(files.has(name), `${name} is still published`);
      }
    }
    for (const name of published.pypi.map(as)) {
      assert.equal(files.has(name), !droppedWheels.some((tag) => name.includes(tag)), name);
    }
    const packages = new Set(planned.map((a) => a.package));
    for (const { name } of published.npm) {
      assert.equal(packages.has(`npm:${name}`), !droppedNpm.includes(name), name);
    }
    for (const { name } of published.crates) {
      assert.ok(packages.has(`crate:${name}`), `the ${name} crate is still published`);
      assert.ok(files.has(`${name}-${VERSION}.crate`));
    }
  });
});
// llmlint: ignore-end[plan_criteria_require_realistic_tests]
