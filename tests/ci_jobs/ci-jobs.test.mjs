// ci.yml's four jobs as their recipes run them — `just ci-lint`, `ci-test-linux`,
// `ci-test-windows` and `ci-test-macos` — over a scratch copy of this workspace
// with a committed change, asked what they select and made to run it.
//
// What a pull request pays for, and on which runner, is decided by these four
// selections, and a mistake in one is silent in the direction of passing: a
// target a job never selects is a check that never ran. So each journey commits
// a change, drives the real recipe through the real Nx, and reads the recipe's
// own output: its `ci-job: <job> runs N task(s): ...` line, and Nx's
// `nx run <project>:<target>` line for each task that ran.
//
// The committed change is to `pool-maintenance`, a project nothing else depends
// on, whose targets are seconds of Node: what the Linux recipes then run is real
// and cheap. The Windows and macOS recipes are driven here through their
// selection step (`--plan`), since this host cannot run their platforms.
import assert from "node:assert/strict";
import { describe, it } from "node:test";
import { commit, root, run, scratchWorkspace } from "./support.mjs";

const CHANGED_PROJECT = "pool-maintenance";
const CHANGED_FILE = "pool-maintenance/measure.sh";
// A file of a project no job tests: the judged lint's project holds
// `lint-llm-diff` alone, which none of the four jobs' recipes selects.
const UNTESTED_FILE = "lint-llm/NOTES.md";
// Projects the change to pool-maintenance does not reach.
const UNREACHED = ["onepipeline", "onepipeline-e2e", "onepipeline-npm", "github-workflows"];

const JOBS = {
  "ci-lint": { job: "lint", targets: ["format-check", "lint"] },
  "ci-test-linux": { job: "test-linux", targets: ["test"] },
  "ci-test-windows": { job: "test-windows", targets: ["lint"] },
  "ci-test-macos": { job: "test-macos", targets: ["lint"] },
};

function selected(log, job) {
  const line = log.split("\n").find((each) => each.startsWith(`ci-job: ${job} runs `));
  assert.ok(line, `the recipe printed no selection line for ${job}:\n${log}`);
  const tasks = line.split(": ").slice(2).join(": ").trim();
  return tasks === "none" ? [] : tasks.split(" ");
}

function ran(log) {
  return [...log.matchAll(/^\s*> nx run (\S+)/gm)].map((match) => match[1]).sort();
}

function editOf(dir, path) {
  return `${run(dir, "cat", [path]).log.trimEnd()}\n# a change the ci-jobs journey commits\n`;
}

describe("each job's recipe selects only the projects a change reaches", () => {
  for (const [recipe, { job, targets }] of Object.entries(JOBS)) {
    it(`${recipe} selects ${CHANGED_PROJECT}'s targets and nothing it does not reach`, (t) => {
      const dir = scratchWorkspace(t);
      commit(dir, CHANGED_FILE, editOf(dir, CHANGED_FILE));
      const { status, log } = run(dir, "just", [recipe, "--plan"], {
        ONEPIPELINE_NX_BASE_REF: "main",
      });
      assert.equal(status, 0, log);
      assert.deepEqual(
        selected(log, job),
        targets.map((target) => `${CHANGED_PROJECT}:${target}`).sort(),
      );
      assert.deepEqual(ran(log), [], `--plan ran tasks:\n${log}`);
    });
  }
});

describe("the Linux recipes run what they select and nothing else", () => {
  for (const recipe of ["ci-lint", "ci-test-linux"]) {
    const { job, targets } = JOBS[recipe];

    it(`${recipe} runs ${CHANGED_PROJECT}'s targets for a change to it`, (t) => {
      const dir = scratchWorkspace(t);
      commit(dir, CHANGED_FILE, editOf(dir, CHANGED_FILE));
      const { status, log } = run(dir, "just", [recipe], { ONEPIPELINE_NX_BASE_REF: "main" });
      assert.equal(status, 0, log);
      const expected = targets.map((target) => `${CHANGED_PROJECT}:${target}`).sort();
      assert.deepEqual(selected(log, job), expected);
      assert.deepEqual(ran(log), expected, log);
      if (recipe === "ci-test-linux") {
        // The project's own suite ran, rather than a runner that skipped it.
        assert.match(log, /^ℹ pass [1-9]/m, log);
      }
      for (const project of UNREACHED) {
        assert.ok(!ran(log).some((task) => task.startsWith(`${project}:`)), `${project} ran`);
      }
      assert.match(log, new RegExp(`^ci-job: ${job}: ok$`, "m"));
    });

    it(`${recipe} runs nothing for a change to nothing it tests`, (t) => {
      const dir = scratchWorkspace(t);
      commit(dir, UNTESTED_FILE, "a note the ci-jobs journey commits\n");
      const { status, log } = run(dir, "just", [recipe], { ONEPIPELINE_NX_BASE_REF: "main" });
      assert.equal(status, 0, log);
      assert.deepEqual(selected(log, job), []);
      assert.deepEqual(ran(log), [], log);
    });
  }

  it("ci-test-linux exits non-zero when a selected target fails", (t) => {
    const dir = scratchWorkspace(t);
    commit(
      dir,
      "pool-maintenance/fails.test.mjs",
      'import assert from "node:assert/strict";\nimport { it } from "node:test";\n\nit("fails on purpose", () => assert.fail("the ci-jobs journey\'s failing test"));\n',
    );
    const { status, log } = run(dir, "just", ["ci-test-linux"], {
      ONEPIPELINE_NX_BASE_REF: "main",
    });
    assert.notEqual(status, 0, log);
    assert.deepEqual(ran(log), ["pool-maintenance:test"], log);
    assert.match(log, /the ci-jobs journey's failing test/);
    assert.match(log, /^ci-job: test-linux failed \(Nx exit \d+\)/m);
  });

  it("ci-lint exits non-zero when a selected target fails", (t) => {
    const dir = scratchWorkspace(t);
    commit(dir, "pool-maintenance/unformatted.mjs", "export   const  unformatted = 1\n");
    const { status, log } = run(dir, "just", ["ci-lint"], { ONEPIPELINE_NX_BASE_REF: "main" });
    assert.notEqual(status, 0, log);
    assert.ok(ran(log).includes("pool-maintenance:format-check"), log);
    assert.match(log, /^ci-job: lint failed \(Nx exit \d+\)/m);
  });
});

describe("the recipes' refusals", () => {
  it("refuses a job ci.yml does not have", () => {
    const { status, log } = run(root, "bash", ["scripts/ci-job.sh", "deploy"]);
    assert.equal(status, 2, log);
    assert.match(log, /'deploy' is not one of ci\.yml's jobs/);
  });
});
