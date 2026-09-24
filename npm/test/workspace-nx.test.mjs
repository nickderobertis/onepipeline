// The workspace's Nx wiring, driven rather than read.
//
// Two contracts live in more than one file and cannot reference each other, so
// each gets a gate here:
//
//   1. The uniform target set. `just build` (and its siblings) promise *every
//      project's* artifact, but the promise is spelled once in the justfile, once
//      in `nx.json`'s caching defaults, and once per `project.json`. A project
//      that never declared the target is not an error to Nx — `run-many` simply
//      finds nothing to run there — so the repo-wide verb quietly stops covering
//      it. Only a gate catches that.
//   2. The `ONEPIPELINE_NX_SHOW_OUTPUT` protocol, which `nx-affected.sh` exports
//      and `nx.sh` reads. The two spellings must be one string; they were
//      `ONEAGENTGRAPH_`-prefixed here until the namespace was fixed, and nothing
//      would have failed if only one of them had been renamed.
//
// The journeys below run the real scripts. Nothing is stubbed, and nothing here
// starts a second Nx *inside* the one already running this suite: `scripts/nx.sh`
// truncates the `.logs/nx.log` its caller is writing, so the build targets are
// driven through the commands they declare rather than through `just build`.

import assert from "node:assert/strict";
import { execFileSync, spawnSync } from "node:child_process";
import { existsSync, readFileSync } from "node:fs";
import { basename, dirname, join } from "node:path";
import { describe, it } from "node:test";
import { fileURLToPath } from "node:url";

const root = join(dirname(fileURLToPath(import.meta.url)), "..", "..");

/**
 * Every project's Nx declaration, discovered rather than restated.
 *
 * A restated list is the one thing this file exists to catch going stale: the
 * cases below would then measure Nx against a set nobody updated, and say
 * nothing about whichever project was missing from it.
 *
 * `git ls-files` is the source Nx itself agrees with: a declaration Nx loads is
 * one committed in the tree, so a project added without its `project.json`
 * staged fails here rather than passing and then failing on somebody's clone.
 * The basename filter keeps the pathspec from also matching a `*-project.json`
 * that is not one of these.
 */
const declarations = execFileSync("git", ["ls-files", "--", "*project.json"], {
  cwd: root,
  encoding: "utf8",
})
  .split("\n")
  .filter((path) => basename(path) === "project.json")
  .sort();

const projects = declarations.map((path) => JSON.parse(readFileSync(join(root, path), "utf8")));

function project(name) {
  const found = projects.find((candidate) => candidate.name === name);
  assert.ok(found, `no committed project.json declares \`${name}\``);
  return found;
}

const justfile = readFileSync(join(root, "justfile"), "utf8");
const nxJson = JSON.parse(readFileSync(join(root, "nx.json"), "utf8"));

/**
 * Run a script and capture both streams and the exit code.
 *
 * `spawnSync` rather than `execFileSync` because every case below asserts on a
 * specific exit code — including the ones that fail closed — and a throw would
 * lose the streams that say why.
 *
 * The child inherits this process's environment, so a case whose subject is an
 * *unset* variable needs a way to say so: an `undefined` value here deletes the
 * name instead of passing it through. Without that, such a case asserts about
 * whichever shell launched the gate rather than about the script — a run with
 * `ONEPIPELINE_NX_SHOW_OUTPUT` already exported failed here for that reason.
 */
function run(args, env = {}) {
  const inherited = { ...process.env, ...env };
  for (const [name, value] of Object.entries(env)) {
    if (value === undefined) delete inherited[name];
  }
  const result = spawnSync("bash", args, {
    cwd: root,
    encoding: "utf8",
    env: inherited,
  });
  assert.equal(result.error, undefined, `could not run ${args.join(" ")}`);
  return result;
}

// The verbs whose comment in the justfile promises every project, so a project
// missing one silently drops out of the repo-wide command. `doc` is deliberately
// absent: it is the Rust crate's rustdoc build, and the packaging project has no
// documentation to render.
const UNIFORM_TARGETS = ["bootstrap", "build", "format", "format-check", "lint", "test", "check"];

