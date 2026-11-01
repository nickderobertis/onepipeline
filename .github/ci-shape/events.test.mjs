// Which workflows and jobs each event starts, read off the workflow files and evaluated by the
// workflow-graph interpreter `.github/tests/` measures the Mac budgets with (its triggers, its
// `if:` expressions, its reusable-workflow expansion):
//
// - a pull request's last commit (`opened`, `synchronize`, `reopened`) runs exactly ci.yml's
//   `lint`, `test-linux`, `test-windows` and `test-macos` across every workflow — `test-macos`
//   skipped on release-plz's own release pull request — and nothing else starts;
// - its closing runs visual-docs.yml alone, to delete its preview;
// - a push to main starts release-plz.yml, visual-docs.yml and rust-cache-warm.yml alone, and
//   the release commit runs release-plz's job and the warmer's Linux and Windows legs, with
//   visual-docs.yml's job skipped;
// - visual-docs.yml keeps its `closed`, `schedule` and `workflow_dispatch` triggers;
// - the workflows are exactly the ones this shape names, with the triggers it gives each.
import assert from "node:assert/strict";
import { readdirSync, readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { describe, it } from "node:test";
import { fileURLToPath } from "node:url";
import { parse } from "yaml";
import { runGraph, triggered } from "../workflow-graph.mjs";

const github = join(dirname(fileURLToPath(import.meta.url)), "..");
const root = join(github, "..");
const load = () =>
  Object.fromEntries(
    readdirSync(join(github, "workflows"))
      .filter((name) => name.endsWith(".yml"))
      .map((name) => [name, parse(readFileSync(join(github, "workflows", name), "utf8"))]),
  );

const FOUR = ["lint", "test-linux", "test-macos", "test-windows"];
// Every workflow, and the events each one is started by.
const TRIGGERS = {
  "ci.yml": ["pull_request"],
  "engine-currency.yml": ["workflow_dispatch"],
  "launcher.yml": ["workflow_dispatch"],
  "release-plz.yml": ["push"],
  "release.yml": ["release"],
  "rust-cache-warm.yml": ["push"],
  "visual-docs.yml": ["pull_request", "push", "schedule", "workflow_dispatch"],
};

function pullRequest(action, branch, fork) {
  return {
    eventName: "pull_request",
    ref: `refs/pull/1/merge`,
    head: "0".repeat(40),
    event: {
      action,
      repository: { full_name: "nickderobertis/onepipeline" },
      pull_request: {
        title: branch.startsWith("release-plz-") ? "chore: release v1.2.4" : "feat: a change",
        head: {
          ref: branch,
          repo: { full_name: fork ? "fork/onepipeline" : "nickderobertis/onepipeline" },
        },
        base: { ref: "main" },
      },
    },
  };
}
function pushToMain(message) {
  return {
    eventName: "push",
    ref: "refs/heads/main",
    head: "0".repeat(40),
    event: { head_commit: { message } },
  };
}
const RELEASE_COMMIT = pushToMain("chore: release v1.2.4 (#1331)");
const ORDINARY_MERGE = pushToMain("feat: a change (#1330)");

// The runs an event starts: each workflow whose trigger accepts it, with its jobs and whether
// each one is selected to run.
function started(all, scenario) {
  return Object.entries(all)
    .filter(([, workflow]) => triggered(workflow, scenario))
    .map(([file, workflow]) => ({ file, jobs: runGraph(workflow, scenario, root) }));
}
const ran = (runs) =>
  runs.flatMap((run) => run.jobs.filter((job) => job.selected).map((job) => `${run.file}:${job.job}`));

// Every way the workflows could start more on a pull request's push or on the release commit than
// this shape allows, or lose a trigger visual-docs.yml keeps.
export function eventProblems(all) {
  const problems = [];
  for (const action of ["opened", "synchronize", "reopened"])
    for (const branch of ["feature", "release-plz-2026-11-01T00-00-00Z"])
      for (const fork of [false, true]) {
        const label = `pull_request ${action} from ${fork ? "a fork's" : "a"} ${branch}`;
        const runs = started(all, pullRequest(action, branch, fork));
        const files = runs.map((run) => run.file).sort();
        if (JSON.stringify(files) !== JSON.stringify(["ci.yml"]))
          problems.push(`${label} starts ${files.join(", ")}, not ci.yml alone`);
        const releasePlz = branch.startsWith("release-plz-") && !fork;
        const want = FOUR.filter((job) => !(releasePlz && job === "test-macos")).map(
          (job) => `ci.yml:${job}`,
        );
        const got = ran(runs).sort();
        if (JSON.stringify(got) !== JSON.stringify(want))
          problems.push(`${label} runs ${got.join(", ")}, not ${want.join(", ")}`);
      }
  const closed = started(all, pullRequest("closed", "feature", false));
  if (JSON.stringify(closed.map((run) => run.file)) !== JSON.stringify(["visual-docs.yml"]))
    problems.push(
      `a closed pull request starts ${closed.map((run) => run.file).join(", ")}, not visual-docs.yml alone`,
    );
  for (const [what, push, want] of [
    [
      "the release commit",
      RELEASE_COMMIT,
      ["release-plz.yml:release-plz", "rust-cache-warm.yml:linux", "rust-cache-warm.yml:windows"],
    ],
    [
      "an ordinary merge",
      ORDINARY_MERGE,
      [
        "release-plz.yml:release-plz",
        "rust-cache-warm.yml:linux",
        "rust-cache-warm.yml:macos",
        "rust-cache-warm.yml:windows",
      ],
    ],
  ]) {
    const runs = started(all, push);
    const files = runs.map((run) => run.file).sort();
    const pushWorkflows = ["release-plz.yml", "rust-cache-warm.yml", "visual-docs.yml"];
    if (JSON.stringify(files) !== JSON.stringify(pushWorkflows))
      problems.push(`${what} starts ${files.join(", ")}, not ${pushWorkflows.join(", ")}`);
    const got = ran(runs).filter((job) => !job.startsWith("visual-docs.yml:"));
    if (JSON.stringify(got.sort()) !== JSON.stringify(want))
      problems.push(`${what} runs ${got.join(", ")}, not ${want.join(", ")}`);
    const docs = ran(runs).filter((job) => job.startsWith("visual-docs.yml:"));
    const release = push === RELEASE_COMMIT;
    if (release && docs.length) problems.push(`the release commit runs ${docs.join(", ")}`);
    if (!release && !docs.length)
      problems.push("an ordinary merge does not publish the canonical gallery");
  }
  const docs = all["visual-docs.yml"]?.on ?? {};
  if (JSON.stringify(docs.pull_request?.types) !== JSON.stringify(["closed"]))
    problems.push("visual-docs.yml does not run on a pull request's `closed` alone");
  if (!docs.schedule) problems.push("visual-docs.yml lost its `schedule`");
  if (!Object.hasOwn(docs, "workflow_dispatch"))
    problems.push("visual-docs.yml lost its `workflow_dispatch`");
  const files = Object.keys(all).sort();
  if (JSON.stringify(files) !== JSON.stringify(Object.keys(TRIGGERS).sort()))
    problems.push(`the workflows are ${files.join(", ")}, not ${Object.keys(TRIGGERS).join(", ")}`);
  for (const [file, events] of Object.entries(TRIGGERS)) {
    const on = Object.keys(all[file]?.on ?? {}).sort();
    if (JSON.stringify(on) !== JSON.stringify(events))
      problems.push(`${file} runs on ${on.join(", ")}, not ${events.join(", ")}`);
  }
  return problems;
}

describe("the events each workflow runs on", () => {
  it("start exactly the four on a pull request's push and release-plz alone on the release commit", () => {
    assert.deepEqual(eventProblems(load()), []);
  });

  it("refuse another workflow or job on a pull request, a push run on the release commit, or a lost trigger", () => {
    const cases = {
      "not ci.yml alone": (all) => {
        all["notignored.yml"] = {
          name: "notignored",
          on: { pull_request: null },
          jobs: { suppressions: { "runs-on": "ubuntu-latest", steps: [{ run: "true" }] } },
        };
      },
      "runs ci.yml:gate": (all) => {
        all["ci.yml"].jobs.gate = { "runs-on": "ubuntu-latest", steps: [{ run: "just check" }] };
      },
      "starts ci.yml, visual-docs.yml, not ci.yml alone": (all) => {
        all["visual-docs.yml"].on.pull_request.types = ["opened", "synchronize", "closed"];
      },
      "not visual-docs.yml alone": (all) => {
        all["launcher.yml"].on.pull_request = { types: ["closed"] };
      },
      "the release commit starts": (all) => {
        all["ci.yml"].on = { push: { branches: ["main"] }, pull_request: null };
      },
      "the release commit runs visual-docs.yml": (all) => {
        delete all["visual-docs.yml"].jobs["visual-docs"].if;
      },
      "the release commit runs release-plz.yml:release-plz, rust-cache-warm.yml:linux, rust-cache-warm.yml:macos":
        (all) => {
          delete all["rust-cache-warm.yml"].jobs.macos.if;
        },
      "lost its `schedule`": (all) => {
        delete all["visual-docs.yml"].on.schedule;
      },
      "lost its `workflow_dispatch`": (all) => {
        delete all["visual-docs.yml"].on.workflow_dispatch;
      },
      "on a pull request's `closed` alone": (all) => {
        all["visual-docs.yml"].on.pull_request.types = [];
      },
      "the workflows are": (all) => {
        all["engine-currency-2.yml"] = all["engine-currency.yml"];
      },
      "launcher.yml runs on": (all) => {
        all["launcher.yml"].on.push = { branches: ["main"] };
      },
    };
    for (const [expected, change] of Object.entries(cases)) {
      const all = load();
      change(all);
      let found;
      try {
        found = eventProblems(all);
      } catch (error) {
        found = [String(error)];
      }
      assert.ok(
        found.some((problem) => problem.includes(expected)),
        `${expected}: ${JSON.stringify(found)}`,
      );
    }
  });
});
