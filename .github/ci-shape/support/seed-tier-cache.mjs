// Seed genuine Nx cache records for the real graph; the journey then invokes
// Nx normally, so a dependency accidentally rebuilding an archive still runs.
import { createRequire } from "node:module";
const require = createRequire(`${process.cwd()}/package.json`);
const { createProjectGraphAsync } = require("nx/src/devkit-exports.js");
const { readNxJson } = require("nx/src/config/nx-json.js");
const { createTaskGraph } = require("nx/src/tasks-runner/create-task-graph.js");
const { createTaskHasher } = require("nx/src/hasher/create-task-hasher.js");
const { hashTask, getTaskDetails } = require("nx/src/hasher/hash-task.js");
const { DbCache } = require("nx/src/tasks-runner/cache.js");
const projectGraph = await createProjectGraphAsync({ exitOnError: true });
const projects = Object.keys(projectGraph.nodes).filter((p) =>
  (p === "onepipeline" || projectGraph.nodes[p].data.root.startsWith("tests/")) &&
  projectGraph.nodes[p].data.targets["test-quick"]?.cache === true);
const graph = createTaskGraph(projectGraph, {}, projects, ["test-quick"], undefined, {});
const hasher = createTaskHasher(projectGraph, readNxJson());
const cache = new DbCache({ skipRemoteCache: true });
await cache.init();
for (const task of Object.values(graph.tasks)) {
  const config = projectGraph.nodes[task.target.project].data.targets[task.target.target];
  if (config.cache !== true) continue;
  await hashTask(hasher, projectGraph, graph, task, process.env, getTaskDetails());
  await cache.put(task, `proven fixture result: ${task.id}\n`, [], 0);
}
console.log(JSON.stringify(projects.filter((p) => projectGraph.nodes[p].data.targets["test-quick"]?.cache === true)));