// The projects that carry none of that set, and are meant not to. `scope:docs`
// is the whole of the exemption: the capture produces documentation images
// rather than an artifact this repository ships or tests, and naming its capture
// `build` would fan `freeze` and `screencomp` — two third-party tools `just
// bootstrap` deliberately does not install — into a verb a clean clone must be
// able to run. Its `project.json` says the same thing as a file-scoped llmlint
// directive; this is that exemption spelled where the gate can see it, so a
// project that declares no targets for any *other* reason still fails below.
const UNIFORM_EXEMPT = ["onepipeline-visual-docs"];

const uniform = projects.filter((declared) => !UNIFORM_EXEMPT.includes(declared.name));

describe("the uniform target set", () => {
  // An exemption that stops being true is worse than no exemption: it would
  // hide a project that had grown half the set and so runs in some repo-wide
  // verbs and not others. Opting out is all-or-nothing, and this is what holds
  // it there.
  for (const name of UNIFORM_EXEMPT) {
    it(`is declined in full by \`${name}\``, () => {
      const declared = project(name);
      const carried = UNIFORM_TARGETS.filter((target) => declared.targets[target]);
      assert.deepEqual(
        carried,
        [],
        `${name} is exempt from the uniform target set but declares ${carried.join(", ")}, so it runs in some repo-wide verbs and not others`,
      );
      assert.ok(
        declared.tags?.includes("scope:docs"),
        `${name} is exempt from the uniform target set, which only a \`scope:docs\` project may be`,
      );
    });
  }

  for (const target of UNIFORM_TARGETS) {
    it(`is declared by every project for \`${target}\``, () => {
      for (const declared of uniform) {
        assert.ok(
          declared.targets[target],
          `${declared.name} declares no \`${target}\` target, so the repo-wide verb skips it`,
        );
      }
    });

    it(`is fanned out of the justfile for \`${target}\``, () => {
      assert.match(
        justfile,
        new RegExp(`scripts/nx\\.sh run-many -t ${target}(\\s|$)`, "m"),
        `no repo-wide recipe fans \`${target}\` across the workspace`,
      );
    });
  }

  // The caching default is the fourth place the target set is spelled. A
  // cacheable target with no default here still runs, but pays full cost every
  // time, which reads as a slow gate rather than as a missing declaration.
  it("gives every cacheable target an nx.json caching default", () => {
    for (const target of UNIFORM_TARGETS) {
      assert.ok(nxJson.targetDefaults[target], `nx.json declares no default for \`${target}\``);
    }
  });
});

describe("the streaming switch", () => {
  // `nx-affected.sh` parses this stdout as JSON, so the wrapper must add nothing
  // to it — not even the summary line it prints for every other caller.
  it("hands Nx's stdout through untouched when it is set", () => {
    const result = run(["scripts/nx.sh", "show", "projects", "--json"], {
      ONEPIPELINE_NX_SHOW_OUTPUT: "1",
    });
    assert.equal(result.status, 0, result.stderr);
    const named = JSON.parse(result.stdout);
    assert.deepEqual(
      named.slice().sort(),
      projects.map((project) => project.name).sort(),
      "the streamed answer is Nx's own project list",
    );
  });

  it("summarises instead when it is unset", () => {
    const result = run(["scripts/nx.sh", "show", "projects", "--json"], {
      ONEPIPELINE_NX_SHOW_OUTPUT: undefined,
    });
    assert.equal(result.status, 0, result.stderr);
    assert.match(result.stdout, /^nx: requested targets succeeded/);
    assert.throws(
      () => JSON.parse(result.stdout),
      "a summarised run must not be mistaken for a parseable answer",
    );
  });

  it("is spelled the same way by the script that sets it and the one that reads it", () => {
    const setter = readFileSync(join(root, "scripts", "nx-affected.sh"), "utf8");
    const reader = readFileSync(join(root, "scripts", "nx.sh"), "utf8");
    // Both halves must be in this repository's own namespace: the pair was
    // inherited under a sibling's prefix, and a rename that missed one file
    // would leave the setter exporting a variable nobody reads.
    for (const [name, text] of [
      ["nx-affected.sh", setter],
      ["nx.sh", reader],
    ]) {
      assert.ok(
        text.includes("ONEPIPELINE_NX_SHOW_OUTPUT"),
        `${name} does not name ONEPIPELINE_NX_SHOW_OUTPUT`,
      );
      assert.ok(
        !/ONEAGENTGRAPH_|ONEVCS_/.test(text),
        `${name} still names a sibling repository's environment namespace`,
      );
    }
  });
});

