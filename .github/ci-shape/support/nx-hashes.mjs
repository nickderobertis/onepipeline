#!/usr/bin/env node
// Adapted from spike-opnx-audit at 705d3530: compute the real Nx task hash of
// every task one of ci.yml's jobs runs, computed by Nx's own hasher — the
// createProjectGraphAsync -> createTaskGraph -> createTaskHasher().hashTask()
// sequence op-nx/github-cache's `capture-hashes.mjs` uses — so a figure here is
// the key the remote cache would be asked for, not a re-derivation of it.
//
// Usage:
//   task-hashes.mjs capture <workspace> <project:target>... [--out <file>]
//   task-hashes.mjs diff <before.json> <after.json>
//
// `capture` loads Nx from <workspace>'s own node_modules, with the daemon off,
// and prints (or writes) {nx, tasks: {<id>: {hash, nodes}}}: `nodes` is Nx's
// per-input breakdown, so `diff` can name the inputs that moved each hash.
// `diff` prints one line per task: `same <id>` or `moved <id>: <input>, ...`.
// Exit 0 on success, 2 on a usage error, 1 when a task is not in the graph or
// a record is unreadable or names a task the other does not.
import { execFileSync } from "node:child_process";
import { readFileSync, writeFileSync } from "node:fs";
import { createRequire } from "node:module";
import { resolve } from "node:path";
import { pathToFileURL } from "node:url";

const usage = (message) => {
  console.error(`task-hashes: ${message}`);
  console.error(
    "ACTION: run 'task-hashes.mjs capture <workspace> <project:target>... [--out <file>]' or 'task-hashes.mjs diff <before.json> <after.json>'",
  );
  process.exit(2);
};

async function capture(workspace, ids, out) {
  const root = resolve(workspace);
  process.env.NX_DAEMON = "false";
  process.env.NX_WORKSPACE_ROOT_PATH = root;
  let createTaskGraph;
  let createProjectGraphAsync;
  let readNxJson;
  let createTaskHasher;
  let projectGraph;
  let hasher;
  let nxVersion;
  let globFiles;
  let HashPlanInspector;
  let inspector;
  try {
    process.chdir(root);
    const require = createRequire(`${root}/package.json`);
    const load = (path) => import(pathToFileURL(require.resolve(path)).href);
    ({ createProjectGraphAsync } = await load("nx/src/devkit-exports.js"));
    ({ readNxJson } = await load("nx/src/config/nx-json.js"));
    ({ createTaskGraph } = await load(
      "nx/src/tasks-runner/create-task-graph.js",
    ));
    ({ createTaskHasher } = await load("nx/src/hasher/create-task-hasher.js"));
    ({ globWithWorkspaceContextSync: globFiles } = await load("nx/src/utils/workspace-context.js"));
    ({ HashPlanInspector } = await load("nx/src/hasher/hash-plan-inspector.js"));
    nxVersion = JSON.parse(
      readFileSync(require.resolve("nx/package.json"), "utf8"),
    ).version;
  } catch (error) {
    console.error(`task-hashes: cannot load Nx from ${root}: ${error.message}`);
    console.error(
      "ACTION: install the workspace's dependencies (its bootstrap or workspace-install script) and name the directory holding nx.json",
    );
    process.exit(1);
  }
  try {
    projectGraph = await createProjectGraphAsync({ exitOnError: true });
    hasher = createTaskHasher(projectGraph, readNxJson(root));
    inspector = new HashPlanInspector(projectGraph, root, readNxJson(root));
    await inspector.init();
  } catch (error) {
    console.error(
      `task-hashes: Nx could not read ${root}'s project graph: ${error.message}`,
    );
    console.error(
      "ACTION: fix the nx.json or project.json Nx names, check it with 'nx show projects', then capture again",
    );
    process.exit(1);
  }
  const tasks = {};
  const combined = { tasks: {}, dependencies: {}, continuousDependencies: {}, roots: [] };
  for (const id of ids) {
    const at = id.lastIndexOf(":");
    if (at <= 0 || !TASK.test(id)) usage(`'${id}' is not <project:target>`);
    const graph = createTaskGraph(projectGraph, {}, [id.slice(0, at)], [id.slice(at + 1)], undefined, {});
    Object.assign(combined.tasks, graph.tasks);
    Object.assign(combined.dependencies, graph.dependencies);
    Object.assign(combined.continuousDependencies, graph.continuousDependencies);
    combined.roots.push(...graph.roots);
  }
  const wanted = ids.map((id) => {
    if (!combined.tasks[id]) throw new Error(`no task ${id} in the task graph`);
    return combined.tasks[id];
  });
  // Nx's batched API deduplicates runtime commands across the selected tasks,
  // exactly as its runner does, rather than spawning each command per task.
  const hashes = await hasher.hashTasks(wanted, combined, process.env);
  for (const [index, id] of ids.entries()) {
    const at = id.lastIndexOf(":");
    if (at <= 0 || !TASK.test(id)) usage(`'${id}' is not <project:target>`);
    const project = id.slice(0, at);
    const target = id.slice(at + 1);
    const missing = (detail) => {
      console.error(`task-hashes: no task ${id} in the task graph; ${detail}`);
      console.error("ACTION: name a task the workspace's graph holds");
      process.exit(1);
    };
    let graph;
    try {
      graph = createTaskGraph(
        projectGraph,
        {},
        [project],
        [target],
        undefined,
        {},
      );
    } catch (error) {
      missing(error.message);
    }
    const task = graph.tasks[id];
    if (!task)
      missing(`it has ${Object.keys(graph.tasks).join(", ") || "none"}`);
    const hash = hashes[index];
    const inspection = inspector.inspectTaskInputs(task.target)[id];
    const sourceFiles = new Set(inspection.files);
    const rules = {};
    for (const key of Object.keys(hash.details.nodes)) {
      const match = key.match(/^(.*):\[(.*)\]$/);
      if (!match) continue;
      const projectRoot = projectGraph.nodes[match[1]]?.data.root ?? ".";
      const patterns = match[2].split(",");
      const expanded = patterns.map((pattern) => pattern
        .replaceAll("{workspaceRoot}/", "")
        .replaceAll("{projectRoot}/", projectRoot === "." ? "" : `${projectRoot}/`));

      for (let i = 0; i < patterns.length; i++) {
        if (patterns[i].startsWith("!")) continue;
        rules[patterns[i]] = globFiles(root, [expanded[i]]).filter((file) => sourceFiles.has(file));
      }
    }
    const tracked = execFileSync("git", ["ls-files", "-z"], { encoding: "utf8" }).split("\0").filter(Boolean);
    const runtimeFiles = new Set();
    for (const key of Object.keys(hash.details.nodes)) {
      const ecosystem = key.match(/--ecosystem (cargo|npm|python|files)/)?.[1];
      const names = { cargo: ["Cargo.toml", "Cargo.lock"], npm: ["package.json", "package-lock.json", "bun.lock", "pnpm-lock.yaml"], python: ["pyproject.toml", "uv.lock"] }[ecosystem];
      if (names) for (const file of tracked) { if (names.includes(file.split("/").at(-1))) runtimeFiles.add(file); }
      if (ecosystem === "files") for (const file of readFileSync(".github/nx-version-files", "utf8").split(/\r?\n/)) {
        if (file && !file.startsWith("#")) runtimeFiles.add(file);
      }
    }
    tasks[id] = { cacheable: projectGraph.nodes[project].data.targets[target].cache === true, hash: hash.value, nodes: hash.details.nodes, sourceFiles: [...sourceFiles].sort(), runtimeFiles: [...runtimeFiles].sort(), rules };
  }
  const record = `${JSON.stringify({ nx: nxVersion, tasks }, null, 2)}\n`;
  if (out) {
    try {
      writeFileSync(out, record);
    } catch (error) {
      console.error(`task-hashes: cannot write ${out}: ${error.message}`);
      console.error("ACTION: pass --out a path in a writable directory");
      process.exit(1);
    }
  } else process.stdout.write(record);
}

