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
import { spawnSync } from "node:child_process";
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { describe, it } from "node:test";
import { commit, root, run, scratchWorkspace } from "./support.mjs";

const CHANGED_PROJECT = "pool-maintenance";
const CHANGED_FILE = "pool-maintenance/measure.sh";
// A file of a project no job tests: the judged lint's project holds
// `lint-llm-diff` alone, which none of the four jobs' recipes selects.
const UNTESTED_FILE = "lint-llm/NOTES.md";
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
        // Either of node's reporters: spec at a terminal here, TAP on a runner.
        assert.match(log, /^(ℹ|#) pass [1-9]/m, log);
      }
      for (const project of UNREACHED) {
        assert.ok(!ran(log).some((task) => task.startsWith(`${project}:`)), `${project} ran`);
      }
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

  it("ci-test-linux hands its tasks none of the Nx wrapper's own switches", (t) => {
    // A suite that drives Nx itself, as the npm suite does, must meet the
    // wrapper as a developer does, not switched by the recipe that started it.
    const dir = scratchWorkspace(t);
    commit(
      dir,
      "pool-maintenance/env.test.mjs",
      'import assert from "node:assert/strict";\nimport { it } from "node:test";\n\nit("inherits no wrapper switch", () => assert.equal(process.env.ONEPIPELINE_NX_SHOW_OUTPUT, undefined));\n',
    );
    const { status, log } = run(dir, "just", ["ci-test-linux"], {
      ONEPIPELINE_NX_BASE_REF: "main",
    });
    assert.equal(status, 0, log);
    assert.deepEqual(ran(log), ["pool-maintenance:test"], log);
  });

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
  it("ci-test-linux exits non-zero when a committed lockfile is stale", (t) => {
    const dir = scratchWorkspace(t);
    commit(dir, CHANGED_FILE, editOf(dir, CHANGED_FILE));
    // What a bootstrap that had to re-resolve the lock leaves behind.
    const lock = join(dir, "package-lock.json");
    run(dir, "bash", ["-c", 'printf "\\n" >> "$1"', "_", lock]);
    const { status, log } = run(dir, "just", ["ci-test-linux"], {
      ONEPIPELINE_NX_BASE_REF: "main",
    });
    assert.notEqual(status, 0, log);
    assert.deepEqual(ran(log), ["pool-maintenance:test"], log);
    assert.match(log, /a committed lockfile differs from what this checkout resolves/);
  });
});

// The four target lists are the one copy of what each job owes, so they are
// held here to what the repo-wide verbs mean: every target a project's `check`
// reaches runs in `lint` or `test-linux`, and every target `just check-cross`
// runs, in `lint` or each of the other two.
describe("the jobs' target lists", () => {
  const script = readFileSync(join(root, "scripts/ci-job.sh"), "utf8");
  const listed = (job) => {
    const line = script
      .split("\n")
      .find((each) => new RegExp(`^\\s*[a-z| -]*\\b${job}\\b[a-z| -]*\\) targets="`).test(each));
    assert.ok(line, `scripts/ci-job.sh lists no targets for ${job}`);
    return /targets="([^"]+)"/.exec(line)[1].split(",");
  };
  const projects = spawnSync("git", ["ls-files", "--", "*project.json"], {
    cwd: root,
    encoding: "utf8",
  })
    .stdout.split("\n")
    .filter((path) => path.endsWith("project.json"))
    .map((path) => JSON.parse(readFileSync(join(root, path), "utf8")));

  it("cover every target a project's `check` reaches between `lint` and `test-linux`", () => {
    const owed = new Set([...listed("lint"), ...listed("test-linux")]);
    for (const project of projects) {
      for (const target of project.targets?.check?.dependsOn ?? []) {
        if (typeof target !== "string") continue;
        assert.ok(owed.has(target), `${project.name}:check reaches ${target}, which no job runs`);
      }
    }
  });

  it("cover every target `just check-cross` runs, on Windows and macOS alike", () => {
    const justfile = readFileSync(join(root, "justfile"), "utf8");
    const recipes = /^check-cross: (.+)$/m.exec(justfile)[1].split(" ");
    const target = { "fmt-check": "format-check", lint: "lint", "test-quick": "test-quick" };
    for (const job of ["test-windows", "test-macos"]) {
      const owed = new Set([...listed("lint"), ...listed(job)]);
      for (const recipe of recipes) {
        assert.ok(target[recipe], `check-cross runs \`${recipe}\`, which this test cannot map`);
        assert.ok(owed.has(target[recipe]), `${job} drops check-cross's ${target[recipe]}`);
      }
    }
  });
});