describe("the affected-selection base override", () => {
  const AFFECTS = ["scripts/nx-affected.sh", "--affects", "onepipeline"];

  it("takes precedence over GITHUB_BASE_REF", () => {
    // The fallback is given a value that cannot survive validation, so the run
    // can only produce a real answer if the override is what was read.
    const result = run(AFFECTS, {
      ONEPIPELINE_NX_BASE_REF: "main",
      GITHUB_BASE_REF: "also bad!",
    });
    assert.equal(result.status, 0, result.stderr);
    assert.doesNotMatch(result.stderr, /is not a usable branch name/);
    assert.doesNotMatch(result.stderr, /no merge base/);
    assert.match(result.stdout.trim(), /^(true|false)$/);
  });

  it("resolves the same base a local run falls back to", () => {
    // `CI` is cleared on both runs because the fallback is branch-specific: only
    // a local run defaults to `main`, and a CI run with no base fails closed
    // instead (covered below). Comparing across that boundary would compare two
    // different documented behaviours.
    const overridden = run(AFFECTS, {
      CI: "",
      GITHUB_BASE_REF: "",
      ONEPIPELINE_NX_BASE_REF: "main",
    });
    const defaulted = run(AFFECTS, { CI: "", GITHUB_BASE_REF: "" });
    assert.equal(overridden.status, 0, overridden.stderr);
    assert.equal(defaulted.status, 0, defaulted.stderr);
    assert.equal(overridden.stdout.trim(), defaulted.stdout.trim());
  });

  it("fails closed on a value that is not a branch name", () => {
    const result = run(AFFECTS, { ONEPIPELINE_NX_BASE_REF: "not a branch!" });
    // Fails *closed*: the caller is told the project is affected, so the gate
    // widens rather than skipping a check it could not scope.
    assert.equal(result.status, 0, result.stderr);
    assert.equal(result.stdout.trim(), "true");
    assert.match(result.stderr, /'not a branch!' is not a usable branch name/);
    assert.match(result.stderr, /treating 'onepipeline' as affected/);
  });

  it("fails closed on a CI build that has no base at all", () => {
    // A push build is *on* the base branch, so there is nothing to scope
    // against. Defaulting to `main` there would find nothing changed and skip
    // every check, which is why this branch refuses the default rather than
    // taking it.
    const result = run(AFFECTS, { CI: "true", GITHUB_BASE_REF: "" });
    assert.equal(result.status, 0, result.stderr);
    assert.equal(result.stdout.trim(), "true");
    assert.match(result.stderr, /this is not a pull-request build/);
    assert.match(result.stderr, /treating 'onepipeline' as affected/);
  });
});

describe("the build targets", () => {
  it("compiles the crate's binary, library, and tests with warnings denied", () => {
    // The crate's own build command, run the way its Nx target runs it. A
    // warning is an error here, so this fails on anything `just check` would
    // reject rather than leaving it for the lint tier.
    execFileSync("just", ["_crate-build"], { cwd: root, encoding: "utf8" });
    const binary = join(root, "target", "debug", "onepipeline");
    assert.ok(
      existsSync(binary) || existsSync(`${binary}.exe`),
      "the crate's build target left no `onepipeline` binary behind",
    );
  });

  it("declares the command each project's build target actually runs", () => {
    const crate = project("onepipeline");
    const packaging = project("onepipeline-npm");
    // The Rust half is what the journey above drives; the npm half is what
    // `launcher.test.mjs` assembles, packs, installs, and executes. Pinning the
    // declarations here is what stops a target from being rewired to something
    // no journey covers.
    assert.match(crate.targets.build.command, /just _crate-build/);
    assert.match(packaging.targets.build.command, /scripts\/npm-build\.mjs launcher/);
  });
});