const TASK = /^[A-Za-z0-9@][A-Za-z0-9@._/-]*:[A-Za-z0-9][A-Za-z0-9._-]*$/;

// A record is what `capture` writes: {tasks: {<id>: {hash: string, nodes: {...}}}}.
function readRecord(path) {
  let record;
  try {
    record = JSON.parse(readFileSync(path, "utf8"));
  } catch (error) {
    console.error(`task-hashes: cannot read ${path}: ${error.message}`);
    console.error("ACTION: pass a record written by 'task-hashes.mjs capture'");
    process.exit(1);
  }
  const tasks = record?.tasks;
  const isMap = (value) =>
    value !== null && typeof value === "object" && !Array.isArray(value);
  const valid =
    isMap(tasks) &&
    Object.keys(tasks).every((id) => TASK.test(id)) &&
    Object.values(tasks).every(
      (task) =>
        typeof task?.hash === "string" &&
        isMap(task.nodes) &&
        Object.values(task.nodes).every((node) => typeof node === "string"),
    );
  if (!valid) {
    console.error(`task-hashes: ${path} is not a capture record`);
    console.error("ACTION: pass a record written by 'task-hashes.mjs capture'");
    process.exit(1);
  }
  return tasks;
}

function diff(beforePath, afterPath) {
  const before = readRecord(beforePath);
  const after = readRecord(afterPath);
  const unmatched = [
    ...Object.keys(before)
      .filter((id) => !Object.hasOwn(after, id))
      .map((id) => `${id} is only in ${beforePath}`),
    ...Object.keys(after)
      .filter((id) => !Object.hasOwn(before, id))
      .map((id) => `${id} is only in ${afterPath}`),
  ];
  for (const id of Object.keys(before).sort()) {
    if (!Object.hasOwn(after, id)) continue;
    if (before[id].hash === after[id].hash) {
      console.log(`same ${id} ${before[id].hash}`);
      continue;
    }
    const keys = new Set([
      ...Object.keys(before[id].nodes),
      ...Object.keys(after[id].nodes),
    ]);
    const moved = [...keys]
      .filter((key) => before[id].nodes[key] !== after[id].nodes[key])
      .sort();
    console.log(
      `moved ${id} ${before[id].hash} -> ${after[id].hash}: ${moved.join(", ")}`,
    );
  }
  if (unmatched.length > 0) {
    for (const line of unmatched) console.error(`task-hashes: ${line}`);
    console.error(
      "ACTION: capture both states with the same task list, so every task is compared",
    );
    process.exit(1);
  }
}

const [mode, ...rest] = process.argv.slice(2);
if (mode === "capture") {
  const outAt = rest.indexOf("--out");
  let out;
  if (outAt >= 0) {
    out = rest[outAt + 1];
    if (!out || out.startsWith("--")) usage("--out needs a path");
    rest.splice(outAt, 2);
  }
  const [workspace, ...ids] = rest;
  if (!workspace || ids.length === 0)
    usage("capture needs a workspace and at least one task");
  await capture(workspace, ids, out && resolve(out));
} else if (mode === "diff") {
  if (rest.length !== 2) usage("diff needs two records");
  diff(rest[0], rest[1]);
} else {
  usage("the mode is capture or diff");
}
