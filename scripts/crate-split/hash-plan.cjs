#!/usr/bin/env node
// What Nx puts in each named task's cache key at the checkout this runs in: run with the
// checkout as the working directory, its `node_modules` holding Nx.
//
//   node scripts/crate-split/hash-plan.cjs OUT TASK...
//
// For each `project:target` it writes Nx's own hash plan (HashPlanInspector, the inputs
// the native task hasher hashes: `file:` paths, `runtime:` commands, `env:` names, the
// project's configuration and the external dependencies), with the task's configured
// `inputs` from the project graph so a file can be traced to the named input that put it
// there. Each distinct runtime command is run here and its output kept, so two
// checkouts' values can be compared. A task the checkout's graph does not have is
// listed under `missing`. It is `crate-split-replay.mjs --observed`'s reader, and reads
// nothing outside the checkout.
"use strict";
const { execSync } = require("node:child_process");
const { createHash } = require("node:crypto");
const { writeFileSync } = require("node:fs");
const { join } = require("node:path");

const root = process.cwd();
const nx = (path) => require(join(root, "node_modules", "nx", "dist", "src", path));

async function main([out, ...tasks]) {
  const { createProjectGraphAsync } = nx("project-graph/project-graph.js");
  const { HashPlanInspector } = nx("hasher/hash-plan-inspector.js");
  const graph = await createProjectGraphAsync({ exitOnError: true });
  const inspector = new HashPlanInspector(graph, root);
  await inspector.init();
  const plans = {};
  const missing = [];
  const runtime = {};
  for (const id of tasks) {
    const at = id.indexOf(":");
    const [project, target] = [id.slice(0, at), id.slice(at + 1)];
    const node = graph.nodes[project];
    if (!node?.data.targets?.[target]) {
      missing.push(id);
      continue;
    }
    const plan = inspector.inspectHashPlan([project], [target], undefined, {}, {}, true)[id] ?? [];
    plans[id] = {
      root: node.data.root,
      inputs: node.data.targets[target].inputs ?? null,
      cache: node.data.targets[target].cache ?? null,
      command: node.data.targets[target].options?.command ?? node.data.targets[target].command ?? null,
      config: createHash("sha256").update(JSON.stringify(node.data)).digest("hex"),
      plan,
    };
    for (const entry of plan) {
      if (entry.startsWith("runtime:") && !(entry in runtime)) {
        const command = entry.slice("runtime:".length);
        try {
          runtime[entry] = {
            ok: true,
            out: execSync(command, { cwd: root, encoding: "utf8", stdio: ["ignore", "pipe", "ignore"] }),
          };
        } catch (error) {
          runtime[entry] = { ok: false, out: String(error.status) };
        }
      }
    }
  }
  const external = createHash("sha256")
    .update(JSON.stringify(graph.externalNodes ?? {}))
    .digest("hex");
  writeFileSync(out, `${JSON.stringify({ plans, missing, runtime, external })}\n`);
}

main(process.argv.slice(2)).catch((error) => {
  console.error(`hash-plan: ${error.message}`);
  process.exitCode = 1;
});
