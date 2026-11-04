import assert from "node:assert/strict";
import { execFileSync, spawn, spawnSync } from "node:child_process";
import { copyFileSync, existsSync, mkdirSync, readdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { test } from "node:test";

const script = fileURLToPath(new URL("../../scripts/ensure-test-archive.sh", import.meta.url));
test("a tier miss builds a real nextest archive, reuses it, and recovers after a failed build", { timeout: 120000 }, () => {
  const dir = mkdtempSync(join(process.env.ONEPIPELINE_NODE_SCRATCH_DIR ?? tmpdir(), "lazy-archive-"));
  try {
    copyFileSync(new URL("../../justfile", import.meta.url), join(dir, "justfile"));
    mkdirSync(join(dir, "src"));
    writeFileSync(join(dir, "Cargo.toml"), '[package]\nname = "archive-journey"\nversion = "0.1.0"\nedition = "2021"\n');
    const source = '#[test]\nfn archived_test_runs() { assert_eq!(2 + 2, 4); }\n';
    writeFileSync(join(dir, "src/lib.rs"), "compile_error!(\"build failure under test\");\n");
    const archive = join(dir, "tests.tar.zst");
    const env = { ...process.env, CARGO_TARGET_DIR: join(dir, "target"), ONEPIPELINE_TEST_ARCHIVE: archive };
    execFileSync("cargo", ["generate-lockfile", "--offline"], { cwd: dir, env });
    const invalid = spawnSync("bash", [script, "invalid"], { cwd: dir, env, encoding: "utf8" });
    assert.equal(invalid.status, 2);
    assert.match(invalid.stderr, /use coverage or quick/);
    const invoke = () => spawnSync("bash", [script, "quick"], { cwd: dir, env, encoding: "utf8" });
    const failed = invoke();
    assert.notEqual(failed.status, 0);
    assert.match(failed.stderr, /build failure under test/);
    assert.equal(existsSync(archive), false);
    assert.equal(existsSync(`${archive}.lock`), false);
    writeFileSync(join(dir, "src/lib.rs"), source);
    const built = invoke();
    assert.equal(built.status, 0, built.stderr);
    const bytes = readFileSync(archive);
    assert.ok(bytes.length > 0);
    const replay = invoke();
    assert.equal(replay.status, 0, replay.stderr);
    assert.equal(replay.stderr, "");
    assert.deepEqual(readFileSync(archive), bytes);
    const ran = spawnSync("cargo", ["nextest", "run", "--archive-file", archive, "--workspace-remap", dir], { cwd: dir, env, encoding: "utf8" });
    assert.equal(ran.status, 0, ran.stderr);
    assert.match(ran.stderr, /archived_test_runs/);
  } finally { rmSync(dir, { recursive: true, force: true }); }
});

test("real Nx runs no archive builder when every selected tier replays", { timeout: 120000 }, async () => {
  const { workspace, command } = await import("./support/cache-workspace.mjs");
  const ws = workspace();
  try {
    const env = { NX_DAEMON: "false", NX_ISOLATE_PLUGINS: "false", NX_SKIP_REMOTE_CACHE: "true", CI: "true",
      ONEPIPELINE_TEST_ARCHIVE: join(ws.dir, "never-built.tar.zst") };
    const seeded = await command(ws.dir, process.execPath, [".github/ci-shape/support/seed-tier-cache.mjs"], env);
    assert.equal(seeded.code, 0, seeded.stdout + seeded.stderr);
    const projects = JSON.parse(seeded.stdout.trim().split("\n").at(-1));
    const run = await command(ws.dir, process.execPath, ["node_modules/nx/dist/bin/nx.js", "run-many", "--targets=test,test-quick", `--projects=${projects.join(",")}`, "--outputStyle=static"], env);
    assert.equal(run.code, 0, run.stdout + run.stderr);
    assert.match(run.stdout, /read the output from the cache/);
    assert.doesNotMatch(run.stdout + run.stderr, /onepipeline:test-(?:quick-)?archive|just _crate-test-(?:quick-)?archive|Creating archive/);
    assert.equal(existsSync(env.ONEPIPELINE_TEST_ARCHIVE), false);
  } finally { ws.close(); }
});

const pause = () => new Promise((resolve) => setTimeout(resolve, 20));
async function until(predicate) {
  const deadline = Date.now() + 20000;
  while (!predicate()) {
    assert.ok(Date.now() < deadline, "archive build did not reach its barrier");
    await pause();
  }
}
function builder(dir, env, mode = "quick") {
  const child = spawn("bash", [script, mode], { cwd: dir, env });
  let log = "";
  child.stdout.on("data", (d) => { log += d; });
  child.stderr.on("data", (d) => { log += d; });
  const done = new Promise((resolve, reject) => {
    child.on("error", reject);
    child.on("close", (code, signal) => resolve({ code, signal, log }));
  });
  return { child, done };
}
for (const scenario of ["successful holder", "failed holder", "SIGTERM", "SIGINT"]) {
  test(`competing archive callers recover after a ${scenario}`, { timeout: 60000 }, async () => {
    const dir = mkdtempSync(join(process.env.ONEPIPELINE_NODE_SCRATCH_DIR, "archive-contention-"));
    const owned = [];
    try {
      copyFileSync(new URL("../../justfile", import.meta.url), join(dir, "justfile"));
      mkdirSync(join(dir, "src"));
      writeFileSync(join(dir, "Cargo.toml"), '[package]\nname="archive-contention"\nversion="0.1.0"\nedition="2021"\n');
      writeFileSync(join(dir, "src/lib.rs"), '#[test] fn from_archive() { assert!(true); }\n');
      writeFileSync(join(dir, "build.rs"), `fn main() {
        let root = std::path::PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
        std::fs::write(root.join("entered"), "ready").unwrap();
        while !root.join("continue").exists() { std::thread::sleep(std::time::Duration::from_millis(10)); }
        if root.join("fail-once").exists() { std::fs::remove_file(root.join("fail-once")).unwrap(); panic!("first builder fails"); }
      }\n`);
      if (scenario === "failed holder") writeFileSync(join(dir, "fail-once"), "");
      const archive = join(dir, "archive.tar.zst");
      const env = { ...process.env, CARGO_TARGET_DIR: join(dir, "target"), ONEPIPELINE_TEST_ARCHIVE: archive };
      execFileSync("cargo", ["generate-lockfile", "--offline"], { cwd: dir, env });
      const first = builder(dir, env); owned.push(first);
      await until(() => existsSync(join(dir, "entered")));
      const second = builder(dir, env); owned.push(second);
      await pause();
      assert.equal(second.child.exitCode, null);
      assert.equal(existsSync(archive), false);
      if (scenario.startsWith("SIG")) assert.equal(first.child.kill(scenario), true);
      writeFileSync(join(dir, "continue"), "go");
      const [a, b] = await Promise.all([first.done, second.done]);
      assert.equal(a.code, scenario === "SIGTERM" ? 143 : scenario === "SIGINT" ? 130 : scenario === "failed holder" ? 101 : 0, a.log);
      assert.equal(b.code, 0, b.log);
      assert.ok(existsSync(archive));
      assert.equal(existsSync(`${archive}.lock`), false);
      const ran = spawnSync("cargo", ["nextest", "run", "--archive-file", archive, "--workspace-remap", dir], { cwd: dir, env, encoding: "utf8" });
      assert.equal(ran.status, 0, ran.stderr);
      assert.match(ran.stderr, /from_archive/);
    } finally {
      // Release only this invocation's build barrier, then await every child
      // we started before removing its workspace, including on assertion failure.
      writeFileSync(join(dir, "continue"), "go");
      await Promise.all(owned.map((run) => run.done));
      rmSync(dir, { recursive: true, force: true });
    }
  });
}

test("coverage mode archives and executes the real instrumented test", { timeout: 120000 }, () => {
  const dir = mkdtempSync(join(process.env.ONEPIPELINE_NODE_SCRATCH_DIR, "archive-coverage-"));
  try {
    copyFileSync(new URL("../../justfile", import.meta.url), join(dir, "justfile"));
    mkdirSync(join(dir, "src"));
    writeFileSync(join(dir, "Cargo.toml"), '[package]\nname="coverage-archive"\nversion="0.1.0"\nedition="2021"\n');
    writeFileSync(join(dir, "src/lib.rs"), '#[test] fn instrumented() { assert_eq!(2+2,4); }\n');
    const archive = join(dir, "instrumented.tar.zst");
    const env = { ...process.env, CARGO_TARGET_DIR: join(dir, "target"), ONEPIPELINE_TEST_ARCHIVE: archive, LLVM_PROFILE_FILE: join(dir, "test-%p.profraw") };
    execFileSync("cargo", ["generate-lockfile", "--offline"], { cwd: dir, env });
    const built = spawnSync("bash", [script, "coverage"], { cwd: dir, env, encoding: "utf8" });
    assert.equal(built.status, 0, built.stderr);
    assert.ok(existsSync(archive));
    assert.equal(readdirSync(dir).filter((p) => p.endsWith(".profraw")).length, 0);
    const ran = spawnSync("cargo", ["nextest", "run", "--archive-file", archive, "--workspace-remap", dir], { cwd: dir, env, encoding: "utf8" });
    assert.equal(ran.status, 0, ran.stderr);
    assert.match(ran.stderr, /instrumented/);
    assert.ok(readdirSync(dir).some((p) => p.endsWith(".profraw")));
  } finally { rmSync(dir, { recursive: true, force: true }); }
});
