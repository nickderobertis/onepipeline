// What ci.yml's four jobs promise, read off the workflow files and the
// screencomp.toml the visual-docs lane is resolved from (this target's inputs):
//
// - `lint`, `test-linux`, `test-windows` and `test-macos`, by those ids, unnamed,
//   on the runners their names say, each bounded at its ruled timeout, each
//   starting on every pull request, including release branches, and
//   none waiting on another job;
// - the visual docs captured and judged as steps of `lint`, through screencomp's
//   composite actions, and never through its reusable workflow from ci.yml;
// - release branches take the ordinary four-job path without shortcuts;
// - `budgets`, last and however the others ended, on every pull request and every push to main,
//   through the action at the release `.github/reporting/action-release/` vendors;
// - every context branch protection requires reported by a pull-request job;
// - every cache a workflow restores keyed on what produced it, pull requests
//   restoring Rust caches without saving them, and `rust-cache-warm.yml` saving
//   from main under exactly the keys the test jobs restore, each leg deleting its
//   families' superseded copies, and a fork's run writing nothing to any cache.
import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { readdirSync, readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { describe, it } from "node:test";
import { fileURLToPath } from "node:url";
import { parse } from "yaml";
import { runGraph } from "../workflow-graph.mjs";

const github = join(dirname(fileURLToPath(import.meta.url)), "..");
const workflows = Object.fromEntries(
  readdirSync(join(github, "workflows"))
    .filter((name) => name.endsWith(".yml"))
    .map((name) => [name, parse(readFileSync(join(github, "workflows", name), "utf8"))]),
);
const ci = workflows["ci.yml"];
const warm = workflows["rust-cache-warm.yml"];

const FOUR = {
  lint: "ubuntu-24.04",
  "test-linux": "ubuntu-latest",
  "test-windows": "windows-latest",
  "test-macos": "macos-latest",
};
// Each job's bound in minutes: `lint` at the 30 the four were planned to, and
// each test job at its measured hosted duration with headroom, as ruled.
const TIMEOUT = { lint: 30, "test-linux": 180, "test-windows": 270, "test-macos": 210 };
const ON_PULL_REQUESTS = "github.event_name == 'pull_request'";
const CONDITION = {
  lint: ON_PULL_REQUESTS,
  "test-linux": ON_PULL_REQUESTS,
  "test-windows": ON_PULL_REQUESTS,
  "test-macos": ON_PULL_REQUESTS,
};
const TESTS = ["test-linux", "test-windows", "test-macos"];
const REPORT_ACTION = "nickderobertis/onebudgetspec-github-action";

// The contexts branch protection requires, from the tree's one copy of them
// (`required-contexts.json`, which tests/ci_workflows.rs reads too).
const REQUIRED = JSON.parse(readFileSync(join(github, "required-contexts.json"), "utf8")).contexts;
// The one lane visual-docs.yml's reusable workflow renders for this repository,
// `report (<arch>, <current>, <verify>, <title>)`, resolved by the workflow-graph
// interpreter from the pinned reusable workflow's own matrix and screencomp.toml:
// what `lint`'s composite steps judge must be what that workflow judges on a push
// to main.
const lanes = runGraph(workflows["visual-docs.yml"], {
  eventName: "push",
  ref: "refs/heads/main",
  head: "0".repeat(40),
  event: { head_commit: { message: "feat: a change" } },
}).filter((row) => row.job === "visual-docs / report");
assert.equal(lanes.length, 1, "visual-docs.yml renders other than one report lane");
const VISUAL_DOCS_LANE = lanes[0].name;
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

describe("the `budgets` job", () => {
  const job = ci.jobs?.budgets ?? {};
  const pinned = steps(job)
    .map(uses)
    .filter((name) => name.startsWith(`${REPORT_ACTION}@`));
  const tag = pinned[0]?.split("@")[1];
  const vendored = join(github, "reporting", "action-release", String(tag));

  it("waits on every other job and runs however they ended", () => {
    const others = Object.keys(ci.jobs).filter((id) => id !== "budgets");
    assert.deepEqual(needs(job).sort(), others.sort());
    assert.equal(job.if, "always()");
    assert.equal(job.name, undefined, "`budgets` overrides its name, which renames its check");
    assert.equal(job["runs-on"], "ubuntu-latest");
  });

  it("is the only job a push to main selects, and a push to main is never cancelled", () => {
    const on = ci.on ?? ci[true];
    assert.deepEqual(on.push, { branches: ["main"] });
    assert.equal(pullRequestTrigger(ci), null, "no type, path or branch filter: drafts included");
    assert.equal(ci.concurrency["cancel-in-progress"], "${{ github.event_name == 'pull_request' }}");
    const jobs = runGraph(ci, {
      eventName: "push",
      ref: "refs/heads/main",
      head: "0".repeat(40),
      event: { head_commit: { message: "feat: a change" } },
    });
    assert.deepEqual(jobs.filter((row) => row.selected).map((row) => row.job), ["budgets"]);
  });

  it("reads and writes only what the action declares, with one comment writer at a time", () => {
    assert.deepEqual(job.permissions, {
      contents: "read",
      actions: "read",
      "pull-requests": "write",
    });
    assert.equal(job.concurrency["cancel-in-progress"], false);
    const [checkout] = steps(job);
    assert.equal(checkout.with["fetch-depth"], 0, "local git proves saved commits' ancestry");
    assert.equal(checkout.with["persist-credentials"], false);
  });

  it("pins one exact release, whose vendored definition is the bytes that release lists", () => {
    assert.equal(pinned.length, 1);
    assert.match(tag, /^v\d+\.\d+\.\d+$/);
    const listed = readFileSync(join(vendored, "SHA256SUMS"), "utf8")
      .split("\n")
      .filter(Boolean)
      .map((line) => line.split(/\s+/));
    const digest = createHash("sha256")
      .update(readFileSync(join(vendored, "action.yml")))
      .digest("hex");
    assert.equal(Object.fromEntries(listed.map(([sum, name]) => [name, sum]))["action.yml"], digest);
  });

  it("passes only inputs the action declares, every required one, and lists release-tail as report-only", () => {
    const declared = parse(readFileSync(join(vendored, "action.yml"), "utf8")).inputs;
    const given = steps(job).find((step) => uses(step).startsWith(`${REPORT_ACTION}@`)).with;
    for (const name of Object.keys(given)) assert.ok(declared[name], `undeclared input ${name}`);
    for (const [name, spec] of Object.entries(declared))
      if (spec.required) assert.ok(given[name], `required input ${name}`);
    assert.deepEqual(given["enforcement-exclude-labels"].split(","), [
      "release-tail",
      "host-variable",
    ]);
    assert.equal(declared.labels.default, "ci-report");
    assert.equal(declared["budget-files"].default, "**/budgets.yaml");
  });

  it("reads what `test-linux` saved and leaves out what a pull request cannot measure", () => {
    const given = steps(job).find((step) => uses(step).startsWith(`${REPORT_ACTION}@`)).with;
    assert.equal(given["omit-labels"], "not-measured-on-pr");
    assert.equal(given["saved-results-artifacts"], "budget-check-results-*");
    const uploads = Object.entries(ci.jobs).flatMap(([id, definition]) =>
      steps(definition)
        .filter((step) => step.with?.name?.startsWith("budget-check-results-"))
        .map((step) => ({ id, step })),
    );
    assert.deepEqual(
      uploads.map(({ id }) => id),
      ["test-linux"],
    );
    const [{ step }] = uploads;
    assert.equal(step.if, "always()");
    assert.match(uses(step), /^actions\/upload-artifact@/);
    assert.deepEqual(step.with, {
      name: "budget-check-results-test-linux",
      path: "target/budget-results/",
      "if-no-files-found": "warn",
    });
    assert.equal(ci.jobs["test-linux"].if, "github.event_name == 'pull_request'");
  });

  it("is held by its own budget, read through the released reader's flags and metric", () => {
    const declared = parse(readFileSync(join(vendored, "action.yml"), "utf8")).inputs;
    const help = readFileSync(join(vendored, "telemetry-help.txt"), "utf8");
    const metric = JSON.parse(
      readFileSync(join(vendored, "report-metric.source.json"), "utf8"),
    ).metric;
    const { budgets } = parse(readFileSync(join(github, "reporting", "budgets.yaml"), "utf8"));
    assert.deepEqual(
      budgets.map((budget) => budget.id),
      ["onepipeline-budget-report-command-seconds"],
    );
    const [budget] = budgets;
    assert.deepEqual(budget.labels, [declared.labels.default]);
    assert.equal(budget.threshold, 120);
    assert.equal(budget.unit, "seconds");
    assert.deepEqual(budget.command.slice(0, 2), ["onebudgetspec-github-action", "telemetry"]);
    for (const flag of budget.command.filter((arg) => arg.startsWith("--")))
      assert.ok(help.includes(`${flag} <`), flag);
    assert.equal(budget.command[budget.command.indexOf("--metric") + 1], metric);
  });

  it("is required by the committed governance declaration", () => {
    assert.ok(REQUIRED.includes("budgets"));
  });
});

describe("the judged lint", () => {
  it("installs in `lint` each harness at an exact release", () => {
    // The harness pins are what an upstream release has broken a required check
    // through before, so neither may float.
    const installs = steps(ci.jobs.lint)
      .map(runs)
      .filter((run) => /^npm install -g /.test(run));
    assert.deepEqual(
      installs.map((run) => run.replace(/@\d+\.\d+\.\d+$/, "@<pinned>")).sort(),
      [
        "npm install -g @anthropic-ai/claude-code@<pinned>",
        "npm install -g @openai/codex@<pinned>",
      ],
    );
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
    // The capture clones HEAD under `target/ci-capture`, and rust-cache, on a
    // restore that is not a full key match, prunes files under `target/`: started
    // first, the clone loses its `rust-toolchain.toml` mid-capture (run
    // 41310531964's `lint`), and its manifests leak into rust-cache's key.
    const restored = lint.findIndex((step) => uses(step).startsWith("Swatinem/rust-cache@"));
    assert.ok(
      restored !== -1 && restored < started,
      "`lint` starts the capture under `target/` before rust-cache restores and prunes it",
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

  it("publish their comment and preview from `lint`, the one place a pull request's are made", () => {
    // visual-docs.yml no longer runs on a pull request's push, so nothing races
    // `lint`'s deploy for the Pages build screencomp gates on; the switch is on,
    // and it is what the deploy, the classify step and the comment each read.
    const lint = ci.jobs.lint;
    assert.equal(lint.env?.VISUAL_DOCS_PUBLISH, "true");
    const switched = (prefix) => {
      const step = steps(lint).find((candidate) => uses(candidate).startsWith(prefix));
      assert.ok(step, `\`lint\` has no ${prefix} step`);
      return step;
    };
    const publish = "${{ env.VISUAL_DOCS_PUBLISH }}";
    assert.equal(switched("nickderobertis/screencomp/visual-docs@").with?.publish, publish);
    assert.equal(switched("nickderobertis/screencomp/visual-docs-aggregate@").with?.publish, publish);
    assert.match(
      String(switched("nickderobertis/screencomp/visual-docs-pages@").if),
      /^env\.VISUAL_DOCS_PUBLISH == 'true' && /,
    );
    assert.equal(workflows["visual-docs.yml"].on.pull_request?.types?.join(), "closed");
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
    // The lane the reusable workflow renders, and the inputs visual-docs.yml sets
    // over its defaults (the snapshot in fixtures/, pinned with it): what the
    // composite steps pass must be what that workflow would, so the two paths
    // judge alike.
    const [arch, current, verify, title] = /\((.+)\)$/.exec(VISUAL_DOCS_LANE)[1].split(", ");
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

describe("release branches use the ordinary four-job path", () => {
  for (const id of Object.keys(FOUR)) {
    it(`keeps ${id} free of release shortcuts`, () => {
      assert.equal(ci.jobs[id].if, ON_PULL_REQUESTS);
      for (const step of steps(ci.jobs[id])) {
        assert.ok(!JSON.stringify(step).includes("shortcut"));
        assert.ok(!JSON.stringify(step).includes("release-plz-"));
      }
    });
  }
});

describe("the required contexts", () => {
  it("are exactly the four and `budgets`, each reported by ci.yml on a pull request", () => {
    assert.deepEqual([...REQUIRED].sort(), [...Object.keys(FOUR), "budgets"].sort());
    const reported = new Set();
    for (const [id, job] of Object.entries(ci.jobs ?? {})) {
      for (const name of rendered(id, job)) reported.add(name);
    }
    for (const context of REQUIRED) {
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
  // The visual-docs capture's build: also the workflow naming its image.
  {
    path: /^target\/ci-capture-target$/,
    key: [
      "runner.os",
      "runner.arch",
      "Cargo.lock",
      "rust-toolchain.toml",
      ".github/workflows/visual-docs.yml",
    ],
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

// What keeps a run's saves under the Actions cache line. Each problem names what is
// wrong, so the mutation table below can hold each one to the alteration that causes it.
const WARM_FAMILIES = {
  linux: ["v0-rust-test-linux-Linux-x64-", "ci-capture-Linux-X64-", "bootstrap-tools-Linux-X64-", "v0-rust-release-plz-Linux-x64-"],
  windows: ["v0-rust-test-windows-Windows_NT-x64-", "bootstrap-tools-Windows-X64-"],
  macos: ["v0-rust-test-macos-Darwin-arm64-", "bootstrap-tools-macOS-ARM64-"],
};
const CLEANUP_TOKEN = "${{ github.token }}";
const SAME_REPOSITORY = "github.event.pull_request.head.repo.full_name == github.repository";
const isCleanup = (step) => runs(step).includes("scripts/actions-cache-prune.mjs");
const familiesOf = (text) => [...String(text).matchAll(/--family (\S+)/g)].map((match) => match[1]);
// The Rust step a pull-request job restores through, and every action that can write
// an Actions cache unless told not to. An action not named in either list is refused
// until someone says which it is, so a new writer cannot arrive unguarded.
const CACHE_READ_ONLY = "${{ steps.nx-cache-trust.outputs.read_only == 'true' && '1' || '' }}";
const NEVER_WRITES = [
  "actions/checkout@", "actions/upload-artifact@", "actions/cache/restore@", "taiki-e/install-action@",
  "dtolnay/rust-toolchain@", "samypr100/setup-dev-drive@", "nickderobertis/screencomp", "nickderobertis/notignored@",
  "nickderobertis/onebudgetspec-github-action@", "amannn/action-semantic-pull-request@",
];
function cacheWriteProblem(id, step) {
  const name = uses(step);
  const label = `ci.yml ${id}: ${name || step.name}`;
  if (!name) return undefined;
  if (name.startsWith("Swatinem/rust-cache@"))
    return String(step.with?.["save-if"]) === "false" ? undefined : `${label} could save`;
  if (name.startsWith("astral-sh/setup-uv@"))
    return step.with?.["enable-cache"] === `\${{ ${SAME_REPOSITORY} }}` ? undefined : `${label} writes for a fork`;
  if (name.startsWith("op-nx/github-cache/start-cache-server@"))
    return step.env?.CACHE_READ_ONLY === CACHE_READ_ONLY ? undefined : `${label} writes for a fork`;
  if (name.startsWith("actions/setup-node@") || name.startsWith("actions/setup-python@"))
    return step.with?.cache === undefined || String(step.if).includes(SAME_REPOSITORY) ? undefined : `${label} writes for a fork`;
  if (name.startsWith("actions/cache/save@") || /^actions\/cache@/.test(name))
    return String(step.if).includes(SAME_REPOSITORY) ? undefined : `${label} writes for a fork`;
  return NEVER_WRITES.some((known) => name.startsWith(known)) ? undefined : `${label} is not classified as writing a cache or not`;
}
function warmProblems(all, budgetFile) {
  const problems = [];
  const warmFlow = all["rust-cache-warm.yml"];
  const trigger = warmFlow.on ?? warmFlow[true];
  if (Object.keys(trigger ?? {}).join() !== "push" || trigger.push?.branches?.join() !== "main")
    problems.push("a warm workflow runs on something other than a push to main");
  if (!String(warmFlow.concurrency?.group ?? "").includes("github.ref"))
    problems.push("the warm workflow lost its concurrency group");
  const named = [];
  for (const [leg, families] of Object.entries(WARM_FAMILIES)) {
    const job = warmFlow.jobs[leg];
    const all_steps = steps(job);
    const cleanup = all_steps.filter(isCleanup);
    if (cleanup.length !== 1) { problems.push(`${leg} has no cleanup step`); continue; }
    if (all_steps.at(-1) !== cleanup[0]) problems.push(`${leg}'s cleanup is not its last ordinary step`);
    named.push(...familiesOf(cleanup[0].run));
    if (familiesOf(cleanup[0].run).join() !== families.join()) problems.push(`${leg}'s cleanup names the wrong families`);
    if (cleanup[0].env?.GH_TOKEN !== CLEANUP_TOKEN) problems.push(`${leg}'s cleanup has no token`);
    for (const step of all_steps)
      if (step !== cleanup[0] && /github\.token|secrets\./.test(JSON.stringify(step)))
        problems.push(`${leg}: a step other than the cleanup is handed the token`);
    const checkout = all_steps.find((step) => uses(step).startsWith("actions/checkout@"));
    if (checkout?.with?.["persist-credentials"] !== false) problems.push(`${leg}'s checkout persists credentials`);
    if (job.permissions?.actions !== "write" || job.permissions?.contents !== "read" || Object.keys(job.permissions).length !== 2)
      problems.push(`${leg} does not hold exactly contents: read and actions: write`);
    const rust = all_steps.findIndex((step) => uses(step).startsWith("Swatinem/rust-cache@"));
    const cold = all_steps[rust + 1];
    const tools = all_steps.findIndex((step) => step.name === "Restore the bootstrap tools");
    if (cold?.if !== `steps.${all_steps[rust]?.id}.outputs.cache-hit != 'true'` || runs(cold).trim() !== "rm -rf target" || !(rust + 1 < tools))
      problems.push(`${leg} lacks the conditional target deletion`);
    const devDrive = "${{ env.DEV_DRIVE_WORKSPACE }}";
    if (leg === "windows" && cold?.["working-directory"] !== devDrive) problems.push("windows deletes a target outside the Dev Drive");
  }
  const linux = steps(warmFlow.jobs.linux);
  const deleted = linux.findIndex((step) => runs(step).trim() === "rm -rf target/ci-capture-target");
  const saved = linux.findIndex((step) => step.name === "Save the visual docs capture's build");
  if (deleted < 0 || deleted < saved || deleted > linux.findIndex(isCleanup))
    problems.push("the Linux leg no longer deletes target/ci-capture-target after the capture's save");
  const budget = budgetFile.budgets.find((entry) => entry.id === "onepipeline-cache-headroom");
  if (familiesOf(budget.command.join(" ")).sort().join() !== [...named].sort().join())
    problems.push("the budget's families differ from the cleanup steps'");
  for (const file of ["ci.yml", "release.yml"])
    for (const [id, job] of Object.entries(all[file].jobs))
      for (const step of steps(job))
        if (uses(step).startsWith("Swatinem/rust-cache@") && String(step.with?.["save-if"]) !== "false")
          problems.push(`${file} ${id}: a Rust cache step could save`);
  for (const id of ["lint", ...TESTS]) {
    const rust = steps(all["ci.yml"].jobs[id]).filter((step) => uses(step).startsWith("Swatinem/rust-cache@"));
    const key = id === "lint" ? "test-linux" : id;
    if (rust.length !== 1 || rust[0].with?.["shared-key"] !== key || rust[0].if !== undefined)
      problems.push(`ci.yml ${id} stopped restoring its Rust cache`);
  }
  for (const [file, flow] of Object.entries(all)) {
    if (pullRequestTrigger(flow) === undefined) continue;
    const held = [flow.permissions, ...Object.values(flow.jobs).map((job) => job.permissions)];
    if (held.some((grant) => grant?.actions === "write" || grant === "write-all"))
      problems.push(`${file}: a pull request triggers a job holding actions: write`);
  }
  for (const [id, job] of Object.entries(all["ci.yml"].jobs))
    for (const step of steps(job)) {
      const problem = cacheWriteProblem(id, step);
      if (problem) problems.push(problem);
    }
  return problems;
}

describe("the Actions cache keeps room for proven results", () => {
  const budgetFile = parse(readFileSync(join(github, "..", "budgets.yaml"), "utf8"));
  const stepNamed = (all, leg, test) => steps(all["rust-cache-warm.yml"].jobs[leg]).find(test);

  it("holds the composed shape as it stands", () => {
    assert.deepEqual(warmProblems(workflows, budgetFile), []);
  });

  // Each alteration the shape must refuse, and the problem it must be refused with.
  const ALTERATIONS = {
    "the cleanup step removed": [(all) => {
      const job = all["rust-cache-warm.yml"].jobs.windows;
      job.steps = job.steps.filter((step) => !isCleanup(step));
    }, "windows has no cleanup step"],
    "a family dropped from a cleanup": [(all) => {
      const step = stepNamed(all, "macos", isCleanup);
      step.run = step.run.replace(" --family bootstrap-tools-macOS-ARM64-", "");
    }, "macos's cleanup names the wrong families"],
    "a step after the cleanup": [(all) => {
      all["rust-cache-warm.yml"].jobs.linux.steps.push({ run: "true" });
    }, "linux's cleanup is not its last ordinary step"],
    "the budget's family dropped": [(_all, budget) => {
      const entry = budget.budgets.find((b) => b.id === "onepipeline-cache-headroom");
      entry.command = entry.command.slice(0, -2);
    }, "the budget's families differ from the cleanup steps'"],
    "a Rust step in ci.yml able to save": [(all) => {
      delete steps(all["ci.yml"].jobs.lint).find((step) => uses(step).startsWith("Swatinem/rust-cache@")).with["save-if"];
    }, "ci.yml lint: a Rust cache step could save"],
    "a Rust step in release.yml able to save": [(all) => {
      delete steps(all["release.yml"].jobs.publish).find((step) => uses(step).startsWith("Swatinem/rust-cache@")).with["save-if"];
    }, "release.yml publish: a Rust cache step could save"],
    "the warm workflow on pull requests": [(all) => {
      (all["rust-cache-warm.yml"].on ?? all["rust-cache-warm.yml"][true]).pull_request = {};
    }, "a warm workflow runs on something other than a push to main"],
    "the warm concurrency group gone": [(all) => {
      delete all["rust-cache-warm.yml"].concurrency;
    }, "the warm workflow lost its concurrency group"],
    "a pull-request job no longer restoring": [(all) => {
      const job = all["ci.yml"].jobs["test-macos"];
      job.steps = job.steps.filter((step) => !uses(step).startsWith("Swatinem/rust-cache@"));
    }, "ci.yml test-macos stopped restoring its Rust cache"],
    "actions: write on a pull-request job": [(all) => {
      all["ci.yml"].jobs["test-linux"].permissions.actions = "write";
    }, "ci.yml: a pull request triggers a job holding actions: write"],
    "a warm checkout persisting credentials": [(all) => {
      delete stepNamed(all, "windows", (step) => uses(step).startsWith("actions/checkout@")).with;
    }, "windows's checkout persists credentials"],
    "another warm step handed the token": [(all) => {
      stepNamed(all, "linux", (step) => step.name === "Build what test-linux and lint build").env = { GH_TOKEN: CLEANUP_TOKEN };
    }, "linux: a step other than the cleanup is handed the token"],
    "setup-uv's fork guard removed": [(all) => {
      delete steps(all["ci.yml"].jobs["test-windows"]).find((step) => uses(step).startsWith("astral-sh/setup-uv@")).with["enable-cache"];
    }, "ci.yml test-windows: astral-sh/setup-uv@v6 writes for a fork"],
    "the Nx sidecar's fork guard removed": [(all) => {
      delete steps(all["ci.yml"].jobs.lint).find((step) => uses(step).startsWith("op-nx/github-cache/")).env.CACHE_READ_ONLY;
    }, /ci\.yml lint: op-nx\/github-cache\/start-cache-server@\S+ writes for a fork/],
    "an unguarded cache save added": [(all) => {
      all["ci.yml"].jobs["test-linux"].steps.push({ uses: "actions/cache/save@v4", with: { path: "x", key: "y" } });
    }, "ci.yml test-linux: actions/cache/save@v4 writes for a fork"],
    "setup-node's own cache turned on": [(all) => {
      steps(all["ci.yml"].jobs.lint).find((step) => uses(step).startsWith("actions/setup-node@")).with.cache = "npm";
    }, "ci.yml lint: actions/setup-node@v4 writes for a fork"],
    "a new action nobody classified": [(all) => {
      all["ci.yml"].jobs.lint.steps.push({ uses: "example/setup-thing@v1" });
    }, "ci.yml lint: example/setup-thing@v1 is not classified as writing a cache or not"],
    "the conditional target deletion removed": [(all) => {
      const job = all["rust-cache-warm.yml"].jobs.macos;
      job.steps = job.steps.filter((step) => step.name !== "Build cold on a moved lock or toolchain");
    }, "macos lacks the conditional target deletion"],
    "the deletion reading another step's output": [(all) => {
      stepNamed(all, "linux", (step) => step.name === "Build cold on a moved lock or toolchain").if = "steps.tools.outputs.cache-hit != 'true'";
    }, "linux lacks the conditional target deletion"],
    "the Windows deletion leaving the Dev Drive": [(all) => {
      delete stepNamed(all, "windows", (step) => step.name === "Build cold on a moved lock or toolchain")["working-directory"];
    }, "windows deletes a target outside the Dev Drive"],
    "the capture's deletion removed": [(all) => {
      const job = all["rust-cache-warm.yml"].jobs.linux;
      job.steps = job.steps.filter((step) => runs(step).trim() !== "rm -rf target/ci-capture-target");
    }, "the Linux leg no longer deletes target/ci-capture-target after the capture's save"],
  };
  for (const [name, [alter, expected]] of Object.entries(ALTERATIONS)) {
    it(`refuses ${name}`, () => {
      const all = structuredClone(workflows);
      const budget = structuredClone(budgetFile);
      alter(all, budget);
      const problems = warmProblems(all, budget);
      assert.ok(
        problems.some((problem) => (expected instanceof RegExp ? expected.test(problem) : problem === expected)),
        `expected "${expected}" among: ${problems.join("; ") || "no problems"}`,
      );
    });
  }

  // Each upstream copy, by the digest its header and versions file both state.
  const COPIES = {
    "scripts/actions-cache-prune.mjs": ["112878b64182200739b5e0c7e34decc3778f0316f0cb05f9bd40f66445d33031", 1],
    "scripts/actions-budget-figures.sh": ["d530d435aead03a443b1ea528db96eef9b97cc343d7eb3fade0b09099bd112ef", 13],
    "scripts/nx-remote-cache.mjs": ["f8ee98c7b95c9e2afe7cab4539183dabce85d92c67506215b052782d849923e8", 10],
  };
  for (const [path, [digest, version]] of Object.entries(COPIES)) {
    it(`carries ${path} version ${version} unaltered`, () => {
      const text = readFileSync(join(github, "..", path), "utf8");
      const lines = text.split("\n");
      const header = lines.findIndex((line) => /sha256: [a-f0-9]{64}$/.test(line));
      assert.equal(lines[header].split(": ").at(-1), digest);
      assert.match(lines[header - 1], new RegExp(`version: ${version}$`));
      assert.equal(createHash("sha256").update(lines.slice(header + 1).join("\n")).digest("hex"), digest);
      const versions = readFileSync(join(github, "..", path.replace(/\.(mjs|sh)$/, ".versions")), "utf8").trim().split("\n");
      assert.equal(versions.at(-1), `${version} ${digest}`);
    });
  }
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

  it("saves from its Linux leg every cache `lint` restores", () => {
    const keys = (all) =>
      all.filter((step) => uses(step).startsWith("actions/cache")).map((step) => step.with.key);
    const saved = steps(warm.jobs.linux)
      .filter((step) => uses(step).startsWith("actions/cache/save@"))
      .map((step) => step.with.key);
    assert.ok(keys(steps(ci.jobs.lint)).length > 0, "lint restores no cache");
    for (const key of keys(steps(ci.jobs.lint))) {
      assert.ok(saved.includes(key), `nothing on main saves ${key}`);
    }
  });

  it("skips the macOS build on the push that merges a release pull request", () => {
    assert.match(String(warm.jobs.macos.if), /head_commit\.message, 'chore: release'/);
    assert.equal(warm.jobs.linux.if, undefined);
    assert.equal(warm.jobs.windows.if, undefined);
  });
});