// rust-cache-warm.yml saves what scripts/warm-rust-cache.sh builds, so its
// recipes are held to the build recipes the jobs' own targets run: a build a
// job compiles that the warm build leaves out is compiled cold on every pull
// request, and nothing else would say so.
describe("the warm build", () => {
  const warm = readFileSync(join(root, "scripts/warm-rust-cache.sh"), "utf8");
  const recipes = (mode) => {
    const body = new RegExp(`^  ${mode}\\)\\n([\\s\\S]*?)\\n    ;;`, "m").exec(warm)?.[1];
    assert.ok(body, `scripts/warm-rust-cache.sh has no ${mode} mode`);
    return new Set([...body.matchAll(/\bjust ([^\n]+)/g)].flatMap((m) => m[1].trim().split(/\s+/)));
  };
  const project = (path) => JSON.parse(readFileSync(join(root, path), "utf8"));
  const recipeOf = (path, target) => {
    const command = project(path).targets[target]?.command ?? "";
    return /^just (_[\w-]+)$/.exec(command)?.[1];
  };
  for (const [mode, builds] of Object.entries({
    // test-linux's instrumented archive, debug build, docs and the stop-verdict
    // tier's release build; lint's clippy, which restores the same key.
    linux: [
      ["project.json", "test-archive"],
      ["project.json", "build"],
      ["project.json", "doc"],
      ["project.json", "lint"],
      ["tests/stop_verdict/project.json", "test-build"],
    ],
    // test-windows' and test-macos' uninstrumented archive and clippy.
    cross: [
      ["project.json", "test-quick-archive"],
      ["project.json", "lint"],
    ],
  })) {
    it(`builds, in its ${mode} mode, every build recipe those jobs' targets run`, () => {
      const warmed = recipes(mode);
      for (const [path, target] of builds) {
        const recipe = recipeOf(path, target);
        assert.ok(recipe, `${path}'s ${target} runs no recipe this can name`);
        assert.ok(warmed.has(recipe), `the ${mode} warm build leaves out ${target}'s ${recipe}`);
      }
    });
  }
});

describe("the task-graph reader", () => {
  for (const [what, input, reason] of [
    ["no JSON at all", "nx: something went wrong\n", /answered no JSON task graph/],
    ["a graph with no task map", '{"graph":{"nodes":{}}}', /a graph with no task map/],
    [
      "a task id that is not a project:target",
      '{"tasks":{"tasks":{"two words:test":{"target":{"project":"two words","target":"test"}}}}}',
      /is not a project:target task/,
    ],
    [
      "a task id extending its entry's target",
      '{"tasks":{"tasks":{"a:test-other":{"target":{"project":"a","target":"test"}}}}}',
      /is not a project:target task/,
    ],
    [
      "a task entry naming another task",
      '{"tasks":{"tasks":{"a:test":{"target":{"project":"b","target":"lint"}}}}}',
      /is not a project:target task/,
    ],
  ]) {
    it(`refuses ${what} rather than reading it as an empty selection`, () => {
      const result = spawnSync("node", [join(root, "scripts/ci-job-tasks.mjs")], {
        input,
        encoding: "utf8",
      });
      assert.equal(result.status, 1, result.stderr);
      assert.equal(result.stdout, "");
      assert.match(result.stderr, reason);
      assert.match(result.stderr, /^ACTION: /m);
    });
  }
});

describe("the recipes' refusals", () => {
  it("reads a well-formed task map as its sorted ids", () => {
    const graph = {
      tasks: {
        tasks: {
          "b:test": { target: { project: "b", target: "test" } },
          "a:lint": { target: { project: "a", target: "lint" } },
          "a:build:production": {
            target: { project: "a", target: "build", configuration: "production" },
          },
        },
      },
    };
    const result = spawnSync("node", [join(root, "scripts/ci-job-tasks.mjs")], {
      input: `nx's own preamble\n${JSON.stringify(graph)}`,
      encoding: "utf8",
    });
    assert.equal(result.status, 0, result.stderr);
    assert.equal(result.stdout, "a:build:production a:lint b:test");
  });

  it("refuses a job ci.yml does not have", () => {
    const { status, log } = run(root, "bash", ["scripts/ci-job.sh", "deploy"]);
    assert.equal(status, 2, log);
    assert.match(log, /'deploy' is not one of ci\.yml's jobs/);
  });
});
