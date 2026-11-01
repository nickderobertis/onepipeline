// Which paths reach the pool-maintenance budget, read from the task graphs Nx itself builds.
//
// Its measurement builds the workspace twice, so no per-change path may select it: not
// `just check` (every project's `check`), and not the affected run, whatever the change
// touches, and no workflow either: it is run by hand, through `just budgets-host`.

import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { readdirSync, readFileSync } from "node:fs";
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

  it("is run by hand through `just budgets-host`, and by no workflow", () => {
    const workflows = join(root, ".github", "workflows");
    for (const name of readdirSync(workflows)) {
      const workflow = readFileSync(join(workflows, name), "utf8");
      assert.doesNotMatch(workflow, /budgets-host/, `${name} runs the host budgets`);
    }
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
