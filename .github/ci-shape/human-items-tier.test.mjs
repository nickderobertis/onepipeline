// The human-items budget tier's split between a pull request and a push to main.
//
// A pull request's `test-linux` counts what each warm host-wide read does
// (`human-items-e2e:test`), beside the rest of the job and with no release binary of its
// own; only the push-to-main `timed` job times one (`human-items-e2e:timed`). Selection is
// asked of the real Nx through `scripts/ci-job.sh <job> --plan` over a real commit in a
// scratch copy of this workspace, as CI asks it; the task graph is the real `nx run
// --graph` over this checkout. The fixture both journeys read is restored by every job that
// selects one and saved by `timed` alone.
import assert from "node:assert/strict";
import { execFileSync, spawnSync } from "node:child_process";
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { describe, it } from "node:test";
import { parse } from "yaml";
import { command, root, workspace } from "./support/cache-workspace.mjs";

const COUNTS = "human-items-e2e:test";
const TIMED = "human-items-e2e:timed";
const PULL_REQUEST_JOBS = ["lint", "test-linux", "test-windows", "test-macos"];
const ENV = { CI: "true", NX_DAEMON: "false", NX_ISOLATE_PLUGINS: "false", GITHUB_BASE_REF: "" };
const ci = parse(readFileSync(join(root, ".github/workflows/ci.yml"), "utf8"));
const declared = JSON.parse(readFileSync(join(root, "tests/human_items/project.json"), "utf8"));

// Commits an edit to `path` on top of HEAD and answers what each job plans against the commit before.
async function plan(ws, path, jobs) {
  const base = ws.git("rev-parse", "HEAD");
  const file = join(ws.dir, path);
  writeFileSync(file, `${readFileSync(file, "utf8")}\n// edited by the human-items journey\n`);
  ws.git("add", "-A");
  ws.git("-c", "user.name=Human items journey", "-c", "user.email=human-items@example.invalid", "commit", "-qm", path);
  const planned = {};
  for (const job of jobs) {
    const result = await command(ws.dir, "bash", ["scripts/ci-job.sh", job, "--plan"], { ...ENV, ONEPIPELINE_NX_BASE_SHA: base });
    assert.equal(result.code, 0, `${job}: ${result.stdout}${result.stderr}`);
    const line = result.stdout.split("\n").find((each) => each.startsWith(`ci-job: ${job} runs `));
    assert.ok(line, `${job} printed no selection: ${result.stdout}`);
    const tasks = line.split("task(s): ")[1].trim();
    planned[job] = new Set(tasks === "none" ? [] : tasks.split(" "));
  }
  return planned;
}

// The tasks `nx run <task>` would run, read from Nx's own task graph, and what each waits on.
function graph(task) {
  const dir = mkdtempSync(join(process.env.ONEPIPELINE_NODE_SCRATCH_DIR || tmpdir(), "human-items-graph-"));
  try {
    const path = join(dir, "graph.json");
    execFileSync(join(root, "node_modules/.bin/nx"), ["run", task, `--graph=${path}`], {
      cwd: root,
      env: { ...process.env, NX_DAEMON: "false" },
      stdio: "ignore",
    });
    return JSON.parse(readFileSync(path, "utf8")).tasks;
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
}

const steps = (job) => ci.jobs[job].steps ?? [];
const fixtureEnv = (job) =>
  steps(job)
    .map((step) => step.env ?? {})
    .filter((env) => "HUMAN_ITEMS_FIXTURE_STORE" in env || "HUMAN_ITEMS_FIXTURE_SAVE_DIR" in env);

describe("the human-items tier", () => {
  it("counts on a pull request for a change under src/, and times only in the push-to-main job", { timeout: 900000 }, async () => {
    const ws = workspace();
    try {
      const planned = await plan(ws, "src/lib.rs", [...PULL_REQUEST_JOBS, "timed"]);
      assert.ok(planned["test-linux"].has(COUNTS), `test-linux: ${[...planned["test-linux"]].join(" ")}`);
      for (const job of PULL_REQUEST_JOBS) assert.ok(!planned[job].has(TIMED), `${job} selects ${TIMED}`);
      assert.ok(planned.timed.has(TIMED), `timed: ${[...planned.timed].join(" ")}`);
      for (const task of planned.timed) assert.match(task, /:timed$/, "the timed job runs a task that is not a `timed` target");
    } finally {
      ws.close();
    }
  });

  it("counts beside the job after the debug build, reaching no `timed` target", () => {
    const test = declared.targets.test;
    assert.notEqual(test.parallelism, false, "the count waits for an empty runner");
    assert.equal(test.command, "just _human-items-counts");
    const tasks = graph(COUNTS);
    assert.ok(tasks.dependencies[COUNTS].includes("onepipeline:build"), JSON.stringify(tasks.dependencies[COUNTS]));
    for (const id of Object.keys(tasks.tasks)) assert.doesNotMatch(id, /:timed$/, `${COUNTS} reaches ${id}`);
  });

  it("times under the `timed`-target contract, saving its results where `ci-job.sh timed` reads them", () => {
    const timed = declared.targets.timed;
    assert.equal(timed.cache, false, "a replayed timing has no producing binary");
    assert.equal(timed.parallelism, false, "a timing taken beside other work");
    const budgets = parse(readFileSync(join(root, "tests/human_items/budgets.yaml"), "utf8")).budgets;
    for (const budget of budgets) assert.ok(budget.labels?.includes("not-measured-on-pr"), budget.id);
    const dry = spawnSync("just", ["--dry-run", "_human-items-timed"], { cwd: root, encoding: "utf8" });
    assert.equal(dry.status, 0, dry.stderr);
    const lines = dry.stderr.split("\n");
    assert.ok(lines.some((line) => /check budgets\.yaml --save-results \.\.\/\.\.\/target\/budget-results\/human-items-e2e\.json$/.test(line)), lines.join("\n"));
  });

  it("restores its fixture in every job that runs it and saves it in the push-to-main job alone", () => {
    for (const job of ["test-linux", "timed"]) {
      const [env, ...more] = fixtureEnv(job);
      assert.equal(more.length, 0, `${job} names the fixture's store more than once`);
      assert.equal(env?.HUMAN_ITEMS_FIXTURE_STORE, "${{ github.workspace }}/tests/human_items/github-store.sh", job);
    }
    for (const job of Object.keys(ci.jobs).filter((name) => name !== "timed"))
      for (const env of fixtureEnv(job)) assert.ok(!("HUMAN_ITEMS_FIXTURE_SAVE_DIR" in env), `${job} would save a fixture`);
    assert.equal(ci.jobs.timed.if, "github.event_name == 'push'");
    const saving = steps("timed").find((step) => step.uses?.startsWith("actions/upload-artifact@") && /human-items-fixture/.test(step.with?.name ?? ""));
    assert.ok(saving, "the timed job uploads no fixture");
    const [env] = fixtureEnv("timed");
    assert.ok(saving.with.path.startsWith(`${env.HUMAN_ITEMS_FIXTURE_SAVE_DIR}/`), saving.with.path);
    // The family the store's names and the fixture's names share.
    const store = readFileSync(join(root, "tests/human_items/github-store.sh"), "utf8");
    const fixture = readFileSync(join(root, "tests/human_items/fixture.rs"), "utf8");
    assert.match(store, /human-items-fixture-\*\)/);
    assert.match(fixture, /format!\("human-items-fixture-Linux-\{arch\}-\{key\}"\)/);
  });
});
