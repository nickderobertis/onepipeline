import assert from "node:assert/strict";
import { execFileSync, spawnSync } from "node:child_process";
import { cpSync, existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, symlinkSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { test } from "node:test";

const root = fileURLToPath(new URL("../../", import.meta.url));
const helper = fileURLToPath(new URL("./support/nx-hashes.mjs", import.meta.url));
const env = { ...process.env, NX_DAEMON: "false", CI: "true" };
for (const key of Object.keys(env)) if (key.startsWith("GITHUB_") || key === "NX_BASE" || key === "NX_HEAD") delete env[key];
const run = (cwd, cmd, args) => execFileSync(cmd, args, { cwd, env, encoding: "utf8", maxBuffer: 64 * 1024 * 1024 });
function fixture(revision) {
  const dir = mkdtempSync(join(process.env.ONEPIPELINE_NODE_SCRATCH_DIR ?? tmpdir(), "nx-inputs-"));
  if (revision) {
    const archive = execFileSync("git", ["archive", revision], { cwd: root, maxBuffer: 128 * 1024 * 1024 });
    execFileSync("tar", ["-xf", "-", "-C", dir], { input: archive });
  } else for (const path of run(root, "git", ["ls-files", "--cached", "--others", "--exclude-standard", "-z"]).split("\0").filter(Boolean)) {
    if (!existsSync(join(root, path))) continue;
    mkdirSync(dirname(join(dir, path)), { recursive: true });
    cpSync(join(root, path), join(dir, path), { dereference: false });
  }
  symlinkSync(resolve(root, "node_modules"), join(dir, "node_modules"), "junction");
  run(dir, "git", ["init", "-q"]);
  run(dir, "git", ["add", "-A"]);
  return dir;
}
function capture(dir, tasks) {
  rmSync(join(dir, ".nx/workspace-data"), { recursive: true, force: true });
  mkdirSync(join(dir, ".nx"), { recursive: true });
  const out = join(dir, ".nx/hash-details.json");
  run(dir, process.execPath, [helper, "capture", dir, ...tasks, "--out", out]);
  return JSON.parse(readFileSync(out, "utf8")).tasks;
}
const versionReaders = new Set([
  "onepipeline-npm:build", "onepipeline-npm:test", "onepipeline-stop-verdict:test", "onepipeline-stop-verdict:test-build",
  ...["contract", "e2e", "package"].flatMap((name) => ["test", "test-quick"].map((target) => `onepipeline-version-${name}:${target}`)),
]);

function ownVersion(dir) {
  return readFileSync(join(dir, "Cargo.toml"), "utf8").match(/\[package\][\s\S]*?\nversion = "([^"]+)"/)[1];
}
function assertLauncherVersion(dir, version) {
  // Exercise the user's cached build while keeping fixture entries out of CI's
  // shared store. The second call must rebuild a newly versioned launcher.
  const output = run(dir, process.execPath, ["node_modules/nx/dist/bin/nx.js", "run", "onepipeline-npm:build", "--skipRemoteCache", "--outputStyle=static"]);
  const manifest = JSON.parse(readFileSync(join(dir, "dist/npm/onepipeline-cli/package.json"), "utf8"));
  assert.equal(manifest.version, version, "Nx replayed a launcher carrying the previous version");
  for (const pin of Object.values(manifest.optionalDependencies)) assert.equal(pin, version);
  return output;
}
function setOwnVersion(dir, version) {
  const old = ownVersion(dir);
  for (const file of ["Cargo.toml", "Cargo.lock"]) {
    const path = join(dir, file);
    const text = readFileSync(path, "utf8");
    writeFileSync(path, file === "Cargo.toml"
      ? text.replace(/(\[package\][\s\S]*?\nversion = ")[^"]+"/, `$1${version}"`)
      : text.replace(`name = "onepipeline"\nversion = "${old}"`, `name = "onepipeline"\nversion = "${version}"`));
  }
}
function assertRecordedRelease(dir, ids) {
  const files = ["Cargo.toml", "Cargo.lock", "CHANGELOG.md"];
  const original = new Map(files.map((file) => [file, readFileSync(join(dir, file))]));
  try {
    // Reproduce the recorded release's own version and changelog, while keeping
    // this workspace's dependency entries, sources and input definition.
    const recordedManifest = run(root, "git", ["show", "96c876ab:Cargo.toml"]);
    setOwnVersion(dir, recordedManifest.match(/\[package\][\s\S]*?\nversion = "([^"]+)"/)[1]);
    writeFileSync(join(dir, "CHANGELOG.md"), run(root, "git", ["show", "96c876ab:CHANGELOG.md"]));
    const after = capture(dir, ids);
    const patch = run(root, "git", ["diff", "96c876ab^", "96c876ab", "--", ...files]);
    execFileSync("git", ["apply", "--reverse", "-"], { cwd: dir, env, input: patch });
    const before = capture(dir, ids);
    for (const id of ids) {
      if (versionReaders.has(id)) assert.notEqual(before[id].hash, after[id].hash, `${id} lost recorded-release version input`);
      else assert.equal(before[id].hash, after[id].hash, `${id} moved on recorded release-plz PR #1387`);
    }
  } finally { for (const [file, bytes] of original) writeFileSync(join(dir, file), bytes); }
}

test("version-only edits rebuild the cached npm launcher through Nx", { timeout: 120000 }, () => {
  const dir = fixture();
  try {
    const version = ownVersion(dir);
    assertLauncherVersion(dir, version);
    assert.match(assertLauncherVersion(dir, version), /\[local cache\]/, "the repeated launcher build did not replay");
    const [major, minor] = version.split(".").map(Number);
    const next = `${major}.${minor + 1}.0`;
    setOwnVersion(dir, next);
    assertLauncherVersion(dir, next);
  } finally { rmSync(dir, { recursive: true, force: true }); }
});

test("real Nx hashes preserve proven results across the recorded release edits", { timeout: 1800000 }, () => {
  const dir = fixture();
  try {
    const selected = new Set();
    for (const job of ["lint", "test-linux", "test-windows", "test-macos"]) {
      const plan = run(dir, "bash", ["scripts/ci-job.sh", job, "--plan"]);
      const line = plan.split("\n").find((line) => line.startsWith("ci-job:"));
      assert.ok(line, `${job} did not list its tasks`);
      for (const task of line.split("task(s): ")[1].split(" ")) selected.add(task);
    }
    const projects = JSON.parse(run(dir, process.execPath, ["node_modules/nx/dist/bin/nx.js", "show", "projects", "--json"]));
    const definitions = new Map(projects.map((name) => [name, JSON.parse(run(dir, process.execPath, ["node_modules/nx/dist/bin/nx.js", "show", "project", name, "--json"]))]));
    const tasks = [...selected].filter((id) => { const [project, target] = id.split(":"); return definitions.get(project).targets[target].cache === true; });
    const profiled = [...selected].filter((id) => {
      const [project, target] = id.split(":");
      return id === "onepipeline:test" || definitions.get(project).targets[target].outputs?.some((path) => /\.prof(?:raw|data)$/.test(path));
    });
    assert.ok(tasks.length > 0 && tasks.length + profiled.length >= 63, `only ${tasks.length} cached and ${profiled.length} uncached coverage tasks were exercised`);
    // This artifact-producing target is outside CI's selection, but unlike a
    // compilation proof its cached output must retain the package version.
    tasks.push("onepipeline-npm:build");
    const before = capture(dir, tasks);
    // The recorded release-plz update at spike main changed precisely these
    // fields and the changelog. Reproduce the same edits for the next version.
    const manifest = readFileSync(join(dir, "Cargo.toml"), "utf8");
    const version = manifest.match(/\[package\][\s\S]*?\nversion = "([^"]+)"/)[1];
    const [major, minor] = version.split(".").map(Number);
    const next = `${major}.${minor + 1}.0`;
    writeFileSync(join(dir, "Cargo.toml"), manifest.replace(`version = "${version}"`, `version = "${next}"`));
    const lock = readFileSync(join(dir, "Cargo.lock"), "utf8");
    writeFileSync(join(dir, "Cargo.lock"), lock.replace(`name = "onepipeline"\nversion = "${version}"`, `name = "onepipeline"\nversion = "${next}"`));
    const changelog = readFileSync(join(dir, "CHANGELOG.md"), "utf8");
    writeFileSync(join(dir, "CHANGELOG.md"), `## [${next}] - 2026-11-03\n\n- Next release\n\n${changelog}`);
    const after = capture(dir, tasks);
    for (const id of tasks) {
      if (versionReaders.has(id)) assert.notEqual(after[id].hash, before[id].hash, `${id} lost its version input`);
      else assert.equal(after[id].hash, before[id].hash, `${id} still hashes release-only edits`);
    }
    writeFileSync(join(dir, "Cargo.toml"), manifest);
    writeFileSync(join(dir, "Cargo.lock"), lock);
    writeFileSync(join(dir, "CHANGELOG.md"), changelog);

    // Every actual fileset Nx reports is exercised; choose one real tracked
    // file per rule, and assert every task that reads that file changes hash.
    const rules = new Map();
    for (const task of Object.values(before)) for (const [rule, files] of Object.entries(task.rules)) {
      if (files.length && !rules.has(rule)) rules.set(rule, files[0]);
    }
    const checkedFiles = new Set();
    for (const [rule, file] of rules) {
      if (checkedFiles.has(file)) continue;
      checkedFiles.add(file);
      const expected = tasks.filter((id) => before[id].sourceFiles.includes(file));
      assert.ok(expected.length, `no reader for ${rule}`);
      const original = readFileSync(join(dir, file));
      writeFileSync(join(dir, file), Buffer.concat([original, Buffer.from("\n ")]));
      const changed = capture(dir, expected);
      writeFileSync(join(dir, file), original);
      for (const id of expected) assert.notEqual(changed[id].hash, before[id].hash, `${id} lost ${rule} (${file})`);
    }

    for (const [label, change] of [
      ["dependency version", (text) => text.replace('tokio = "1"', 'tokio = "1.1"')],
      ["feature", (text) => text.includes("[features]") ? text.replace("[features]", "[features]\nnx_input_probe = []") : `${text}\n[features]\nnx_input_probe = []\n`],
    ]) {
      const mutated = change(manifest);
      assert.ok(mutated !== manifest, `fixture did not edit a ${label}`);
      writeFileSync(join(dir, "Cargo.toml"), mutated);
      const expected = tasks.filter((id) => before[id].runtimeFiles.includes("Cargo.toml"));
      const changed = capture(dir, expected);
      for (const id of expected) assert.notEqual(changed[id].hash, before[id].hash, `${id} lost ${label}`);
      writeFileSync(join(dir, "Cargo.toml"), manifest);
    }

    const dependencyLock = lock.replace(/(\[\[package\]\]\nname = "(?!onepipeline")[^"]+"\nversion = ")[^"]+/, (_match, prefix) => `${prefix}0.0.0`);
    assert.notEqual(dependencyLock, lock);
    writeFileSync(join(dir, "Cargo.lock"), dependencyLock);
    const cargoReaders = tasks.filter((id) => before[id].runtimeFiles.includes("Cargo.lock"));
    const changedLock = capture(dir, cargoReaders);
    for (const id of cargoReaders) assert.notEqual(changedLock[id].hash, before[id].hash, `${id} lost dependency lock version`);
    writeFileSync(join(dir, "Cargo.lock"), lock);

    const npmPath = join(dir, "package.json");
    const npmText = readFileSync(npmPath, "utf8");
    const npm = JSON.parse(npmText);
    npm.devDependencies.nx = "23.1.2";
    writeFileSync(npmPath, JSON.stringify(npm));
    const npmReaders = tasks.filter((id) => before[id].runtimeFiles.includes("package.json"));
    const changedNpm = capture(dir, npmReaders);
    for (const id of npmReaders) assert.notEqual(changedNpm[id].hash, before[id].hash, `${id} lost npm dependency`);
    writeFileSync(npmPath, npmText);
    console.error(`Nx inputs: exercised ${rules.size} fileset rules through ${checkedFiles.size} file mutations, Cargo features/dependencies/lock versions and npm dependencies`);

    // Prove this test can detect a missing input through real Nx, rather than
    // merely mirroring the declarative definition it is meant to guard.
    const nxPath = join(dir, "nx.json");
    const nxText = readFileSync(nxPath, "utf8");
    const nx = JSON.parse(nxText);
    nx.namedInputs.rustBuild = nx.namedInputs.rustBuild.filter((input) => input !== "{workspaceRoot}/src/**/*");
    writeFileSync(nxPath, JSON.stringify(nx));
    const victim = "onepipeline-contract:test";
    const missingBefore = capture(dir, [victim]);
    const source = readFileSync(join(dir, "src/lib.rs"));
    writeFileSync(join(dir, "src/lib.rs"), Buffer.concat([source, Buffer.from("\n ")]));
    const missingAfter = capture(dir, [victim]);
    assert.throws(() => assert.notEqual(missingAfter[victim].hash, missingBefore[victim].hash, "lost src/lib.rs"), /lost src\/lib.rs/);
    writeFileSync(join(dir, "src/lib.rs"), source);
    writeFileSync(nxPath, nxText);
  } finally { rmSync(dir, { recursive: true, force: true }); }
});


