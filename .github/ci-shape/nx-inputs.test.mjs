import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { cpSync, existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, symlinkSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { test } from "node:test";

const root = fileURLToPath(new URL("../../", import.meta.url));
const helper = fileURLToPath(new URL("./support/nx-hashes.mjs", import.meta.url));
const env = { ...process.env, NX_DAEMON: "false", CI: "true" };
for (const key of Object.keys(env)) if (key.startsWith("GITHUB_")) delete env[key];
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
  const out = join(dir, "hashes.json");
  run(dir, process.execPath, [helper, "capture", dir, ...tasks, "--out", out]);
  return JSON.parse(readFileSync(out, "utf8")).tasks;
}
const versionReaders = new Set([
  "onepipeline-npm:test", "onepipeline-stop-verdict:test-build",
  ...["contract", "e2e", "package"].flatMap((name) => ["test", "test-quick"].map((target) => `onepipeline-version-${name}:${target}`)),
]);

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
    assert.ok(tasks.length >= 63, `only ${tasks.length} cacheable tasks were exercised`);
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
    const extra = [...versionReaders].filter((id) => id.startsWith("onepipeline-version-"));
    const now = capture(current, [...new Set([...cached, ...extra])]);
    // Reproduce release-plz PR #1387's exact three-file edit, rather than
    // approximating its changelog layout. This is the recorded release
    // 0.98.4 -> 0.99.0 at the pinned starting revision.
    const releaseFiles = ["Cargo.toml", "Cargo.lock", "CHANGELOG.md"];
    for (const file of releaseFiles) writeFileSync(join(current, file), run(root, "git", ["show", `96c876ab^:${file}`]));
    const priorRelease = capture(current, [...new Set([...cached, ...extra])]);
    for (const id of [...cached, ...extra]) {
      if (!now[id].cacheable) continue; // Stop-verdict is deliberately uncached.
      if (versionReaders.has(id)) assert.notEqual(priorRelease[id].hash, now[id].hash, `${id} lost recorded-release version input`);
      else assert.equal(priorRelease[id].hash, now[id].hash, `${id} moved on recorded release-plz PR #1387`);
    }
    for (const file of releaseFiles) writeFileSync(join(current, file), readFileSync(join(root, file)));
    for (const id of cached) {
      const union = new Set([...now[id].sourceFiles, ...now[id].runtimeFiles]);
      const split = id.startsWith("onepipeline-contract:") ? "contract" : id.startsWith("onepipeline-e2e:") ? "e2e" : id.startsWith("onepipeline:") ? "package" : null;
      if (split) for (const target of ["test", "test-quick"]) {
        const moved = now[`onepipeline-version-${split}:${target}`];
        for (const file of [...moved.sourceFiles, ...moved.runtimeFiles]) union.add(file);
      }
      for (const file of originals[id].sourceFiles) {
        if (file === "CHANGELOG.md" && !versionReaders.has(id)) continue;
        assert.ok(union.has(file === "biome.json" ? "biome.jsonc" : file), `${id} lost the input ${file}`);
      }
    }
  } finally {
    rmSync(base, { recursive: true, force: true });
    rmSync(current, { recursive: true, force: true });
  }
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
