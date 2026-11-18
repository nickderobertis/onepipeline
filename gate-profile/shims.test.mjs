// The pieces `just gate-profile` puts in a run's path, each driven as that run drives it: the
// phase planner and the `just` shim over a real justfile with the real just; the `cargo` shim in
// front of a stand-in cargo that records the command it was given (what reaches cargo is what is
// under test, not cargo); and scripts/nx.sh's hook in front of a stand-in Nx that reports the
// environment and arguments it got, which is everything the hook decides.
import assert from "node:assert/strict";
import { spawn, spawnSync } from "node:child_process";
import {
  chmodSync,
  cpSync,
  existsSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  realpathSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import test from "node:test";
import { fixtureEnvironment, root } from "./fixture.mjs";

const shims = join(root, "gate-profile", "shims");
// The level these journeys run at, which may itself be inside an Nx task.
const here = process.env.NX_TASK_HASH ?? "";

function scratch(t, name) {
  const dir = realpathSync(
    mkdtempSync(join(process.env.ONEPIPELINE_NODE_SCRATCH_DIR ?? tmpdir(), `${name}-`)),
  );
  t.after(() => rmSync(dir, { recursive: true, force: true }));
  return dir;
}

function write(path, content, mode) {
  mkdirSync(dirname(path), { recursive: true });
  writeFileSync(path, content);
  if (mode) chmodSync(path, mode);
}

const lines = (path) =>
  existsSync(path)
    ? readFileSync(path, "utf8")
        .trim()
        .split("\n")
        .filter(Boolean)
        .map((line) => JSON.parse(line))
    : [];

const JUSTFILE = `set shell := ["bash", "-eu", "-o", "pipefail", "-c"]

base:
    @echo base >> log
m: base
    @echo m >> log
n: base
    @echo n >> log
top: m n
    @echo top >> log
fails:
    @exit 4
after: fails
    @echo after >> log
takes x:
    @echo "takes {{x}}" >> log
given: (takes "1")
    @echo given >> log
then: base && m
    @echo then >> log
[parallel]
fan: m n
    @echo fan >> log
rest *xs:
    @echo "rest {{xs}}" >> log
`;

const plan = (dir, ...args) => {
  const real = spawnSync("bash", ["-c", "command -v just"], { encoding: "utf8" }).stdout.trim();
  return spawnSync(process.execPath, [join(root, "gate-profile", "phases.mjs"), real, ...args], {
    cwd: dir,
    encoding: "utf8",
  });
};

test("the planner orders prior dependencies as just does, and refuses what it cannot split", (t) => {
  const dir = scratch(t, "phases");
  write(join(dir, "justfile"), JUSTFILE);
  const top = plan(dir, "top");
  assert.equal(top.status, 0, top.stderr);
  assert.deepEqual(top.stdout.trim().split("\n"), ["base", "m", "n"]);
  assert.equal(plan(dir, "base").stdout, "");
  assert.equal(plan(dir, "rest", "a", "b", "c").status, 0);
  for (const args of [["given"], ["then"], ["fan"], ["base", "m"], ["nosuch"]])
    assert.equal(plan(dir, ...args).status, 3, args.join(" "));
  const broken = scratch(t, "phases-broken");
  write(join(broken, "justfile"), "this is not a justfile\n");
  const refused = plan(broken, "top");
  assert.equal(refused.status, 2);
  assert.match(refused.stderr, /cannot read the justfile/);
});

function justShim(dir, args, env = {}) {
  return spawnSync(join(shims, "just"), args, {
    cwd: dir,
    encoding: "utf8",
    env: { ...fixtureEnvironment(), ...env },
  });
}

test("the just shim runs a recipe as timed phases only at the run's own level", async (t) => {
  const dir = scratch(t, "just-shim");
  write(join(dir, "justfile"), JUSTFILE);
  const run = join(dir, "run");
  mkdirSync(run);
  const profiled = { ONEPIPELINE_GATE_PROFILE_DIR: run, ONEPIPELINE_GATE_PROFILE_TOP: here };
  const log = () => readFileSync(join(dir, "log"), "utf8").trim().split("\n");
  const reset = () => {
    rmSync(join(dir, "log"), { force: true });
    rmSync(join(run, "phases.jsonl"), { force: true });
    rmSync(join(run, "run.json"), { force: true });
  };

  await t.test("each prior dependency, then the recipe, is a phase of the run", () => {
    reset();
    const result = justShim(dir, ["top"], profiled);
    assert.equal(result.status, 0, result.stderr);
    assert.deepEqual(log(), ["base", "m", "n", "top"]);
    const phases = lines(join(run, "phases.jsonl"));
    assert.deepEqual(
      phases.map((p) => [p.recipe, p.parent, p.exit]),
      [
        ["base", "", 0],
        ["m", "", 0],
        ["n", "", 0],
        ["top", "", 0],
      ],
    );
    for (const p of phases) assert.ok(p.end >= p.start && p.cpu_ms_after[0] >= p.cpu_ms_before[0]);
    assert.equal(JSON.parse(readFileSync(join(run, "run.json"), "utf8")).exit, 0);
  });

  await t.test("a failed dependency stops the phases after it, as just would", () => {
    reset();
    const result = justShim(dir, ["after"], profiled);
    assert.equal(result.status, 4);
    assert.equal(existsSync(join(dir, "log")), false);
    assert.deepEqual(
      lines(join(run, "phases.jsonl")).map((p) => [p.recipe, p.exit]),
      [["fails", 4]],
    );
    assert.equal(JSON.parse(readFileSync(join(run, "run.json"), "utf8")).exit, 4);
  });

  await t.test("a recipe it cannot split runs whole, as one phase", () => {
    reset();
    const result = justShim(dir, ["given"], profiled);
    assert.equal(result.status, 0, result.stderr);
    assert.deepEqual(log(), ["takes 1", "given"]);
    assert.deepEqual(
      lines(join(run, "phases.jsonl")).map((p) => p.recipe),
      ["given"],
    );
  });

  await t.test("inside a task, unprofiled, or for an option it is just itself", () => {
    for (const env of [
      {},
      { ...profiled, NX_TASK_HASH: "a-task-of-the-run" },
      { ONEPIPELINE_GATE_PROFILE_TOP: here },
    ]) {
      reset();
      const result = justShim(dir, ["m"], env);
      assert.equal(result.status, 0, result.stderr);
      assert.deepEqual(log(), ["base", "m"]);
      assert.equal(existsSync(join(run, "phases.jsonl")), false, JSON.stringify(env));
    }
    reset();
    const summary = justShim(dir, ["--summary"], profiled);
    assert.equal(summary.status, 0, summary.stderr);
    assert.match(summary.stdout, /\bbase\b/);
    assert.equal(existsSync(join(run, "phases.jsonl")), false);
  });

  await t.test("a run directory that is not one is refused, not written around", () => {
    reset();
    const result = justShim(dir, ["m"], {
      ...profiled,
      ONEPIPELINE_GATE_PROFILE_DIR: join(dir, "gone"),
    });
    assert.equal(result.status, 2);
    assert.match(result.stderr, /is not a directory/);
  });
});

test("the cargo shim times a build under the root with --timings, and passes all else through", (t) => {
  const dir = scratch(t, "cargo-shim");
  const repo = join(dir, "repo");
  const run = join(dir, "run");
  mkdirSync(repo);
  mkdirSync(run);
  const standin = join(dir, "bin", "cargo");
  write(
    standin,
    `#!/usr/bin/env node
require("node:fs").appendFileSync(process.env.STANDIN_LOG, JSON.stringify(process.argv.slice(2)) + "\\n");
process.exit(Number(process.env.STANDIN_EXIT ?? 0));
`,
    0o755,
  );
  const log = join(dir, "standin.jsonl");
  const env = {
    ...fixtureEnvironment(),
    PATH: `${shims}:${dirname(standin)}:${process.env.PATH}`,
    STANDIN_LOG: log,
    ONEPIPELINE_GATE_PROFILE_DIR: run,
    ONEPIPELINE_GATE_PROFILE_ROOT: repo,
    ONEPIPELINE_GATE_PROFILE_RUN: "run-1",
    NX_TASK_TARGET_PROJECT: "crate",
    NX_TASK_TARGET_TARGET: "lint",
  };
  const cargo = (args, extra = {}, cwd = repo) => {
    rmSync(log, { force: true });
    rmSync(join(run, "cargo.jsonl"), { force: true });
    const result = spawnSync("cargo", args, { cwd, encoding: "utf8", env: { ...env, ...extra } });
    return { status: result.status, got: lines(log)[0], records: lines(join(run, "cargo.jsonl")) };
  };

  const built = [
    [
      ["+stable", "--locked", "build", "-p", "x"],
      ["+stable", "--locked", "build", "--timings", "-p", "x"],
    ],
    [
      ["--color", "never", "check"],
      ["--color", "never", "check", "--timings"],
    ],
    [
      ["clippy", "--all-targets", "--", "-D", "warnings"],
      ["clippy", "--timings", "--all-targets", "--", "-D", "warnings"],
    ],
    [
      ["nextest", "run", "-E", "all()"],
      ["nextest", "run", "--timings", "-E", "all()"],
    ],
    [
      ["llvm-cov", "--no-report", "nextest", "--locked"],
      ["llvm-cov", "--no-report", "nextest", "--timings", "--locked"],
    ],
    [
      ["llvm-cov", "nextest-archive", "--archive-file", "a"],
      ["llvm-cov", "nextest-archive", "--timings", "--archive-file", "a"],
    ],
  ];
  for (const [args, expected] of built) {
    const { status, got, records } = cargo(args);
    assert.equal(status, 0);
    assert.deepEqual(got, expected);
    assert.equal(records.length, 1);
    assert.deepEqual(records[0].argv, args);
    assert.equal(records[0].task, "crate:lint");
    assert.equal(records[0].cwd, repo);
  }

  const untouched = [
    [["llvm-cov", "show-env", "--sh"]],
    [["metadata", "--format-version", "1"]],
    [["nextest", "--version"]],
    [["build"], {}, dir],
    [["build"], { ONEPIPELINE_GATE_PROFILE_CARGO: "run-1" }],
    [["build"], { ONEPIPELINE_GATE_PROFILE_DIR: "" }],
    [["build"], { ONEPIPELINE_GATE_PROFILE_ROOT: "" }],
  ];
  for (const [args, extra, cwd] of untouched) {
    const { status, got, records } = cargo(args, extra, cwd);
    assert.equal(status, 0);
    assert.deepEqual(got, args, JSON.stringify([args, extra, cwd]));
    assert.deepEqual(records, []);
  }

  // cargo's status is the shim's, and the record keeps it; an argument round-trips through the
  // record's JSON, its quotes, backslashes and whitespace escaped and other control bytes dropped.
  const awkward = 'a"b\\c\td\ne\u0001f';
  const failed = cargo(["build", "--features", awkward], { STANDIN_EXIT: "5" });
  assert.equal(failed.status, 5);
  assert.equal(failed.records[0].exit, 5);
  assert.deepEqual(failed.records[0].argv, ["build", "--features", 'a"b\\c\td\nef']);
  assert.deepEqual(failed.got, ["build", "--timings", "--features", awkward]);
});

test("an interrupted cargo build is still recorded, with the interrupt as its status", async (t) => {
  const dir = scratch(t, "cargo-interrupt");
  const run = join(dir, "run");
  mkdirSync(run);
  const standin = join(dir, "bin", "cargo");
  // Like cargo, it dies of the interrupt rather than turning it into an exit status.
  write(standin, '#!/usr/bin/env bash\ntouch "$STANDIN_STARTED"\nexec sleep 60\n', 0o755);
  const started = join(dir, "started");
  const child = spawn(join(shims, "cargo"), ["build"], {
    cwd: dir,
    detached: true,
    stdio: "ignore",
    env: {
      ...fixtureEnvironment(),
      PATH: `${dirname(standin)}:${process.env.PATH}`,
      STANDIN_STARTED: started,
      ONEPIPELINE_GATE_PROFILE_DIR: run,
      ONEPIPELINE_GATE_PROFILE_ROOT: dir,
      ONEPIPELINE_GATE_PROFILE_RUN: "run-1",
    },
  });
  const exited = new Promise((resolve) =>
    child.on("exit", (code, signal) => resolve(code ?? signal)),
  );
  while (!existsSync(started)) await new Promise((resolve) => setTimeout(resolve, 50));
  process.kill(-child.pid, "SIGINT");
  assert.equal(await exited, 130);
  assert.deepEqual(
    lines(join(run, "cargo.jsonl")).map((record) => [record.argv, record.exit]),
    [[["build"], 130]],
  );
});

/** A workspace holding scripts/nx.sh, optionally the profiler, and a stand-in Nx. */
function nxWorkspace(t, { profiler = true } = {}) {
  const dir = scratch(t, "nx-hook");
  mkdirSync(join(dir, "scripts"));
  cpSync(join(root, "scripts", "nx.sh"), join(dir, "scripts", "nx.sh"));
  if (profiler)
    for (const name of ["lib.sh", "nx-hook.sh", "nx-linger.cjs"])
      cpSync(join(root, "gate-profile", name), join(dir, "gate-profile", name), {
        recursive: true,
      });
  // What nx.sh checks for before it would reinstall, so it reaches the stand-in.
  write(join(dir, "node_modules/.bin/onemessagebus"), "#!/bin/sh\n", 0o755);
  write(join(dir, "node_modules/@onebudgetspec/sdk/package.json"), "{}\n");
  write(
    join(dir, "node_modules/.bin/nx"),
    `#!/usr/bin/env node
const { execFileSync } = require("node:child_process");
const names = ["NX_PROFILE", "NX_CACHE_DIRECTORY", "NX_WORKSPACE_DATA_DIRECTORY", "NODE_OPTIONS",
  "ONEPIPELINE_GATE_PROFILE_DIR", "ONEPIPELINE_GATE_PROFILE_INVOCATION", "ONEPIPELINE_GATE_PROFILE_NODE_OPTIONS"];
const env = Object.fromEntries(names.filter((n) => n in process.env).map((n) => [n, process.env[n]]));
const child = execFileSync(process.execPath, ["-p", "process.env.NODE_OPTIONS ?? null"], { encoding: "utf8" }).trim();
console.log(JSON.stringify({ args: process.argv.slice(2), env, child }));
`,
    0o755,
  );
  return dir;
}

function nx(dir, args, env) {
  const result = spawnSync("bash", ["scripts/nx.sh", ...args], {
    cwd: dir,
    encoding: "utf8",
    env: { ...fixtureEnvironment(), ONEPIPELINE_NX_SHOW_OUTPUT: "1", ...env },
  });
  return { ...result, seen: result.status === 0 ? JSON.parse(result.stdout) : null };
}

test("nx.sh's hook profiles a run's own Nx runs and takes itself off nested ones", async (t) => {
  const dir = nxWorkspace(t);
  const run = join(dir, "target", "gate-profile", "x");
  mkdirSync(run, { recursive: true });
  const top = {
    ONEPIPELINE_GATE_PROFILE_DIR: run,
    ONEPIPELINE_GATE_PROFILE_TOP: here,
    NODE_OPTIONS: "--max-old-space-size=300",
  };

  await t.test("unset, it changes nothing", () => {
    const { status, seen, stderr } = nx(dir, ["run-many", "-t", "lint"], {});
    assert.equal(status, 0, stderr);
    assert.deepEqual(seen.args, ["run-many", "-t", "lint"]);
    assert.deepEqual(seen.env, {});
  });

  await t.test("at the run's level a task run gets its own profile and this run's cache", () => {
    const { status, seen, stderr } = nx(dir, ["run-many", "-t", "lint"], top);
    assert.equal(status, 0, stderr);
    assert.deepEqual(seen.args, ["run-many", "-t", "lint", "--skip-remote-cache"]);
    const id = seen.env.ONEPIPELINE_GATE_PROFILE_INVOCATION;
    assert.equal(seen.env.NX_PROFILE, `target/gate-profile/x/nx/${id}.profile.json`);
    assert.equal(seen.env.NX_CACHE_DIRECTORY, join(run, "nx-state", "cache"));
    assert.equal(seen.env.NX_WORKSPACE_DATA_DIRECTORY, join(run, "nx-state", "data"));
    // The preload put NODE_OPTIONS back as it was, so a process Nx starts does not load it.
    assert.equal(seen.env.NODE_OPTIONS, "--max-old-space-size=300");
    assert.equal(seen.child, "--max-old-space-size=300");
    assert.equal("ONEPIPELINE_GATE_PROFILE_NODE_OPTIONS" in seen.env, false);
    const record = JSON.parse(readFileSync(join(run, "nx", `${id}.invocation.json`), "utf8"));
    assert.deepEqual(record.args, ["run-many", "-t", "lint"]);
  });

  await t.test("an Nx command that runs no tasks is left alone", () => {
    const { seen } = nx(dir, ["show", "projects"], top);
    assert.deepEqual(seen.args, ["show", "projects"]);
    assert.equal("NX_PROFILE" in seen.env, false);
  });

  await t.test(
    "inside one of the run's tasks, the run's settings are taken off, others kept",
    () => {
      const { status, seen, stderr } = nx(dir, ["run-many", "-t", "test"], {
        ...top,
        NX_TASK_HASH: "a-task-of-the-run",
        NX_PROFILE: "target/gate-profile/x/nx/outer.profile.json",
        NX_CACHE_DIRECTORY: join(run, "nx-state", "cache"),
        NX_WORKSPACE_DATA_DIRECTORY: join(dir, "elsewhere"),
      });
      assert.equal(status, 0, stderr);
      assert.deepEqual(seen.args, ["run-many", "-t", "test"]);
      assert.deepEqual(seen.env, {
        NX_WORKSPACE_DATA_DIRECTORY: join(dir, "elsewhere"),
        NODE_OPTIONS: "--max-old-space-size=300",
      });
    },
  );

  await t.test("a run directory that is not one is refused", () => {
    const { status, stderr } = nx(dir, ["run-many", "-t", "lint"], {
      ...top,
      ONEPIPELINE_GATE_PROFILE_DIR: join(dir, "gone"),
    });
    assert.equal(status, 2);
    assert.match(stderr, /is not a directory/);
  });

  await t.test("a copy of nx.sh without the profiler drops every setting of the run", () => {
    const bare = nxWorkspace(t, { profiler: false });
    const { status, seen, stderr } = nx(bare, ["run-many", "-t", "test"], {
      ONEPIPELINE_GATE_PROFILE_DIR: run,
      NX_PROFILE: "target/gate-profile/x/nx/outer.profile.json",
      NX_CACHE_DIRECTORY: join(run, "nx-state", "cache"),
      NX_WORKSPACE_DATA_DIRECTORY: join(run, "nx-state", "data"),
    });
    assert.equal(status, 0, stderr);
    assert.deepEqual(seen.env, {});
  });
});
