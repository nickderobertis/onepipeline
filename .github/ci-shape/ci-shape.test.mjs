// What ci.yml's four jobs promise, read off the workflow files and nothing else
// (this target's inputs are `.github/` alone):
//
// - `lint`, `test-linux`, `test-windows` and `test-macos`, by those ids, unnamed,
//   on the runners their names say, each bounded at 30 minutes, each starting on
//   every pull request and none waiting on another job;
// - the visual docs captured and judged as steps of `lint`, through screencomp's
//   composite actions, and never through its reusable workflow from ci.yml;
// - the release pull request's shortcut asked by the three test jobs, gating
//   their work, and never by `lint`;
// - every job behind a context branch protection requires today still defined;
// - every cache a workflow restores keyed on what produced it, pull requests
//   restoring Rust caches without saving them, and `rust-cache-warm.yml` saving
//   from main under exactly the keys the test jobs restore.
import assert from "node:assert/strict";
import { readdirSync, readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { describe, it } from "node:test";
import { fileURLToPath } from "node:url";
import { parse } from "yaml";

const github = join(dirname(fileURLToPath(import.meta.url)), "..");
const workflows = Object.fromEntries(
  readdirSync(join(github, "workflows"))
    .filter((name) => name.endsWith(".yml"))
    .map((name) => [name, parse(readFileSync(join(github, "workflows", name), "utf8"))]),
);
const ci = workflows["ci.yml"];
const warm = workflows["rust-cache-warm.yml"];

const FOUR = {
  lint: "ubuntu-latest",
  "test-linux": "ubuntu-latest",
  "test-windows": "windows-latest",
  "test-macos": "macos-latest",
};
// Each job's bound in minutes: `lint` at the 30 the four were planned to, and
// each test job at its measured hosted duration with headroom, as ruled.
const TIMEOUT = { lint: 30, "test-linux": 180, "test-windows": 270, "test-macos": 210 };
const ON_PULL_REQUESTS = "github.event_name == 'pull_request'";
// test-macos alone also skips release-plz's own release pull request.
const CONDITION = {
  lint: ON_PULL_REQUESTS,
  "test-linux": ON_PULL_REQUESTS,
  "test-windows": ON_PULL_REQUESTS,
  "test-macos": `${ON_PULL_REQUESTS} && !(startsWith(github.head_ref, 'release-plz-') && github.event.pull_request.head.repo.full_name == github.repository)`,
};
const TESTS = ["test-linux", "test-windows", "test-macos"];

// The contexts branch protection requires, from the tree's one copy of them
// (`required-contexts.json`, which tests/ci_workflows.rs reads too); the jobs
// behind them stay until protection moves to the four.
const REQUIRED = JSON.parse(readFileSync(join(github, "required-contexts.json"), "utf8")).contexts;
const REUSABLE_VISUAL_DOCS = "nickderobertis/screencomp/.github/workflows/visual-docs-reusable.yml";
const COMPOSITES = [
  "nickderobertis/screencomp@",
  "nickderobertis/screencomp/visual-docs@",
  "nickderobertis/screencomp/visual-docs-pages@",
  "nickderobertis/screencomp/visual-docs-aggregate@",
];

const steps = (job) => job.steps ?? [];
const uses = (step) => (typeof step.uses === "string" ? step.uses : "");
const runs = (step) => (typeof step.run === "string" ? step.run : "");
const needs = (job) => (job.needs === undefined ? [] : [job.needs].flat());

function pullRequestTrigger(workflow) {
  const on = workflow.on ?? workflow[true];
  return on && typeof on === "object" ? on.pull_request : undefined;
}

// A job's rendered names: its `name:` or its id, each matrix leg's values in
// parentheses when unnamed, as GitHub renders a check.
function rendered(id, job) {
  const matrix = job.strategy?.matrix;
  let legs = [{}];
  if (matrix?.include) legs = matrix.include;
  else if (matrix) {
    for (const [axis, values] of Object.entries(matrix)) {
      legs = legs.flatMap((leg) => values.map((value) => ({ ...leg, [axis]: String(value) })));
    }
  }
  return legs.map((leg) => {
    if (typeof job.name === "string") {
      return Object.entries(leg).reduce(
        (name, [axis, value]) => name.replace(`\${{ matrix.${axis} }}`, value),
        job.name,
      );
    }
    const values = Object.values(leg);
    return values.length === 0 ? id : `${id} (${values.join(", ")})`;
  });
}

describe("ci.yml's four jobs", () => {
  it("run on every pull request, with no filter on the trigger", () => {
    const trigger = pullRequestTrigger(ci);
    assert.notEqual(trigger, undefined, "ci.yml has no pull_request trigger");
    for (const filter of ["paths", "paths-ignore", "branches", "branches-ignore"]) {
      assert.equal(trigger?.[filter], undefined, `ci.yml filters pull requests by ${filter}`);
    }
    const types = trigger?.types;
    if (types) {
      for (const kind of ["opened", "synchronize", "reopened"]) {
        assert.ok(types.includes(kind), `ci.yml's pull_request trigger leaves out ${kind}`);
      }
    }
  });

  for (const [id, runner] of Object.entries(FOUR)) {
    it(`define \`${id}\`: unnamed, on ${runner}, bounded at ${TIMEOUT[id]} minutes, waiting on nothing`, () => {
      const job = ci.jobs?.[id];
      assert.ok(job, `ci.yml defines no \`${id}\` job`);
      assert.equal(job.name, undefined, `\`${id}\` overrides its name, which renames its check`);
      assert.equal(job["runs-on"], runner);
      assert.equal(
        job["timeout-minutes"],
        TIMEOUT[id],
        `\`${id}\` is not bounded at ${TIMEOUT[id]} minutes`,
      );
      assert.deepEqual(needs(job), [], `\`${id}\` waits on ${needs(job).join(", ")}`);
      assert.equal(job.strategy, undefined, `\`${id}\` is a matrix, which renames its check`);
      assert.equal(job.if, CONDITION[id], `\`${id}\` runs on a condition nobody agreed`);
    });
  }

  it("run each job's checks through its own recipe", () => {
    for (const [id, recipe] of Object.entries({
      lint: "just ci-lint",
      "test-linux": "just ci-test-linux",
      "test-windows": "just ci-test-windows",
      "test-macos": "just ci-test-macos",
    })) {
      const calls = steps(ci.jobs[id]).filter((step) => runs(step).includes(recipe));
      assert.equal(calls.length, 1, `\`${id}\` calls ${recipe} ${calls.length} times, not once`);
    }
  });
});

describe("the judged lint", () => {
  it("installs in `lint` exactly the harnesses the `llmlint` job pins", () => {
    // The two run one judge until the old job is retired; the harness pins are
    // what an upstream release has broken a required check through before.
    const installs = (job) =>
      steps(job)
        .map(runs)
        .filter((run) => /^npm install -g /.test(run))
        .sort();
    assert.ok(installs(ci.jobs.llmlint).length > 0, "the llmlint job installs no harness");
    assert.deepEqual(installs(ci.jobs.lint), installs(ci.jobs.llmlint));
  });
});

describe("the visual docs", () => {
  it("are captured, judged and given every publishing step as steps of `lint`", () => {
    const lint = steps(ci.jobs.lint);
    const captures = lint.filter((step) =>
      runs(step).startsWith("bash screenshots/ci-capture-lanes.sh start "),
    );
    assert.equal(captures.length, 1, "`lint` does not start the capture exactly once");
    assert.deepEqual(
      String(captures[0].env?.SHOTS_LANES).split(" "),
      ["shots/current/x86_64", "shots/verify/x86_64"],
      "`lint` does not capture both lanes the reproducibility gate compares",
    );
    const started = lint.indexOf(captures[0]);
    const judged = lint.findIndex((step) =>
      uses(step).startsWith("nickderobertis/screencomp/visual-docs@"),
    );
    const waited = lint.findIndex((step) =>
      runs(step).startsWith("bash screenshots/ci-capture-lanes.sh wait "),
    );
    assert.ok(
      started < waited && waited < judged,
      "`lint` judges the capture before waiting for it",
    );
    for (const action of COMPOSITES) {
      assert.ok(
        lint.some((step) => uses(step).startsWith(action)),
        `\`lint\` does not run ${action}`,
      );
    }
    for (const [id, job] of Object.entries(ci.jobs)) {
      if (id === "lint") continue;
      assert.ok(
        !steps(job).some((step) => runs(step).includes("screenshots/ci-capture")),
        `ci.yml's \`${id}\` captures the visual docs, which is \`lint\`'s`,
      );
    }
  });

  it("never run through screencomp's reusable workflow from ci.yml", () => {
    for (const [id, job] of Object.entries(ci.jobs)) {
      assert.ok(
        !String(job.uses ?? "").startsWith(REUSABLE_VISUAL_DOCS),
        `ci.yml's \`${id}\` calls the reusable workflow, which adds five jobs`,
      );
    }
  });

  it("are judged with the inputs visual-docs.yml's reusable workflow gives its one lane", () => {
    // The lane branch protection names, `(<arch>, <current>, <verify>, <title>)`,
    // and the inputs visual-docs.yml sets over the reusable workflow's defaults
    // (the snapshot in fixtures/, pinned with it): what the composite steps
    // pass must be what that workflow would, so the two paths judge alike.
    const lane = REQUIRED.find((context) => context.startsWith("visual-docs / report ("));
    const [arch, current, verify, title] = /\((.+)\)$/.exec(lane)[1].split(", ");
    const reusable = parse(
      readFileSync(join(github, "fixtures", "visual-docs-reusable.yml"), "utf8"),
    );
    const defaults = (reusable.on ?? reusable[true]).workflow_call.inputs;
    const caller = workflows["visual-docs.yml"].jobs["visual-docs"].with;
    const effective = (input) => String(caller[input] ?? defaults[input].default);
    const lint = steps(ci.jobs.lint);
    const classify = lint.find((step) =>
      uses(step).startsWith("nickderobertis/screencomp/visual-docs@"),
    ).with;
    assert.deepEqual(
      {
        arch: classify.arch,
        current: classify.current,
        verify: classify["verify-second"],
        title: classify["gallery-title"],
        pages: classify.pages,
        drift: classify["fail-on-drift"],
        manifest: classify["update-manifest"],
      },
      {
        arch,
        current,
        verify,
        title,
        pages: effective("pages"),
        drift: effective("fail-on-drift"),
        manifest: effective("update-manifest"),
      },
    );
    const aggregate = lint.find((step) =>
      uses(step).startsWith("nickderobertis/screencomp/visual-docs-aggregate@"),
    ).with;
    assert.deepEqual(JSON.parse(aggregate.matrix), [
      {
        arch,
        project: classify.project,
        current,
        verify,
        manifest: classify.manifest,
        gallery_title: title,
      },
    ]);
    const capture = lint.find((step) => step.env?.SHOTS_LANES !== undefined);
    assert.deepEqual(capture.env.SHOTS_LANES.split(" "), [
      `${current}/${arch}`,
      `${verify}/${arch}`,
    ]);
  });

  it("are judged by the screencomp release visual-docs.yml pins", () => {
    const pinned = workflows["visual-docs.yml"].jobs["visual-docs"].with["screencomp-version"];
    const installed = steps(ci.jobs.lint).find((step) =>
      uses(step).startsWith("nickderobertis/screencomp@"),
    );
    assert.equal(installed?.with?.version, pinned);
  });
});

describe("the release pull request's shortcut", () => {
  for (const id of TESTS) {
    it(`is asked first by \`${id}\`, and gates every step after it`, () => {
      const all = steps(ci.jobs[id]);
      const at = all.findIndex((step) => step.id === "shortcut");
      assert.ok(at >= 0, `\`${id}\` does not ask the shortcut`);
      assert.match(runs(all[at]), /^node scripts\/release-pr-shortcut\.mjs /);
      // Only the checkout, and a check that a credential is present, which a
      // shortcut must not let a run pass without.
      const before = all.slice(0, at);
      assert.ok(
        before.every(
          (step) =>
            uses(step).startsWith("actions/checkout@") ||
            (uses(step) === "" && Object.keys(step.env ?? {}).every((name) => name.startsWith("HAS_"))),
        ),
        `\`${id}\` does work before asking whether it is owed`,
      );
      for (const step of all.slice(at + 1)) {
        // The verdict a carrier reads is recorded on a shortcut too: the job
        // succeeded, having proven its tests already passed.
        const gated =
          String(step.if ?? "").includes("steps.shortcut.outputs.skip != 'true'") ||
          String(step.if ?? "").includes("steps.gate.outcome == 'failure'") ||
          (step.id === "proved" && step.if === undefined);
        assert.ok(gated, `\`${id}\`'s "${step.name ?? uses(step)}" runs on a shortcut`);
      }
    });
  }

  it("is never asked by `lint`", () => {
    for (const step of steps(ci.jobs.lint)) {
      assert.ok(!runs(step).includes("release-pr-shortcut"), "`lint` asks the shortcut");
      assert.ok(!String(step.if ?? "").includes("shortcut"), "`lint` reads the shortcut");
    }
  });
});

describe("today's required contexts", () => {
  it("are each still reported by a job of a workflow that runs on pull requests", () => {
    const reported = new Set();
    for (const workflow of Object.values(workflows)) {
      if (pullRequestTrigger(workflow) === undefined) continue;
      for (const [id, job] of Object.entries(workflow.jobs ?? {})) {
        for (const name of rendered(id, job)) reported.add(name);
      }
    }
    const visual = workflows["visual-docs.yml"]?.jobs?.["visual-docs"];
    for (const context of REQUIRED) {
      if (context.startsWith("visual-docs / ")) {
        assert.ok(
          String(visual?.uses ?? "").startsWith(REUSABLE_VISUAL_DOCS),
          `no visual-docs job reports \`${context}\``,
        );
        continue;
      }
      assert.ok(reported.has(context), `no job reports \`${context}\` on a pull request`);
    }
  });
});

// What each cache path is built from, and so what its key must name. A path
// not listed here is a cache nobody has said the inputs of.
const CACHE_INPUTS = [
  {
    path: /(^|\/)target\/tools\n~\/\.cargo\/bin\/cargo-prune-superseded\*$/,
    key: ["runner.os", "runner.arch", "steps.pins.outputs.tools", "hashFiles('rust-toolchain.toml')"],
  },
  // Anything else cargo builds or fetches: the lockfile and the toolchain.
  {
    path: /(^|\/)(target|\.cargo)(\/|$)/,
    key: ["runner.os", "runner.arch", "Cargo.lock", "rust-toolchain.toml"],
  },
];

function workflowSteps() {
  const found = [];
  for (const [file, workflow] of Object.entries(workflows)) {
    for (const [id, job] of Object.entries(workflow.jobs ?? {})) {
      for (const step of steps(job)) found.push({ file, id, job, step });
    }
  }
  return found;
}

describe("the caches", () => {
  it("are each keyed on everything that produced them", () => {
    for (const { file, id, step } of workflowSteps()) {
      if (!uses(step).startsWith("actions/cache")) continue;
      const path = String(step.with?.path ?? "").trim();
      const inputs = CACHE_INPUTS.find((entry) => entry.path.test(path));
      assert.ok(inputs, `${file} \`${id}\` caches ${path}, whose inputs nobody has declared`);
      for (const part of inputs.key) {
        assert.ok(
          String(step.with?.key ?? "").includes(part),
          `${file} \`${id}\` keys ${path} without ${part}: ${step.with?.key}`,
        );
      }
    }
  });

  it("restore Rust builds through a rust-cache release that keys on the lockfile and toolchain", () => {
    // rust-cache's key is the OS, every installed toolchain's version and a hash
    // of every Cargo manifest and lockfile, in v2.9.2; a floating ref can change
    // what its key hashes underneath this check.
    for (const { file, id, step } of workflowSteps()) {
      if (!uses(step).startsWith("Swatinem/rust-cache@")) continue;
      // A workflow no pull request runs restores nothing a pull request waits
      // on; these are what a pull request restores and what main saves for it.
      if (pullRequestTrigger(workflows[file]) === undefined && file !== "rust-cache-warm.yml") {
        continue;
      }
      assert.equal(uses(step), "Swatinem/rust-cache@v2.9.2", `${file} \`${id}\``);
      assert.equal(step.with?.key, undefined, `${file} \`${id}\` adds a key of its own`);
    }
  });

  it("are never saved by a pull-request run", () => {
    for (const { file, id, job, step } of workflowSteps()) {
      const workflow = workflows[file];
      if (pullRequestTrigger(workflow) === undefined) continue;
      if (uses(step).startsWith("Swatinem/rust-cache@")) {
        const save = String(step.with?.["save-if"] ?? "");
        assert.ok(
          save === "false" ||
            save.includes("github.ref == 'refs/heads/main'") ||
            save.includes("github.event_name != 'pull_request'"),
          `${file} \`${id}\` saves its Rust cache from a pull request`,
        );
      }
      if (uses(step).startsWith("actions/cache/save@") || uses(step) === "actions/cache@v4") {
        assert.ok(
          String(step.if ?? "").includes("refs/heads/main") || job.if?.includes("push"),
          `${file} \`${id}\` saves a cache from a pull request`,
        );
      }
    }
  });
});

describe("rust-cache-warm.yml", () => {
  const LEGS = { linux: "test-linux", windows: "test-windows", macos: "test-macos" };

  it("runs on push to main alone, and is no pull-request check", () => {
    assert.equal(pullRequestTrigger(warm), undefined);
    assert.deepEqual((warm.on ?? warm[true]).push?.branches, ["main"]);
  });

  for (const [leg, job] of Object.entries(LEGS)) {
    it(`builds and saves, testing nothing, what \`${job}\` restores (${leg})`, () => {
      const warmer = warm.jobs?.[leg];
      assert.ok(warmer, `rust-cache-warm.yml has no \`${leg}\` job`);
      assert.equal(warmer["runs-on"], ci.jobs[job]["runs-on"]);
      const saved = steps(warmer).filter((step) => uses(step).startsWith("Swatinem/rust-cache@"));
      const restored = steps(ci.jobs[job]).filter((step) =>
        uses(step).startsWith("Swatinem/rust-cache@"),
      );
      assert.equal(saved.length, 1);
      assert.equal(restored.length, 1);
      assert.equal(saved[0].with?.["shared-key"], job);
      assert.equal(restored[0].with?.["shared-key"], job);
      assert.equal(saved[0].with?.["save-if"], undefined, "the warm build is not saved");
      assert.equal(saved[0].with?.workspaces, restored[0].with?.workspaces);
      for (const step of steps(warmer)) {
        assert.doesNotMatch(runs(step), /\bjust (ci-test|check|test)\b|nextest run/, step.name);
      }
      const keys = (all) =>
        all
          .filter((step) => uses(step).startsWith("actions/cache"))
          .map((step) => step.with.key)
          .sort();
      for (const key of keys(steps(ci.jobs[job]))) {
        assert.ok(keys(steps(warmer)).includes(key), `nothing on main saves ${key}`);
      }
      // ci.yml's workflow-wide CARGO_* settings are part of rust-cache's key.
      for (const [name, value] of Object.entries(ci.env ?? {})) {
        if (name.startsWith("CARGO") || name.startsWith("RUST")) {
          assert.equal(warm.env?.[name], value, `the warm build's ${name} differs from ci.yml's`);
        }
      }
    });
  }

  it("skips the macOS build on the push that merges a release pull request", () => {
    assert.match(String(warm.jobs.macos.if), /head_commit\.message, 'chore: release'/);
    assert.equal(warm.jobs.linux.if, undefined);
    assert.equal(warm.jobs.windows.if, undefined);
  });
});