test("Nx's own expanded inputs cover every original cached task's files after the splits", { timeout: 240000 }, () => {
  const base = fixture("96c876ab");
  const current = fixture();
  try {
    const ids = new Set();
    for (const job of ["lint", "test-linux", "test-windows", "test-macos"]) {
      const line = run(base, "bash", ["scripts/ci-job.sh", job, "--plan"]).split("\n").find((line) => line.startsWith("ci-job:"));
      for (const id of line.split("task(s): ")[1].split(" ")) ids.add(id);
    }
    const originals = capture(base, [...ids]);
    const cached = [...ids].filter((id) => originals[id].cacheable);
    assert.equal(cached.length, 63, "the pinned pre-change workspace no longer has the audited 63 cacheable tasks");
    const extra = ["onepipeline-npm:build", ...[...versionReaders].filter((id) => id.startsWith("onepipeline-version-"))];
    const now = capture(current, [...new Set([...cached, ...extra])]);
    assertRecordedRelease(current, [...new Set([...cached, ...extra])]);
    // Manager ruling 229: the retired shortcut and its test are deliberately
    // deleted; every other historical input still belongs in the union.
    const retired = new Set(["scripts/release-pr-shortcut.mjs", "tests/ci_jobs/release-pr-shortcut.test.mjs"]);
    for (const file of retired) assert.ok(!existsSync(join(current, file)), `${file} must remain retired`);
    for (const id of cached) {
      const union = new Set([...now[id].sourceFiles, ...now[id].runtimeFiles]);
      const split = id.startsWith("onepipeline-contract:") ? "contract" : id.startsWith("onepipeline-e2e:") ? "e2e" : id.startsWith("onepipeline:") ? "package" : null;
      if (split) for (const target of ["test", "test-quick"]) {
        const moved = now[`onepipeline-version-${split}:${target}`];
        for (const file of [...moved.sourceFiles, ...moved.runtimeFiles]) union.add(file);
      }
      for (const file of originals[id].sourceFiles) {
        if (retired.has(file)) continue;
        if (file === "CHANGELOG.md" && !versionReaders.has(id)) continue;
        // Narrowed: a task now reads the recipes it runs through scripts/nx-recipe-input.mjs,
        // which the narrowing journey below holds, and names the file only if it reads it.
        if (file === "justfile" && !now[id].sourceFiles.includes(file)) continue;
        assert.ok(union.has(file === "biome.json" ? "biome.jsonc" : file), `${id} lost the input ${file}`);
      }
    }
  } finally {
    rmSync(base, { recursive: true, force: true });
    rmSync(current, { recursive: true, force: true });
  }
});

