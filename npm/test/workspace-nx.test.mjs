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
import {
  appendFileSync,
  copyFileSync,
  cpSync,
  existsSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  rmSync,
  symlinkSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { basename, dirname, join } from "node:path";
import { afterEach, beforeEach, describe, it } from "node:test";
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
function run(args, env = {}, cwd = root) {
  const inherited = { ...process.env, ...env };
  for (const [name, value] of Object.entries(env)) {
    if (value === undefined) delete inherited[name];
  }
  const result = spawnSync("bash", args, {
    cwd,
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

// The live and network tiers (`tier:live`: `onepipeline-smoke` reaches GitHub,
// `onepipeline-release-compat` PyPI) declare every uniform target but `test`.
// Their journey is a target of its own, uncached and reached by no `check`, so
// `just test` and `just check` stay offline and credential-free; a `test` that
// ran it would put the network into both, and one that ran nothing would look
// covered while proving nothing.
const LIVE = projects.filter((declared) => declared.tags?.includes("tier:live"));
const LIVE_TARGETS = {
  "onepipeline-smoke": "smoke",
  "onepipeline-release-compat": "release-compat",
};
const declaresUniform = (declared, target) =>
  target !== "test" || !declared.tags?.includes("tier:live");

// The Rust projects that run a share of the offline suite, each declaring
// `test-quick` so `just check-cross` reaches all of it on the macOS and Windows
// legs: the crate and every offline test tier.
const OFFLINE_RUST = projects.filter(
  (declared) => declared.tags?.includes("lang:rust") && !declared.tags?.includes("tier:live"),
);

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
      for (const declared of uniform.filter((each) => declaresUniform(each, target))) {
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

describe("the live and network tiers", () => {
  it("are the two projects that run outside the offline tier", () => {
    assert.deepEqual(
      LIVE.map((declared) => declared.name).sort(),
      Object.keys(LIVE_TARGETS).sort(),
    );
  });

  for (const [name, target] of Object.entries(LIVE_TARGETS)) {
    it(`runs \`${name}\`'s journey through an uncached target no \`check\` reaches`, () => {
      const declared = project(name);
      assert.equal(
        declared.targets.test,
        undefined,
        `${name} declares a \`test\`, which \`just test\` would run`,
      );
      assert.ok(declared.targets[target], `${name} declares no \`${target}\` target`);
      assert.equal(
        declared.targets[target].cache,
        false,
        `${name}:${target} could replay a live answer`,
      );
      assert.equal(nxJson.targetDefaults[target]?.cache, false, `nx.json caches \`${target}\``);
      for (const owner of projects) {
        const needs = JSON.stringify(owner.targets?.check?.dependsOn ?? []);
        assert.doesNotMatch(
          needs,
          new RegExp(`"${target}"`),
          `${owner.name}:check reaches ${target}`,
        );
      }
    });

    it(`is what the \`${target === "smoke" ? "smoke-real" : target}\` recipe runs`, () => {
      const recipe = target === "smoke" ? "smoke-real" : target;
      const body = justfile.split(`\n${recipe}:\n`)[1]?.split("\n\n")[0] ?? "";
      assert.match(body, new RegExp(`scripts/nx\\.sh run ${name}:${target}(\\s|$)`), body);
      assert.doesNotMatch(body, /cargo /, `\`${recipe}\` reaches cargo outside the graph`);
    });
  }
});

describe("the offline tier split", () => {
  // The target set `onepipeline-note-journeys` declares is the shape every
  // offline test tier copies, so a tier that grew or lost one is caught here.
  const TIER_TARGETS = Object.keys(project("onepipeline-note-journeys").targets).sort();

  for (const name of ["onepipeline-e2e", "onepipeline-contract"]) {
    it(`gives \`${name}\` the note journeys' target set`, () => {
      assert.deepEqual(Object.keys(project(name).targets).sort(), TIER_TARGETS);
    });
  }

  it("declares `test-quick` on every offline Rust project, fanned out by one recipe", () => {
    for (const declared of OFFLINE_RUST) {
      assert.ok(declared.targets["test-quick"], `${declared.name} declares no \`test-quick\``);
    }
    assert.match(justfile, /scripts\/nx\.sh run-many -t test-quick(\s|$)/m);
  });

  it("merges every tier's profiles into the one aggregate the floor is measured on", () => {
    const aggregate = project("onepipeline").targets.test;
    assert.match(aggregate.command, /just _crate-coverage/);
    const waitedOn = aggregate.dependsOn.flatMap((need) =>
      typeof need === "string"
        ? [`onepipeline:${need}`]
        : need.projects.map((name) => `${name}:${need.target}`),
    );
    const tiers = [
      "onepipeline:test-rest",
      ...OFFLINE_RUST.filter((declared) => declared.name !== "onepipeline").map(
        (declared) => `${declared.name}:test`,
      ),
    ];
    assert.deepEqual(waitedOn.sort(), tiers.sort());
    // A tier replayed from the cache restores exactly the profiles it wrote, so
    // the report merges what it would have merged had every tier run.
    for (const id of tiers) {
      const [name, target] = id.split(":");
      const outputs = project(name).targets[target].outputs ?? [];
      assert.ok(
        outputs.some(
          (output) =>
            output.startsWith("{workspaceRoot}/target/llvm-cov-target/") &&
            output.endsWith(".profraw"),
        ),
        `${id} declares none of its profiles as outputs: ${JSON.stringify(outputs)}`,
      );
    }
  });

  it("keeps the e2e and contract binaries out of the crate's own tier", () => {
    const evaluate = (name) =>
      execFileSync("just", ["--evaluate", name], { cwd: root, encoding: "utf8" });
    assert.match(project("onepipeline").targets["test-rest"].command, /just _crate-test-rest/);
    const rest = evaluate("rest-tier");
    for (const binary of ["e2e", "contract", "note", "smoke", "release_channel"]) {
      assert.ok(
        rest.includes(`not binary(${binary})`),
        `rest-tier runs the ${binary} binary: ${rest}`,
      );
    }
  });
});

describe("the Rust targets' cache inputs", () => {
  // Every target that builds, lints or tests the Rust code keys on the doubles
  // the journeys drive and on both configuration files, so a change to any of
  // them runs the target again instead of replaying a result built without it.
  // Asked of Nx itself (`nx show target inputs`), so a named input that resolves
  // differently from how it reads fails here.
  const RUST_TARGETS = [
    "build",
    "format-check",
    "lint",
    "test",
    "test-rest",
    "test-quick",
    "doc",
    "budgets",
    "msrv",
    "deps-check",
    "smoke",
    "release-compat",
  ];
  const REQUIRED = ["crates/testfakes/src/lib.rs", ".cargo/config.toml", ".config/nextest.toml"];

  function inputsOf(id) {
    const result = run(["scripts/nx.sh", "show", "target", "inputs", id, "--json"], {
      ONEPIPELINE_NX_SHOW_OUTPUT: "1",
    });
    assert.equal(result.status, 0, result.stderr);
    return JSON.parse(result.stdout).files;
  }

  for (const declared of projects.filter((each) => each.tags?.includes("lang:rust"))) {
    for (const target of RUST_TARGETS.filter((each) => declared.targets[each])) {
      it(`keys ${declared.name}:${target} on the doubles and both configuration files`, () => {
        const files = inputsOf(`${declared.name}:${target}`);
        for (const file of REQUIRED) {
          assert.ok(files.includes(file), `${declared.name}:${target} does not hash ${file}`);
        }
      });
    }
  }

  it("leaves a file outside a tier's inputs out of its hash", () => {
    // The other half: an input list that named the whole workspace would pass
    // the cases above and replay nothing, ever.
    const files = inputsOf("onepipeline-e2e:test");
    assert.ok(
      !files.includes("npm/test/workspace-nx.test.mjs"),
      "onepipeline-e2e:test hashes the npm suite",
    );
    assert.ok(
      !files.includes("tests/contract.rs"),
      "onepipeline-e2e:test hashes the contract binary",
    );
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
  // Every case here names the base it is about, so none may inherit one from
  // the shell running the suite — least of all a base *commit*, which wins over
  // every branch and which CI's push build exports to the very gate running
  // this file. Inherited, it answered for the branch cases below, which then
  // failed closed on nothing and said so to an empty stderr.
  const affects = (env) =>
    run(AFFECTS, {
      ONEPIPELINE_NX_BASE_SHA: undefined,
      ONEPIPELINE_NX_BASE_REF: undefined,
      ...env,
    });

  it("takes precedence over GITHUB_BASE_REF", () => {
    // The fallback is given a value that cannot survive validation, so the run
    // can only produce a real answer if the override is what was read.
    const result = affects({
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
    const overridden = affects({
      CI: "",
      GITHUB_BASE_REF: "",
      ONEPIPELINE_NX_BASE_REF: "main",
    });
    const defaulted = affects({ CI: "", GITHUB_BASE_REF: "" });
    assert.equal(overridden.status, 0, overridden.stderr);
    assert.equal(defaulted.status, 0, defaulted.stderr);
    assert.equal(overridden.stdout.trim(), defaulted.stdout.trim());
  });

  it("fails closed on a value that is not a branch name", () => {
    const result = affects({ ONEPIPELINE_NX_BASE_REF: "not a branch!" });
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
    const result = affects({ CI: "true", GITHUB_BASE_REF: "" });
    assert.equal(result.status, 0, result.stderr);
    assert.equal(result.stdout.trim(), "true");
    assert.match(result.stderr, /this is not a pull-request build/);
    assert.match(result.stderr, /treating 'onepipeline' as affected/);
  });
});

/**
 * A scratch repository holding this working tree's tracked files, committed,
 * so a journey can make real commits and ask the real scripts what they select.
 * The installed `node_modules` is linked rather than reinstalled: the scratch
 * runs the same pinned Nx this suite does.
 */
function scratchRepository(t) {
  const scratch = mkdtempSync(join(tmpdir(), "onepipeline-nx-affected-"));
  t.after(() => rmSync(scratch, { recursive: true, force: true }));
  const tracked = execFileSync("git", ["ls-files", "-z"], { cwd: root, encoding: "utf8" })
    .split("\0")
    .filter((path) => path && existsSync(join(root, path)));
  for (const path of tracked) cpSync(join(root, path), join(scratch, path), { recursive: true });
  symlinkSync(join(root, "node_modules"), join(scratch, "node_modules"), "junction");
  const git = (...args) =>
    execFileSync(
      "git",
      ["-c", "user.name=scratch", "-c", "user.email=scratch@example.invalid", ...args],
      {
        cwd: scratch,
        encoding: "utf8",
      },
    ).trim();
  git("init", "-q", "-b", "main");
  // No background writer may outlive the command that started it, or the
  // teardown above meets it: git 2.47+ answers a commit with a *detached*
  // auto-maintenance, which on this many loose objects repacks into `.git` for
  // seconds after `commit` returns, and `rmSync` failed `ENOTEMPTY` on the
  // `.git` it was still writing. Set in the repository, so the scripts' own git
  // calls read it too.
  git("config", "maintenance.auto", "false");
  git("config", "gc.auto", "0");
  git("add", "-A");
  git("commit", "-q", "-m", "base");
  return {
    path: scratch,
    head: () => git("rev-parse", "HEAD"),
    change(file) {
      appendFileSync(join(scratch, file), "\n");
      git("commit", "-q", "-am", `change ${file}`);
    },
    /** `--affects` for each project, on a push build whose base is `base`. */
    affects(base, names, env = {}) {
      return Object.fromEntries(
        names.map((name) => {
          const result = run(
            ["scripts/nx-affected.sh", "--affects", name],
            {
              CI: "true",
              GITHUB_BASE_REF: "",
              ONEPIPELINE_NX_BASE_REF: undefined,
              ONEPIPELINE_NX_BASE_SHA: base,
              ...env,
            },
            scratch,
          );
          assert.equal(result.status, 0, result.stderr);
          return [name, result.stdout.trim()];
        }),
      );
    },
  };
}

describe("affected selection over real commits", () => {
  const TIERS = [
    "onepipeline",
    "onepipeline-e2e",
    "onepipeline-contract",
    "onepipeline-note-journeys",
  ];

  it("selects a test tier by the files its tests read, and the crate with it", (t) => {
    const repo = scratchRepository(t);
    const base = repo.head();
    repo.change("tests/e2e/adoption.rs");
    assert.deepEqual(repo.affects(base, TIERS), {
      onepipeline: "true",
      "onepipeline-e2e": "true",
      "onepipeline-contract": "false",
      "onepipeline-note-journeys": "false",
    });

    const before = repo.head();
    repo.change("docs/contract.md");
    assert.equal(repo.affects(before, ["onepipeline-contract"])["onepipeline-contract"], "true");
  });

  it("answers `true` for the crate on a change only a test tier owns", (t) => {
    // `changes` gates `cross`, `msrv`, `deny`, `install` and `wheel` on this
    // answer, so a test-only change must still reach them.
    const repo = scratchRepository(t);
    const base = repo.head();
    repo.change("tests/note/main.rs");
    assert.deepEqual(repo.affects(base, ["onepipeline", "onepipeline-note-journeys"]), {
      onepipeline: "true",
      "onepipeline-note-journeys": "true",
    });
  });

  it("scopes a push build to the commit before the push", (t) => {
    // The push changed nothing the contract tier reads, so a scoped answer is
    // `false`; the same build with no base fails closed to `true` and says why.
    // (The crate's own project is no witness here: some of its targets —
    // `lint-llm-diff` among them — hash the whole workspace, so every change
    // reaches it.)
    const repo = scratchRepository(t);
    const before = repo.head();
    repo.change("tests/e2e/adoption.rs");
    assert.equal(repo.affects(before, ["onepipeline-contract"])["onepipeline-contract"], "false");

    const unscoped = run(
      ["scripts/nx-affected.sh", "--affects", "onepipeline-contract"],
      {
        CI: "true",
        GITHUB_BASE_REF: "",
        ONEPIPELINE_NX_BASE_REF: undefined,
        ONEPIPELINE_NX_BASE_SHA: undefined,
      },
      repo.path,
    );
    assert.equal(unscoped.status, 0, unscoped.stderr);
    assert.equal(unscoped.stdout.trim(), "true");
    assert.match(unscoped.stderr, /ONEPIPELINE_NX_BASE_SHA names no base commit/);
    assert.match(unscoped.stderr, /treating 'onepipeline-contract' as affected/);
  });

  it("prefers the base commit to the base branch", (t) => {
    // The branch cannot survive validation, so only a run that read the commit
    // can give a scoped answer.
    const repo = scratchRepository(t);
    const before = repo.head();
    repo.change("tests/e2e/adoption.rs");
    const result = run(
      ["scripts/nx-affected.sh", "--affects", "onepipeline-contract"],
      {
        CI: "true",
        GITHUB_BASE_REF: "",
        ONEPIPELINE_NX_BASE_REF: "not a branch!",
        ONEPIPELINE_NX_BASE_SHA: before,
      },
      repo.path,
    );
    assert.equal(result.status, 0, result.stderr);
    assert.equal(result.stdout.trim(), "false");
    assert.doesNotMatch(result.stderr, /not a usable branch name/);
  });

  it("runs the targets of the affected projects only, and every project without a base", (t) => {
    // The path `just check-affected` takes, through the real Nx: only `just` is
    // a double, recording which recipe each project's `format-check` ran.
    const repo = scratchRepository(t);
    mkdirSync(join(repo.path, "bin"));
    writeFileSync(
      join(repo.path, "bin", "just"),
      '#!/usr/bin/env bash\necho "$*" >> "$JUST_CALLS"\n',
      {
        mode: 0o755,
      },
    );
    const calls = join(repo.path, "just.calls");
    const before = repo.head();
    repo.change("tests/e2e/adoption.rs");
    const checked = (sha) => {
      writeFileSync(calls, "");
      const result = run(
        [
          "scripts/nx-affected.sh",
          "-t",
          "format-check",
          "--exclude",
          "onepipeline-npm",
          "--skip-nx-cache",
        ],
        {
          CI: "true",
          GITHUB_BASE_REF: "",
          ONEPIPELINE_NX_BASE_REF: undefined,
          ONEPIPELINE_NX_BASE_SHA: sha,
          PATH: `${join(repo.path, "bin")}:${process.env.PATH}`,
          JUST_CALLS: calls,
        },
        repo.path,
      );
      assert.equal(result.status, 0, result.stderr);
      return {
        ran: readFileSync(calls, "utf8").split("\n").filter(Boolean),
        stderr: result.stderr,
      };
    };

    const scoped = checked(before);
    assert.ok(scoped.ran.includes("_tier-fmt-check tests/e2e/main.rs"), scoped.ran.join(" | "));
    assert.ok(!scoped.ran.includes("_tier-fmt-check tests/contract.rs"), scoped.ran.join(" | "));
    assert.doesNotMatch(scoped.stderr, /running every project/);

    for (const sha of ["0000000000000000000000000000000000000000", undefined]) {
      const unscoped = checked(sha);
      assert.ok(
        unscoped.ran.includes("_tier-fmt-check tests/e2e/main.rs"),
        unscoped.ran.join(" | "),
      );
      assert.ok(
        unscoped.ran.includes("_tier-fmt-check tests/contract.rs"),
        unscoped.ran.join(" | "),
      );
      assert.match(unscoped.stderr, /ONEPIPELINE_NX_BASE_SHA/);
      assert.match(unscoped.stderr, /running every project instead of the affected ones/);
    }
  });

  for (const [why, sha] of [
    ["names no commit this checkout has", "0123456789abcdef0123456789abcdef01234567"],
    ["is not a commit SHA", "main; rm -rf /"],
  ]) {
    it(`fails closed on a base commit that ${why}`, (t) => {
      // GitHub reports forty zeros for a branch's first push, and a force-push
      // can name a commit no fetch brought in: neither is guessed past.
      const repo = scratchRepository(t);
      const result = run(
        ["scripts/nx-affected.sh", "--affects", "onepipeline"],
        {
          CI: "true",
          GITHUB_BASE_REF: "",
          ONEPIPELINE_NX_BASE_REF: "main",
          ONEPIPELINE_NX_BASE_SHA: sha,
        },
        repo.path,
      );
      assert.equal(result.status, 0, result.stderr);
      assert.equal(result.stdout.trim(), "true");
      assert.match(result.stderr, /ONEPIPELINE_NX_BASE_SHA=/);
      assert.match(result.stderr, /treating 'onepipeline' as affected/);
    });
  }
});

describe("a tier replayed from the cache", () => {
  // The floor is one report over every tier's profiles, and `coverage-clean`
  // empties the tree those profiles sit in before any tier runs. So a tier Nx
  // replays has to put its profiles back, or the aggregate measures the suite
  // without it. Driven through the real Nx and the committed project graph in a
  // scratch repository; only `just` is a double, standing in for the
  // instrumented runs: each tier recipe writes one profile under the name the
  // real `_tier-test` gives it, and the aggregate lists what it would merge.
  const DOUBLE = `#!/usr/bin/env bash
echo "$1" >> "$JUST_CALLS"
tree=target/llvm-cov-target
case "$1" in
  _crate-coverage-clean) rm -rf "$tree" ;;
  _crate-test-archive) ;;
  _crate-coverage) ls "$tree" > "$JUST_SAW" ;;
  *)
    tier="$(sed -n "s/^$1:.*(_tier-test \\"\\([a-z0-9_]*\\)\\".*/\\1/p" justfile)"
    [ -n "$tier" ] || { echo "just double: '$1' runs no tier" >&2; exit 1; }
    name="$(sed -n 's/.*LLVM_PROFILE_FILE_NAME="\\([^"]*\\)".*/\\1/p' justfile)"
    name="\${name//\\$1/$tier}"; name="\${name//%p/$$}"; name="\${name//%14m/0}"
    mkdir -p "$tree" && echo profile > "$tree/$name"
    if [ "$tier" = e2e ]; then
      mkdir -p target/budget-records && echo '{}' > target/budget-records/stacked-spikes-stage.json
    fi
    ;;
esac
`;
  const TIER_RECIPES = ["_crate-test-rest", "_e2e-test", "_contract-test", "_note-test"];

  it("restores its profiles for the aggregate to merge beside the tier that ran", (t) => {
    const repo = scratchRepository(t);
    mkdirSync(join(repo.path, "bin"));
    writeFileSync(join(repo.path, "bin", "just"), DOUBLE, { mode: 0o755 });
    const calls = join(repo.path, "just.calls");
    const saw = join(repo.path, "aggregate.saw");
    const aggregate = () => {
      writeFileSync(calls, "");
      const result = run(
        ["scripts/nx.sh", "run", "onepipeline:test"],
        {
          PATH: `${join(repo.path, "bin")}:${process.env.PATH}`,
          JUST_CALLS: calls,
          JUST_SAW: saw,
        },
        repo.path,
      );
      assert.equal(result.status, 0, result.stderr);
      return {
        ran: readFileSync(calls, "utf8").split("\n").filter(Boolean),
        merged: readFileSync(saw, "utf8").split("\n").filter(Boolean),
      };
    };
    const tiersIn = (merged) =>
      merged.map((profile) => profile.match(/^onepipeline-([a-z0-9_]+)-/)?.[1]).sort();

    const cold = aggregate();
    for (const recipe of TIER_RECIPES) assert.ok(cold.ran.includes(recipe), cold.ran.join(" "));
    assert.deepEqual(tiersIn(cold.merged), ["contract", "e2e", "note", "rest"]);

    // Only the note tier's inputs change; the budget record the e2e tier
    // writes is gone, as it is on a fresh runner.
    rmSync(join(repo.path, "target", "budget-records"), { recursive: true, force: true });
    repo.change("tests/note/main.rs");
    const warm = aggregate();
    assert.deepEqual(
      warm.ran.filter((recipe) => TIER_RECIPES.includes(recipe)),
      ["_note-test"],
      `only the changed tier runs: ${warm.ran.join(" ")}`,
    );
    assert.ok(warm.ran.includes("_crate-coverage-clean"), warm.ran.join(" "));
    assert.deepEqual(tiersIn(warm.merged), ["contract", "e2e", "note", "rest"]);
    assert.ok(
      existsSync(join(repo.path, "target", "budget-records", "stacked-spikes-stage.json")),
      "the replayed e2e tier did not restore the budget record `budgets` reads",
    );
  });

  it("runs again after a change to the doubles or either configuration file", (t) => {
    // The other half of the cache inputs above, asked of a warm cache rather
    // than of `nx show`: each of these files changes what the tier's binary
    // does, so a replay after it would report a run nobody made. A file outside
    // the tier's inputs is the control — it replays, so the cases that run again
    // are not running because nothing ever replays.
    const repo = scratchRepository(t);
    mkdirSync(join(repo.path, "bin"));
    writeFileSync(join(repo.path, "bin", "just"), DOUBLE, { mode: 0o755 });
    const calls = join(repo.path, "just.calls");
    const tierRan = () => {
      writeFileSync(calls, "");
      const result = run(
        ["scripts/nx.sh", "run", "onepipeline-contract:test"],
        {
          PATH: `${join(repo.path, "bin")}:${process.env.PATH}`,
          JUST_CALLS: calls,
          JUST_SAW: join(repo.path, "aggregate.saw"),
        },
        repo.path,
      );
      assert.equal(result.status, 0, result.stderr);
      return readFileSync(calls, "utf8").split("\n").includes("_contract-test");
    };

    assert.ok(tierRan(), "a cold cache did not run the tier");
    assert.ok(!tierRan(), "an unchanged tree did not replay the tier");
    for (const file of [
      "crates/testfakes/src/lib.rs",
      ".cargo/config.toml",
      ".config/nextest.toml",
    ]) {
      repo.change(file);
      assert.ok(tierRan(), `a change to ${file} replayed the tier from the cache`);
    }
    repo.change("npm/test/workspace-nx.test.mjs");
    assert.ok(!tierRan(), "a change outside the tier's inputs ran it again");
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

describe("the locked install", () => {
  // A tree installed before `@onebudgetspec/sdk` was pinned still has both
  // shims, and the stage budget's command would fail importing the SDK. The
  // wrapper runs from a scratch copy whose `npm` is a double that records the
  // call, so the real tree's `node_modules` is never reinstalled under the suite.
  const bash = execFileSync("bash", ["-c", "command -v bash"], { encoding: "utf8" }).trim();
  const dirnameTool = execFileSync("bash", ["-c", "command -v dirname"], {
    encoding: "utf8",
  }).trim();
  let scratch;

  beforeEach(() => {
    scratch = mkdtempSync(join(tmpdir(), "nx-locked-install-"));
    mkdirSync(join(scratch, "scripts"));
    copyFileSync(join(root, "scripts", "nx.sh"), join(scratch, "scripts", "nx.sh"));
    mkdirSync(join(scratch, "node_modules", ".bin"), { recursive: true });
    for (const shim of ["nx", "onemessagebus"]) {
      writeFileSync(join(scratch, "node_modules", ".bin", shim), `#!/bin/sh\necho "${shim} $*"\n`, {
        mode: 0o755,
      });
    }
    mkdirSync(join(scratch, "bin"));
    symlinkSync(dirnameTool, join(scratch, "bin", "dirname"));
  });

  afterEach(() => rmSync(scratch, { recursive: true, force: true }));

  function withNpm(status) {
    writeFileSync(
      join(scratch, "bin", "npm"),
      `#!${bash}\necho "$*" >> npm.calls\nmkdir -p node_modules/@onebudgetspec/sdk\necho '{}' > node_modules/@onebudgetspec/sdk/package.json\nexit ${status}\n`,
      { mode: 0o755 },
    );
  }

  function wrapper() {
    return spawnSync(bash, [join(scratch, "scripts", "nx.sh"), "show", "projects"], {
      encoding: "utf8",
      env: { PATH: join(scratch, "bin"), ONEPIPELINE_NX_SHOW_OUTPUT: "1" },
    });
  }

  const calls = () =>
    existsSync(join(scratch, "npm.calls")) ? readFileSync(join(scratch, "npm.calls"), "utf8") : "";

  it("reinstalls a tree that is missing only the onebudgetspec SDK", () => {
    withNpm(0);
    const result = wrapper();
    assert.equal(result.status, 0, result.stderr);
    assert.match(calls(), /^ci /);
    assert.equal(result.stdout, "nx show projects\n");
  });

  it("leaves a tree that already carries the SDK alone", () => {
    withNpm(0);
    mkdirSync(join(scratch, "node_modules", "@onebudgetspec", "sdk"), { recursive: true });
    writeFileSync(join(scratch, "node_modules", "@onebudgetspec", "sdk", "package.json"), "{}");
    const result = wrapper();
    assert.equal(result.status, 0, result.stderr);
    assert.equal(calls(), "");
  });

  it("fails naming the locked install when it cannot complete", () => {
    withNpm(1);
    const result = wrapper();
    assert.equal(result.status, 1);
    assert.match(result.stderr, /nx: 'npm ci' failed in /);
    assert.equal(result.stdout, "");
  });

  it("names the SDK among what it cannot install without npm", () => {
    const result = wrapper();
    assert.equal(result.status, 1);
    assert.match(
      result.stderr,
      /npm not found;.*the pinned onebudgetspec SDK the stage budget reports through/,
    );
  });
});
