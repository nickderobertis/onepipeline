// The CI time-profile spike harness, driven as its users run it: `ci-time-workflow.mjs`
// over the repository's own ci.yml, and `ci-time-profile.mjs job` over a hosted run held
// in its response cache, which is how a second measurement of a run reads it, so the run
// is read with no GitHub request.
import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { test } from "node:test";
import { fileURLToPath } from "node:url";

const root = join(dirname(fileURLToPath(import.meta.url)), "..", "..");
const workflowScript = join(root, "scripts", "ci-time-workflow.mjs");
const profileScript = join(root, "scripts", "ci-time-profile.mjs");
const run = (script, args) =>
  spawnSync(process.execPath, [script, ...args], { cwd: root, encoding: "utf8" });

function scratch(t) {
  const dir = mkdtempSync(join(tmpdir(), "ci-time-profile-"));
  t.after(() => rmSync(dir, { recursive: true, force: true }));
  return dir;
}

test("the workflow edits turn every cache write off, add the observers, and are the same every time", (t) => {
  const dir = scratch(t);
  const original = readFileSync(join(root, ".github", "workflows", "ci.yml"), "utf8");
  const a = join(dir, "a.yml");
  const b = join(dir, "b.yml");
  writeFileSync(a, original);
  writeFileSync(b, original);
  for (const file of [a, b]) {
    const result = run(workflowScript, [file, "--overhead-sample", "hash:1/6"]);
    assert.equal(result.status, 0, result.stderr);
  }
  const edited = readFileSync(a, "utf8");
  assert.equal(edited, readFileSync(b, "utf8"), "one input gives one output");
  assert.doesNotMatch(edited, /CACHE_READ_ONLY: \$\{\{/);
  assert.equal(edited.match(/CACHE_READ_ONLY: '1'/g).length, 4);
  assert.doesNotMatch(edited, /enable-cache: \$\{\{/);
  assert.doesNotMatch(edited, /save-if: (?!false)/);
  assert.match(edited, /VISUAL_DOCS_PUBLISH: "false"/);
  for (const job of ["test-linux", "test-macos"]) {
    const block = edited.slice(edited.indexOf(`\n  ${job}:\n`));
    assert.match(block, /RUSTC_WRAPPER=\$m\/rustc-timer/);
    assert.match(block, /NX_PROFILE=target\/ci-time-nx-profile\.json/);
    assert.match(block, /name: measure-\$\{\{ github\.job \}\}/);
  }
  assert.equal(edited.match(/one e2e partition instrumented, then plain/g).length, 1);
  // The rust-cache key is computed before the wrapper is set: the timer step comes after it.
  const linux = edited.slice(edited.indexOf("\n  test-linux:\n"));
  assert.ok(linux.indexOf("Swatinem/rust-cache") < linux.indexOf("rustc-timer"));

  const again = run(workflowScript, [a]);
  assert.equal(again.status, 1);
  assert.match(again.stderr, /already carries the measurement edits/);
});

test("a workflow not shaped as the edits expect is refused and left as it was", (t) => {
  const dir = scratch(t);
  const file = join(dir, "ci.yml");
  writeFileSync(file, "name: CI\njobs: {}\n");
  const result = run(workflowScript, [file]);
  assert.equal(result.status, 1);
  assert.match(result.stderr, /expected 4 of/);
  assert.equal(readFileSync(file, "utf8"), "name: CI\njobs: {}\n");
  assert.equal(run(workflowScript, []).status, 2);
});

const REPO = "example/onepipeline";
const RUN = 42;
const T0 = Date.parse("2026-11-13T10:00:00Z");
const iso = (s) => new Date(T0 + s * 1000).toISOString();
const key = (endpoint) => `${endpoint.replace(/[^\w.-]+/g, "_")}.txt`;

function record(name, start, wall, parts) {
  const category = (s, extra = {}) => ({
    count: 1,
    s,
    inclusive_s: s,
    background_count: 0,
    background_s: 0,
    ...extra,
  });
  return {
    schema: 2,
    binary: "e2e",
    test: name,
    run_id: "nextest-run",
    attempt: 1,
    os: "linux",
    arch: "x86_64",
    started_at: (T0 + start * 1000) / 1000,
    wall_s: wall,
    ended: "test-thread",
    categories: {
      spawn: category(parts.spawn),
      git: category(parts.git),
      build: category(0),
      fixture: category(0),
      wait: category(parts.wait, { polls: 3 }),
      fs: category(0),
    },
    unattributed_s: wall - parts.spawn - parts.git - parts.wait,
    programs: { onepipeline: { count: 4, s: parts.spawn }, git: { count: 2, s: parts.git } },
  };
}

// A finished test-linux job: 60 s of setup, a 30 s rust-cache restore, a 600 s gate holding
// a build task and the e2e tier, and a 10 s upload.
function cacheOneRun(cache) {
  const api = join(cache, "api");
  mkdirSync(api, { recursive: true });
  const put = (endpoint, body) =>
    writeFileSync(join(api, key(endpoint)), typeof body === "string" ? body : JSON.stringify(body));
  put(`repos/${REPO}/actions/runs/${RUN}`, {
    status: "completed",
    head_sha: "a".repeat(40),
    head_branch: "demo",
    pull_requests: [{ number: 7 }],
  });
  const steps = [
    ["Set up job", 0, 60],
    ["Run Swatinem/rust-cache@v2.9.2", 60, 90],
    [
      "Tiers, MSRV, smoke, pinned-wheel journeys, install proofs and wheel (affected projects)",
      90,
      690,
    ],
    ["Keep the per-test profile records", 690, 700],
  ];
  put(`repos/${REPO}/actions/runs/${RUN}/jobs?per_page=100`, {
    jobs: [
      {
        id: 9,
        name: "test-linux",
        run_attempt: 1,
        runner_name: "runner",
        labels: ["ubuntu-24.04"],
        conclusion: "success",
        started_at: iso(0),
        completed_at: iso(700),
        steps: steps.map(([name, a, b], i) => ({
          name,
          number: i + 1,
          conclusion: "success",
          started_at: iso(a),
          completed_at: iso(b),
        })),
      },
    ],
  });
  put(
    `repos/${REPO}/actions/jobs/9/logs`,
    [
      `${iso(5)} [command]/usr/bin/git log -1 --format=%H`,
      `${iso(5)} ${"b".repeat(40)}`,
      `${iso(70)} Restored from cache key "v0-rust-test-linux" full match: true.`,
      `${iso(91)} ci-job: test-linux runs 2 task(s): demo:build demo-e2e:test`,
      `${iso(400)} Starting 2 tests across 1 binary`,
      `${iso(600)} \u001b[32mSummary\u001b[0m [ 200.000s] 2 tests run: 2 passed, 0 skipped`,
      `${iso(689)}   Critical path:     8m 0s (2 tasks)`,
    ].join("\n"),
  );
  put(`repos/${REPO}/actions/runs/${RUN}/artifacts?per_page=100`, {
    artifacts: ["test-profile-test-linux", "nx-cache-entries-test-linux", "measure-test-linux"].map(
      (name, id) => ({ id, name, expired: false }),
    ),
  });
  const artifact = (name) => {
    const dir = join(cache, "runs", String(RUN), "test-linux", name);
    mkdirSync(dir, { recursive: true });
    writeFileSync(join(dir, ".complete"), "");
    return dir;
  };
  const records = join(artifact("test-profile-test-linux"), "e2e");
  mkdirSync(records);
  writeFileSync(
    join(records, "a.json"),
    JSON.stringify(record("a", 400, 200, { spawn: 120, git: 20, wait: 50 })),
  );
  writeFileSync(
    join(records, "b.json"),
    JSON.stringify(record("b", 400, 100, { spawn: 40, git: 0, wait: 60 })),
  );
  writeFileSync(
    join(records, "late.json"),
    JSON.stringify(record("late", 683, 3, { spawn: 3, git: 0, wait: 0 })),
  );
  writeFileSync(
    join(artifact("nx-cache-entries-test-linux"), "executions.json"),
    JSON.stringify({
      tasks: [
        { task: "demo:build", result: "executed", started_at: iso(100), finished_at: iso(450) },
        { task: "demo-e2e:test", result: "executed", started_at: iso(300), finished_at: iso(680) },
        // Another task runs one more test of the e2e binary after the tier has ended.
        {
          task: "demo-version:smoke",
          result: "executed",
          started_at: iso(682),
          finished_at: iso(688),
        },
      ],
      store_attempts: [{ status: "failed", http_status: 403 }],
    }),
  );
  writeFileSync(
    join(artifact("nx-cache-entries-test-linux"), "manifest.json"),
    JSON.stringify({ entries: [], stored_bytes: 0 }),
  );
  const measure = artifact("measure-test-linux");
  const rustc = (a, b, crate, dir) =>
    [
      (T0 + a * 1000) / 1000,
      (T0 + b * 1000) / 1000,
      crate,
      "lib",
      dir.includes("llvm-cov") ? 1 : 0,
      `/home/runner/work/onepipeline/onepipeline/target/${dir}/deps`,
      0,
      "/home/runner/.rustup/toolchains/1.97.1-x86_64-unknown-linux-gnu/bin/rustc",
    ].join("\t");
  writeFileSync(
    join(measure, "rustc.tsv"),
    [
      rustc(100, 250, "onepipeline", "debug"),
      rustc(300, 380, "onepipeline", "llvm-cov-target/debug"),
    ].join("\n"),
  );
}

test("one job's wall clock is divided into steps, Nx tasks, compiles and test categories that sum to it, from the cache alone", (t) => {
  const cache = scratch(t);
  cacheOneRun(cache);
  const json = join(cache, "out.json");
  const result = run(profileScript, [
    "job",
    "--run",
    String(RUN),
    "--job",
    "test-linux",
    "--repo",
    REPO,
    "--cache",
    cache,
    "--json",
    json,
  ]);
  assert.equal(result.status, 0, result.stderr);
  const out = result.stdout;
  assert.match(out, /wall clock 11\.7 min/);
  assert.match(out, new RegExp(`GITHUB_SHA ${"b".repeat(40)}`));
  assert.match(out, /Nx selection: 2 task\(s\): demo:build demo-e2e:test/);
  assert.match(out, /Critical path 8m 0s \(2 tasks\)/);
  assert.match(out, /Restored from cache key "v0-rust-test-linux" full match: true/);
  assert.match(out, /Nx store attempts by status\/http: \{"failed\/403":1\}/);
  assert.match(out, /GitHub requests this invocation: 0/);

  const r = JSON.parse(readFileSync(json, "utf8"));
  assert.equal(r.wall_s, 700);
  assert.equal(r.step_classes["gate (Nx tasks)"], 600);
  assert.equal(r.step_classes["cache restore"], 30);
  // The gate's categories account for each of its 600 seconds exactly once.
  const gate = Object.values(r.gate_categories).reduce((a, b) => a + b, 0);
  assert.ok(Math.abs(gate - 600) < 1e-6, `gate categories sum to ${gate}`);
  assert.ok(Math.abs(r.gate_categories["gate: Nx planning before the first task"] - 10) < 1e-6);
  assert.ok(Math.abs(r.gate_categories["compile: plain (target/debug)"] - 150) < 1e-6);
  // From 300 s to 450 s the build and the e2e tier overlap, so each second is half each's.
  // The instrumented rustc (300-380 s) is both tasks' compile: 40 + 40 s.
  assert.ok(Math.abs(r.gate_categories["compile: instrumented (llvm-cov-target)"] - 80) < 1e-6);
  assert.ok(Math.abs(r.gate_categories["task (no test records): demo:build"] - (50 + 35)) < 1e-6);
  assert.ok(
    Math.abs(
      r.gate_categories["task: before its first test, no rustc running (cargo, nextest, archive)"] -
        // e2e's 380-400 s at half, and the version task's 682-683 s.
        11,
    ) < 1e-6,
  );
  // The tests: 400-450 s both tests in half the second (a quarter each), 450-500 s both in the
  // whole (a half each), 500-600 s test a alone. So a has 137.5 s, split 120/20/50/10 of its
  // 200 s record, and b 37.5 s, split 40/0/60/0 of its 100 s.
  const spawn = r.gate_categories["test: spawn: onepipeline"];
  // Plus the version task's late test: 3 s of spawn, alone, 683-686 s.
  assert.ok(Math.abs(spawn - (137.5 * 120) / 200 - (37.5 * 40) / 100 - 3) < 1e-6, `spawn ${spawn}`);
  assert.ok(
    Math.abs(r.gate_categories["task: after its last test (profdata merge, LCOV export)"] - 80) <
      1e-6,
  );
  assert.ok(Math.abs(r.gate_categories["task: after its last test"] - 2) < 1e-6);
  assert.deepEqual(
    r.binaries.map((b) => [b.binary, b.tests, b.task]),
    [["e2e", 3, "demo-e2e:test"]],
  );
  // The later test is the other task's: the tier's last test, and so its export, stay put.
  assert.deepEqual(
    r.tasks.map((x) => [x.task, x.tests]),
    [
      ["demo:build", 0],
      ["demo-e2e:test", 2],
      ["demo-version:smoke", 1],
    ],
  );
  assert.equal(r.slowest[0].test, "e2e::a");
  assert.deepEqual(
    r.nextest_runs.map((n) => [n.tests, n.s, n.tasks]),
    [[2, 200, ["demo:build", "demo-e2e:test"]]],
  );
  assert.deepEqual(r.concurrency_profile, { 0: 14, 1: 436, 2: 150 });
});

test("a run without the named job is refused without guessing", (t) => {
  const cache = scratch(t);
  cacheOneRun(cache);
  const result = run(profileScript, [
    "job",
    "--run",
    String(RUN),
    "--job",
    "test-macos",
    "--repo",
    REPO,
    "--cache",
    cache,
  ]);
  assert.equal(result.status, 1);
  assert.match(result.stderr, /run 42 has no job test-macos/);
  assert.equal(run(profileScript, ["job", "--run", "x", "--job", "test-linux"]).status, 2);
  assert.equal(run(profileScript, ["nonsense"]).status, 2);
});