// Every cached target of every project, as real Nx reads it: what a global input's edit
// could move.
function cachedTargets(dir) {
  const out = join(dir, ".nx/project-graph.json");
  mkdirSync(join(dir, ".nx"), { recursive: true });
  rmSync(join(dir, ".nx/workspace-data"), { recursive: true, force: true });
  run(dir, process.execPath, ["node_modules/nx/dist/bin/nx.js", "graph", `--file=${out}`]);
  const found = new Map();
  for (const [name, node] of Object.entries(JSON.parse(readFileSync(out, "utf8")).graph.nodes))
    for (const [target, definition] of Object.entries(node.data.targets ?? {}))
      if (definition.cache === true) found.set(`${name}:${target}`, definition);
  rmSync(out, { force: true });
  return found;
}
const recipeOf = (definition) => String(definition.options?.command ?? definition.command ?? "").match(/^just (\S+)/)?.[1];
function assertMoved(before, after, expected, what) {
  const moved = Object.keys(before).filter((id) => before[id].hash !== after[id].hash).sort();
  assert.deepEqual(moved, [...expected].sort(), what);
}

test("a justfile edit moves the tasks that run or read what it changed, and no other", { timeout: 900000 }, () => {
  const dir = fixture();
  try {
    const targets = cachedTargets(dir);
    const tasks = [...targets.keys()];
    const before = capture(dir, tasks);
    const readers = tasks.filter((id) => before[id].sourceFiles.includes("justfile"));
    assert.ok(readers.includes("onepipeline:test-quick"), "the crate's tests read the justfile");
    assert.ok(!readers.includes("onepipeline-contract:test-quick"), "the contract tier does not read the file");
    const running = (...recipes) => tasks.filter((id) => recipes.includes(recipeOf(targets.get(id))));
    const quickTiers = tasks.filter((id) => id.endsWith(":test-quick"));
    const path = join(dir, "justfile");
    const original = readFileSync(path, "utf8");
    const editRecipe = (name) => {
      const header = new RegExp(`^${name}\\b[^\\n]*:[^\\n]*\\n`, "m");
      assert.match(original, header, `the justfile has no ${name}`);
      return original.replace(header, (line) => `${line}    @true input-probe\n`);
    };
    for (const [what, edited, expected] of [
      ["a recipe one task runs", editRecipe("_crate-doc"), running("_crate-doc")],
      ["a recipe many tasks run", editRecipe("_tier-lint"), running("_tier-lint")],
      ["a recipe reached only through a script a recipe names", editRecipe("_crate-test-quick-archive"), quickTiers],
      ["the variable the journeys read from the file", original.replace(/^onetaskgraph-version := "/m, 'onetaskgraph-version := "9'), quickTiers],
      ["a comment", `# a comment nothing runs\n${original}`, []],
    ]) {
      assert.notEqual(edited, original, what);
      writeFileSync(path, edited);
      const after = capture(dir, tasks);
      writeFileSync(path, original);
      assertMoved(before, after, new Set([...expected, ...readers]), what);
    }
    assert.ok(tasks.some((id) => !readers.includes(id) && !recipeOf(targets.get(id))), "a task that runs no recipe keeps its hash above");
  } finally { rmSync(dir, { recursive: true, force: true }); }
});

// nx.json stays a global input, and not by choice: Nx's own hash planner adds it (and
// .gitignore) to every task, named or not. A later Nx that stops doing so fails this,
// which is when narrowing it becomes possible.
test("Nx itself hashes nx.json into every task, so no input setting can narrow it", { timeout: 300000 }, () => {
  const dir = fixture();
  try {
    const tasks = ["onepipeline-e2e:test-quick", "onepipeline-contract:lint", "pool-maintenance:format-check"];
    const path = join(dir, "nx.json");
    const nx = JSON.parse(readFileSync(path, "utf8"));
    nx.namedInputs.sharedGlobals = nx.namedInputs.sharedGlobals.filter((input) => input !== "{workspaceRoot}/nx.json");
    writeFileSync(path, JSON.stringify(nx));
    const before = capture(dir, tasks);
    for (const id of tasks) assert.ok(before[id].sourceFiles.includes("nx.json"), `${id} no longer hashes nx.json`);
    writeFileSync(path, JSON.stringify({ ...nx, namedInputs: { ...nx.namedInputs, inputProbe: ["{workspaceRoot}/LICENSE"] } }));
    const after = capture(dir, tasks);
    for (const id of tasks) assert.notEqual(after[id].hash, before[id].hash, `${id} kept its hash, so nx.json can be narrowed now`);
  } finally { rmSync(dir, { recursive: true, force: true }); }
});

test("narrowing hashes selects the same projects: a justfile edit still touches them", { timeout: 300000 }, () => {
  const base = fixture("7e3a3c72");
  const current = fixture();
  try {
    const affected = (dir, file) => {
      rmSync(join(dir, ".nx/workspace-data"), { recursive: true, force: true });
      return JSON.parse(run(dir, process.execPath, ["node_modules/nx/dist/bin/nx.js", "show", "projects", "--affected", "--files", file, "--json"])).sort();
    };
    const projects = (dir) => new Set(JSON.parse(run(dir, process.execPath, ["node_modules/nx/dist/bin/nx.js", "show", "projects", "--json"])));
    const selected = affected(current, "justfile");
    assert.ok(selected.includes("onepipeline-contract"), "the justfile no longer selects the contract tier");
    // A project added since the base has no "before" to match; it is held to being selected at all.
    const existed = projects(base);
    assert.ok(selected.includes("onepipeline-status-read"), "the justfile does not select a project added after the base");
    assert.deepEqual(selected.filter((name) => existed.has(name)), affected(base, "justfile"), "the justfile selects other projects than before");
  } finally {
    rmSync(base, { recursive: true, force: true });
    rmSync(current, { recursive: true, force: true });
  }
});

test("a task that runs a recipe hashes that recipe, and the justfile is not global", { timeout: 120000 }, () => {
  const dir = fixture();
  try {
    const missing = [...cachedTargets(dir)].filter(([, definition]) => {
      const recipe = recipeOf(definition);
      return recipe && !definition.inputs.some((input) => new RegExp(`^node scripts/nx-recipe-input\\.mjs ${recipe}( |$)`).test(input.runtime ?? ""));
    }).map(([id]) => id);
    assert.deepEqual(missing, []);
    const { sharedGlobals } = JSON.parse(readFileSync(join(dir, "nx.json"), "utf8")).namedInputs;
    assert.ok(!sharedGlobals.includes("{workspaceRoot}/justfile"), "the justfile is global again");
  } finally { rmSync(dir, { recursive: true, force: true }); }
});

test("a failed recipe digest never yields a replayable key", { timeout: 120000 }, () => {
  const dir = fixture();
  try {
    const digest = (...args) => spawnSync(process.execPath, ["scripts/nx-recipe-input.mjs", ...args], { cwd: dir, encoding: "utf8" });
    const healthy = digest("_crate-doc");
    assert.equal(healthy.status, 0, healthy.stderr);
    assert.match(healthy.stdout.trim(), /^[0-9a-f]{64}$/);
    assert.equal(digest("_crate-doc").stdout, healthy.stdout, "a healthy tree must keep one digest");
    const [first, second] = [digest("no-such-recipe"), digest("no-such-recipe")];
    for (const failed of [first, second]) {
      assert.equal(failed.status, 1);
      assert.doesNotMatch(failed.stdout, /^[0-9a-f]{64}$/m);
      assert.match(failed.stderr, /ACTION: /);
    }
    assert.notEqual(first.stdout, second.stdout);
  } finally { rmSync(dir, { recursive: true, force: true }); }
});

test("every quick tier's real Nx hash retains the selected nextest partition", { timeout: 120000 }, () => {
  const dir = fixture();
  const previous = env.ONEPIPELINE_TEST_PARTITION;
  try {
    const files = run(root, "git", ["ls-files", "--cached", "--others", "--exclude-standard", "-z"]).split("\0").filter((p) => /(^|\/)project\.json$/.test(p));
    const tasks = files.map((path) => JSON.parse(readFileSync(join(root, path)))).filter((p) => p.targets["test-quick"]).map((p) => `${p.name}:test-quick`);
    const before = capture(dir, tasks);
    env.ONEPIPELINE_TEST_PARTITION = previous === "count:1/2" ? "count:2/2" : "count:1/2";
    const after = capture(dir, tasks);
    for (const id of tasks) assert.notEqual(after[id].hash, before[id].hash, `${id} lost ONEPIPELINE_TEST_PARTITION`);
  } finally {
    if (previous === undefined) delete env.ONEPIPELINE_TEST_PARTITION;
    else env.ONEPIPELINE_TEST_PARTITION = previous;
    rmSync(dir, { recursive: true, force: true });
  }
});


test("the recorded release hash check survives a later package version", { timeout: 120000 }, () => {
  const dir = fixture();
  try {
    const [major, minor] = ownVersion(dir).split(".").map(Number);
    setOwnVersion(dir, `${major}.${minor + 1}.0`);
    writeFileSync(join(dir, "CHANGELOG.md"), "## A later release\n\n" + readFileSync(join(dir, "CHANGELOG.md"), "utf8"));
    assertRecordedRelease(dir, ["onepipeline-contract:test-quick", "onepipeline-version-contract:test-quick"]);
  } finally { rmSync(dir, { recursive: true, force: true }); }
});

test("every cached CI task hashes exact tool pins and rejects a missing runner input", { timeout: 240000 }, () => {
  const dir = fixture();
  try {
    const selected = new Set();
    for (const job of ["lint", "test-linux", "test-windows", "test-macos"]) {
      const line = run(dir, "bash", ["scripts/ci-job.sh", job, "--plan"]).split("\n").find((line) => line.startsWith("ci-job:"));
      for (const id of line.split("task(s): ")[1].split(" ")) selected.add(id);
    }
    const definitions = new Map(JSON.parse(run(dir, process.execPath, ["node_modules/nx/dist/bin/nx.js", "show", "projects", "--json"])).map((name) => [name, JSON.parse(run(dir, process.execPath, ["node_modules/nx/dist/bin/nx.js", "show", "project", name, "--json"]))]));
    const tasks = [...selected].filter((id) => { const [project, target] = id.split(":"); return definitions.get(project).targets[target].cache === true; });
    const before = capture(dir, tasks);
    const file = ".github/ci-tool-versions.json";
    const guard = (values) => {
      const missing = tasks.filter((id) => !values[id].sourceFiles.includes(file));
      assert.deepEqual(missing, [], `${missing.join(", ")} lost the exact tool-version input`);
    };
    guard(before);
    assert.ok(Object.values(before).every((task) => task.sourceFiles.every((path) => !path.startsWith(".nx/"))), "hash details must not hash themselves");
    const path = join(dir, file), original = readFileSync(path, "utf8"), tools = JSON.parse(original);
    for (const tool of Object.keys(tools)) {
      const version = tools[tool].split(".").map(Number); version[2] += 1;
      writeFileSync(path, JSON.stringify({ ...tools, [tool]: version.join(".") }, null, 2) + "\n");
      const changed = capture(dir, tasks);
      for (const id of tasks) assert.notEqual(changed[id].hash, before[id].hash, `${id} lost ${tool}'s version`);
    }
    writeFileSync(path, original);
    const nxPath = join(dir, "nx.json"), nxText = readFileSync(nxPath, "utf8"), nx = JSON.parse(nxText);
    nx.namedInputs.sharedGlobals = nx.namedInputs.sharedGlobals.filter((input) => input !== `{workspaceRoot}/${file}`);
    writeFileSync(nxPath, JSON.stringify(nx));
    assert.throws(() => guard(capture(dir, tasks)), /lost the exact tool-version input/);
  } finally { rmSync(dir, { recursive: true, force: true }); }
});

test("cached workflow contract journeys hash their budget and figures inputs", { timeout: 120000 }, () => {
  const dir = fixture();
  try {
    const task = "github-workflows:test-ci-shape";
    const before = capture(dir, [task])[task].hash;
    const unchanged = [];
    for (const file of ["budgets.yaml", "scripts/actions-budget-figures.sh"]) {
      const path = join(dir, file);
      const original = readFileSync(path, "utf8");
      // The first changes the registered command the journey asserts; the
      // second must invalidate the upstream-copy byte-parity proof.
      const changed = file === "budgets.yaml"
        ? original.replace("cache-use]", "cache-use-input-probe]")
        : `${original}\n# cache input probe\n`;
      assert.notEqual(changed, original, `${file} probe made no edit`);
      writeFileSync(path, changed);
      const after = capture(dir, [task])[task].hash;
      writeFileSync(path, original);
      if (after === before) unchanged.push(file);
    }
    assert.deepEqual(unchanged, [], "workflow contract checks lost inputs they read");
  } finally { rmSync(dir, { recursive: true, force: true }); }
});


test("cached workflow targets hash their audited transitive workspace inputs", { timeout: 180000 }, () => {
  const dir = fixture();
  try {
    const shape = "github-workflows:test-ci-shape";
    const tasks = [shape, "github-workflows:lint", "github-workflows:format-check"];
    const before = capture(dir, tasks);
    // Each category comes from the real command: plan wrappers, copy-parity
    // configuration, nextest compilation/listing, and the workspace hash audit.
    const files = [
      "scripts/ci-job.sh", "scripts/ci-job-tasks.mjs", "scripts/nx-affected.sh", "scripts/nx.sh",
      "llmlint.yml", "biome.jsonc", "src/lib.rs", "crates/testfakes/src/lib.rs",
      "tests/e2e/publication_boundary.rs", "tests/fixtures/human-items.json",
      "schemas/events.json", "docs/contract.md", ".cargo/config.toml", ".config/nextest.toml",
      "npm/onepipeline-cli/bin/onepipeline.js", "release/release.py", "README.md",
    ];
    const unchanged = [];
    for (const file of files) {
      const path = join(dir, file);
      const original = readFileSync(path);
      writeFileSync(path, Buffer.concat([original, Buffer.from("\n")]));
      const after = capture(dir, tasks);
      writeFileSync(path, original);
      const readers = file === "biome.jsonc" ? tasks : [shape];
      for (const id of readers) if (after[id].hash === before[id].hash) unchanged.push(`${id}: ${file}`);
    }
    assert.deepEqual(unchanged, [], "cached workflow targets lost audited transitive inputs");
  } finally { rmSync(dir, { recursive: true, force: true }); }
});

test("the workspace digest moves with what a proof reads and not with version-handled files", () => {
  const dir = mkdtempSync(join(tmpdir(), "workspace-digest-"));
  try {
    const script = fileURLToPath(new URL("../workspace-proof-input.mjs", import.meta.url));
    const digest = () => run(dir, process.execPath, [script]).trim();
    const write = (path, content) => {
      mkdirSync(dirname(join(dir, path)), { recursive: true });
      writeFileSync(join(dir, path), content);
    };
    run(dir, "git", ["init", "-q"]);
    const versionHandled = [
      "Cargo.toml", "Cargo.lock", "crate/Cargo.toml", "package.json", "npm/package.json",
      "package-lock.json", "pyproject.toml", "CHANGELOG.md", "crate/CHANGELOG.md", "vendor/node_modules/tracked.js",
    ];
    for (const path of [...versionHandled, "src/kept.rs", "crate/pyproject.toml", "gone.txt", "replaced.txt", "holds-deleted.txt"]) write(path, "first\n");
    symlinkSync("src/kept.rs", join(dir, "link"));
    run(dir, "git", ["add", "-A"]);
    let current = digest();
    assert.match(current, /^[0-9a-f]{64}$/);
    for (const path of versionHandled) {
      write(path, "second\n");
      assert.equal(digest(), current, `${path} moved the digest`);
    }
    const moves = (what, change) => {
      change();
      const after = digest();
      assert.notEqual(after, current, `${what} left the digest unmoved`);
      assert.equal(digest(), after, `${what} gave an unstable digest`);
      current = after;
    };
    moves("a source edit", () => write("src/kept.rs", "second\n"));
    moves("a nested pyproject, which no version edit touches", () => write("crate/pyproject.toml", "second\n"));
    moves("deleting a tracked file", () => rmSync(join(dir, "gone.txt")));
    moves("a file holding a deleted file's marker", () => write("holds-deleted.txt", ""));
    moves("retargeting a tracked link", () => { rmSync(join(dir, "link")); symlinkSync("gone.txt", join(dir, "link")); });
    moves("replacing a file with a directory", () => { rmSync(join(dir, "replaced.txt")); write("replaced.txt/inside.txt", "first\n"); });
  } finally { rmSync(dir, { recursive: true, force: true }); }
});

test("a failed workspace digest never yields a replayable key", { timeout: 180000 }, () => {
  const dir = fixture();
  try {
    const task = "github-workflows:test-ci-shape";
    const digest = () => spawnSync(process.execPath, [".github/workspace-proof-input.mjs"], { cwd: dir, encoding: "utf8" });
    mkdirSync(join(dir, "probe"));
    writeFileSync(join(dir, "probe/tracked.txt"), "tracked\n");
    run(dir, "git", ["add", "-A"]);
    const healthy = digest();
    assert.equal(healthy.status, 0, healthy.stderr);
    assert.match(healthy.stdout.trim(), /^[0-9a-f]{64}$/);
    const healthyHash = capture(dir, [task])[task].hash;
    assert.equal(capture(dir, [task])[task].hash, healthyHash, "a healthy tree must keep one key");
    // The index still names the file, and lstat on it now fails with ENOTDIR.
    rmSync(join(dir, "probe"), { recursive: true });
    writeFileSync(join(dir, "probe"), "no longer a directory\n");
    const [first, second] = [digest(), digest()];
    for (const failed of [first, second]) {
      assert.equal(failed.status, 1, failed.stdout);
      assert.doesNotMatch(failed.stdout, /^[0-9a-f]{64}$/m);
      assert.match(failed.stderr, /ACTION: /);
    }
    assert.notEqual(first.stdout, second.stdout);
    assert.notEqual(first.stderr, second.stderr);
    // Real Nx over that failure: no two attempts share a key, so none replays.
    const [one, two] = [capture(dir, [task])[task].hash, capture(dir, [task])[task].hash];
    assert.notEqual(one, two, "a failed digest repeated its key");
    assert.notEqual(one, healthyHash);
  } finally { rmSync(dir, { recursive: true, force: true }); }
});

test("no project a CI job selects names a workspace-wide file input", { timeout: 180000 }, () => {
  const dir = fixture();
  try {
    const script = readFileSync(join(dir, "scripts/ci-job.sh"), "utf8");
    const jobTargets = new Set([...script.matchAll(/targets="([^"]+)"/g)].flatMap((match) => match[1].split(",")));
    assert.ok(jobTargets.has("test-ci-shape"), "scripts/ci-job.sh's target lists were not read");
    const namedInputs = JSON.parse(readFileSync(join(dir, "nx.json"), "utf8")).namedInputs;
    // Nx's affected selection reads every `{workspaceRoot}/` input of every target of a
    // project, selected or not, as "this project is touched when that file changes".
    const expand = (inputs, named) => inputs.flatMap((input) => {
      if (typeof input === "string" && input in named) return expand(named[input], named);
      return [input.fileset ?? input];
    });
    const offenders = () => {
      const nx = (...args) => run(dir, process.execPath, ["node_modules/nx/dist/bin/nx.js", ...args]);
      rmSync(join(dir, ".nx/workspace-data"), { recursive: true, force: true });
      return JSON.parse(nx("show", "projects", "--json")).flatMap((name) => {
        const project = JSON.parse(nx("show", "project", name, "--json"));
        const targets = Object.entries(project.targets ?? {});
        if (!targets.some(([target]) => jobTargets.has(target))) return [];
        const named = { ...namedInputs, ...project.namedInputs };
        return targets.flatMap(([target, definition]) => expand(definition.inputs ?? [], named)
          .filter((input) => /^\{workspaceRoot\}\/\*\*(\/\*)?$/.test(input))
          .map(() => `${name}:${target}`));
      });
    };
    assert.deepEqual(offenders(), []);
    const path = join(dir, ".github/project.json");
    const project = JSON.parse(readFileSync(path, "utf8"));
    project.targets["test-ci-shape"].inputs.unshift("wholeWorkspace");
    writeFileSync(path, JSON.stringify(project));
    assert.deepEqual(offenders(), ["github-workflows:test-ci-shape"], "the assertion must catch the regression");
  } finally { rmSync(dir, { recursive: true, force: true }); }
});
