// Which paths reach the pool-maintenance budget, read from the task graphs Nx itself builds.
//
// Its measurement builds the workspace twice, so no per-change path may select it: not
// `just check` (every project's `check`), and not the affected run, whatever the change
// touches. The release-PR sweep runs it, after its gate, through `just budgets-host`.

import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { readFileSync } from "node:fs";
import { createRequire } from "node:module";
import { dirname, join } from "node:path";
import { describe, it } from "node:test";
import { fileURLToPath } from "node:url";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");
const measurement = "pool-maintenance:budgets-host";
const nxPackage = createRequire(join(root, "package.json")).resolve("nx/package.json");
const nx = join(dirname(nxPackage), JSON.parse(readFileSync(nxPackage, "utf8")).bin.nx);

/** The task ids of the graph Nx would run for `args`. */
function tasks(args) {
  const result = spawnSync(process.execPath, [nx, ...args, "--graph=stdout"], {
    cwd: root,
    encoding: "utf8",
    env: { ...process.env, NX_DAEMON: "false", NX_USE_LOCAL: "true" },
    maxBuffer: 64 * 1024 * 1024,
  });
  assert.equal(result.status, 0, `nx ${args.join(" ")} failed:\n${result.stderr}`);
  return Object.keys(JSON.parse(result.stdout).tasks.tasks);
}

describe("the pool-maintenance budget", () => {
  it("is the task its own target runs, so the graphs below can show it", () => {
    assert.ok(tasks(["run", measurement]).includes(measurement));
  });

  it("is not reached by `check` over every project", () => {
    const graph = tasks(["run-many", "-t", "check"]);
    assert.ok(graph.includes("pool-maintenance:check"), graph.join(" "));
    assert.ok(!graph.includes(measurement), graph.join(" "));
  });

  for (const file of [
    "src/main.rs",
    "pool-maintenance/measure.sh",
    "pool-maintenance/budgets.yaml",
    "pool-maintenance/project.json",
    "nx.json",
    "justfile",
  ]) {
    it(`is not reached by the affected run over a change to ${file}`, () => {
      const graph = tasks(["affected", "-t", "check", `--files=${file}`]);
      assert.ok(graph.length > 0, `a change to ${file} selected nothing at all`);
      assert.ok(!graph.includes(measurement), graph.join(" "));
      if (file.startsWith("src/")) {
        assert.deepEqual(
          graph.filter((task) => task.startsWith("pool-maintenance:")),
          [],
          "a source-only change selected the pool-maintenance project",
        );
      }
    });
  }

  it("is run by the release-PR sweep after its quality gate", () => {
    const workflow = readFileSync(join(root, ".github", "workflows", "release-sweep.yml"), "utf8");
    const gate = workflow.indexOf("just check");
    const job = workflow.split(/\n {2}host-budgets:\n/)[1]?.split(/\n {2}\S/)[0] ?? "";
    assert.ok(gate > 0, "release-sweep.yml no longer runs `just check`");
    assert.match(job, /^ {4}needs: release-sweep$/m, "the host budgets do not wait for the gate");
    assert.match(job, /^ {8}run: just budgets-host$/m, job);
    const justfile = readFileSync(join(root, "justfile"), "utf8");
    const recipe = justfile.split("\nbudgets-host:\n")[1]?.split("\n\n")[0] ?? "";
    assert.match(recipe, /scripts\/nx\.sh run-many -t budgets-host$/m, recipe);
  });

  it("is registered in its project's budgets file alone", () => {
    const own = readFileSync(join(root, "pool-maintenance", "budgets.yaml"), "utf8");
    assert.match(own, /- id: onepipeline-pool-maintenance-seconds\n {4}[\s\S]*labels: \[host\]/);
    const shared = readFileSync(join(root, "budgets.yaml"), "utf8");
    assert.doesNotMatch(shared, /onepipeline-pool-maintenance-seconds/);
  });
});
