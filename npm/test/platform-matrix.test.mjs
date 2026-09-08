// The drift gate for the one platform matrix this repository restates five
// times.
//
// The set of Rust targets a release builds has to agree, exactly, across:
//
//   1. `scripts/npm-build.mjs`'s TARGETS (triple -> platform/arch/exe),
//   2. `npm/onepipeline-cli/bin/onepipeline.js`'s PACKAGES (the launcher's
//      platform-to-package resolution),
//   3. `npm/onepipeline-cli/package.json`'s optionalDependencies (what npm
//      installs), and
//   4. the `upload`, `build-wheels`, and `build-npm` matrices in
//      `.github/workflows/release.yml` (what actually gets built), and
//   5. that file's `verify-npm` matrix (what a release actually *installs and
//      starts* on each platform).
//
// The fifth is not the same question as the fourth, and the gap between them is
// how a platform stayed broken: aarch64 Linux was built and published by every
// release and installed by none of them, so nothing watched the one package
// whose registry lag was visible from 0.16.4 onward. Building a platform nobody
// verifies is a platform nobody is watching.
//
// None can be generated from another — a workflow matrix is YAML a workflow
// engine reads, npm resolves optionalDependencies before any code runs, and the
// launcher must resolve with no build step. So the sets are reconciled here
// instead: add a platform in one place and this fails until it is added in all
// five. Drift here does not break a build; it 404s an install, on the one
// platform nobody tested.

import { readFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { describe, it } from "node:test";
import assert from "node:assert/strict";
import { fileURLToPath } from "node:url";

const REPO_ROOT = resolve(dirname(fileURLToPath(import.meta.url)), "..", "..");

function read(...parts) {
  return readFileSync(join(REPO_ROOT, ...parts), "utf8");
}

function objectLiteral(source, name) {
  const start = source.indexOf(`const ${name} = {`);
  assert.notEqual(start, -1, `no \`const ${name} = {\` in the source`);
  const end = source.indexOf("\n};", start);
  assert.notEqual(end, -1, `\`${name}\` is not terminated by a \`};\` line`);
  return source.slice(start, end);
}

function literalKeys(source, name) {
  return [...objectLiteral(source, name).matchAll(/^\s{2}"([^"]+)":/gm)].map((m) => m[1]);
}

function workflowTargets(workflow, job) {
  const start = workflow.indexOf(`\n  ${job}:\n`);
  assert.notEqual(start, -1, `no \`${job}\` job in release.yml`);
  // Jobs are indented two spaces, so the next line at that indent ends this one.
  const rest = workflow.slice(start + 1);
  const nextJob = rest.slice(1).search(/\n {2}[a-z][a-z0-9-]*:\n/);
  const body = nextJob === -1 ? rest : rest.slice(0, nextJob + 1);
  const targets = [...body.matchAll(/^\s*- target: (\S+)$/gm)].map((m) => m[1]);
  assert.ok(targets.length > 0, `the \`${job}\` job builds no targets`);
  return targets;
}

/// The platform package each `verify-npm` leg exists to prove, read out of that
/// job's own matrix. The `package:` key is what makes a leg an assertion about a
/// platform rather than a runner label — and `release.yml` checks at run time
/// that the leg really did resolve the package named here.
function workflowVerifyPackages(workflow) {
  const start = workflow.indexOf("\n  verify-npm:\n");
  assert.notEqual(start, -1, "no `verify-npm` job in release.yml");
  const rest = workflow.slice(start + 1);
  const nextJob = rest.slice(1).search(/\n {2}[a-z][a-z0-9-]*:\n/);
  const body = nextJob === -1 ? rest : rest.slice(0, nextJob + 1);
  return [...body.matchAll(/^\s*- os: \S+\n\s*package: (\S+)$/gm)].map((m) => m[1]);
}

describe("the platform matrix", () => {
  const buildScript = read("scripts", "npm-build.mjs");
  const launcher = read("npm", "onepipeline-cli", "bin", "onepipeline.js");
  const manifest = JSON.parse(read("npm", "onepipeline-cli", "package.json"));
  const workflow = read(".github", "workflows", "release.yml");

  const triples = literalKeys(buildScript, "TARGETS");
  // `{ platform: "linux", arch: "x64", ... }` for each triple, in the same order.
  const facts = [
    ...objectLiteral(buildScript, "TARGETS").matchAll(/platform: "([^"]+)", arch: "([^"]+)"/g),
  ].map(([, platform, arch]) => ({ platform, arch }));

  it("names the same triples in every release matrix", () => {
    assert.ok(triples.length >= 1, "npm-build.mjs declares no targets");
    for (const job of ["upload", "build-wheels", "build-npm"]) {
      assert.deepEqual(
        workflowTargets(workflow, job).sort(),
        [...triples].sort(),
        `release.yml's \`${job}\` matrix and npm-build.mjs's TARGETS disagree`,
      );
    }
  });

  it("resolves every built target from the launcher", () => {
    assert.equal(facts.length, triples.length, "every target needs platform/arch facts");
    assert.deepEqual(
      literalKeys(launcher, "PACKAGES").sort(),
      facts.map(({ platform, arch }) => `${platform}-${arch}`).sort(),
      "the launcher's PACKAGES keys and npm-build.mjs's TARGETS disagree",
    );
  });

  it("installs exactly the packages the launcher resolves", () => {
    const built = facts.map(({ platform, arch }) => `onepipeline-cli-${platform}-${arch}`).sort();
    assert.deepEqual(
      Object.keys(manifest.optionalDependencies).sort(),
      built,
      "the launcher's optionalDependencies and the packages a release builds disagree",
    );
    const resolved = [...objectLiteral(launcher, "PACKAGES").matchAll(/: "([^"]+)"/g)]
      .map((m) => m[1])
      .sort();
    assert.deepEqual(
      resolved,
      built,
      "the launcher resolves package names a release does not build",
    );
  });

  it("verifies an install on every platform the launcher declares a package for", () => {
    const declared = Object.keys(manifest.optionalDependencies).sort();
    const verified = workflowVerifyPackages(workflow);
    assert.deepEqual(
      [...verified].sort(),
      declared,
      "release.yml's `verify-npm` matrix and the launcher's optionalDependencies disagree — a " +
        "platform with a package and no leg is one no release installs, and a leg for a package " +
        "nothing publishes can only ever be red",
    );
    assert.equal(
      new Set(verified).size,
      verified.length,
      "two `verify-npm` legs name the same platform package, so one platform is unwatched",
    );
  });
});
