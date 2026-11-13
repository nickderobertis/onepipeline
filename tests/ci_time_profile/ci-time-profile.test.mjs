// The CI time-profile spike harness, driven as its users run it. `ci-time-workflow.mjs`
// runs over the repository's own ci.yml and ci-job.sh, and the rustc timer and sampler it
// generates are executed for real. `ci-time-profile.mjs` runs against a stand-in `gh` on
// PATH that serves a finished hosted run (its rate floor, first reads, cache reuse and an
// unfinished run), and over a run held in its cache whose Nx trace Nx's own profiling
// life cycle writes and whose records start from the recorder's golden record.
import assert from "node:assert/strict";
import { execFileSync, spawnSync } from "node:child_process";
import {
  chmodSync,
  copyFileSync,
  existsSync,
  mkdirSync,
  mkdtempSync,
  readdirSync,
  readFileSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { createRequire } from "node:module";
import { tmpdir } from "node:os";
import { dirname, join, relative } from "node:path";
import { test } from "node:test";
import { fileURLToPath } from "node:url";
import { SAMPLER, TIMER } from "../../scripts/ci-time-workflow.mjs";

const root = join(dirname(fileURLToPath(import.meta.url)), "..", "..");
const workflowScript = join(root, "scripts", "ci-time-workflow.mjs");
const profileScript = join(root, "scripts", "ci-time-profile.mjs");
const run = (script, args, options = {}) =>
  spawnSync(process.execPath, [script, ...args], { cwd: root, encoding: "utf8", ...options });

function scratch(t) {
  const dir = mkdtempSync(join(tmpdir(), "ci-time-profile-"));
  t.after(() => rmSync(dir, { recursive: true, force: true }));
  return dir;
}

// A checkout holding the repository's own ci.yml and ci-job.sh, as the patcher reads them.
function checkout(dir) {
  mkdirSync(join(dir, ".github", "workflows"), { recursive: true });
  mkdirSync(join(dir, "scripts"), { recursive: true });
  copyFileSync(
    join(root, ".github", "workflows", "ci.yml"),
    join(dir, ".github", "workflows", "ci.yml"),
  );
  copyFileSync(join(root, "scripts", "ci-job.sh"), join(dir, "scripts", "ci-job.sh"));
  return join(dir, ".github", "workflows", "ci.yml");
}

test("the workflow edits turn every cache write off, add the observers, and are the same every time", (t) => {
  const a = checkout(join(scratch(t), "a"));
  const b = checkout(join(scratch(t), "b"));
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
  const ciJob = readFileSync(join(root, "scripts", "ci-job.sh"), "utf8");
  // The case line that names the job: `test-linux)` or `test-windows | test-macos)`.
  const targets = (job) =>
    new RegExp(`^[^)\\n]*\\b${job}\\b[^)\\n]*\\) targets="([^"]+)"`, "m").exec(ciJob)[1];
  for (const job of ["test-linux", "test-macos"]) {
    const block = edited.slice(edited.indexOf(`\n  ${job}:\n`));
    assert.match(block, /RUSTC_WRAPPER=\$m\/rustc-timer/);
    assert.match(block, /NX_PROFILE=target\/ci-time-nx-profile\.json/);
    assert.match(block, /name: measure-\$\{\{ github\.job \}\}/);
    // The graph step plans exactly the targets ci-job.sh runs for the job.
    assert.ok(block.includes(`-t "${targets(job)}" --graph=stdout`), job);
  }
  assert.equal(edited.match(/one e2e partition instrumented, then plain/g).length, 1);
  // The rust-cache key is computed before the wrapper is set: the timer step comes after it.
  const linux = edited.slice(edited.indexOf("\n  test-linux:\n"));
  assert.ok(linux.indexOf("Swatinem/rust-cache") < linux.indexOf("rustc-timer"));

  const plain = checkout(join(scratch(t), "plain"));
  assert.equal(run(workflowScript, [plain]).status, 0);
  assert.doesNotMatch(readFileSync(plain, "utf8"), /one e2e partition/);

  const again = run(workflowScript, [a]);
  assert.equal(again.status, 1);
  assert.match(again.stderr, /already carries the measurement edits\nACTION: /);
});

test("a workflow or checkout not shaped as the edits expect is refused and left as it was", (t) => {
  const file = checkout(scratch(t));
  writeFileSync(file, "name: CI\njobs: {}\n");
  const shape = run(workflowScript, [file]);
  assert.equal(shape.status, 1);
  assert.match(shape.stderr, /expected 4 of .*\nACTION: /);
  assert.equal(readFileSync(file, "utf8"), "name: CI\njobs: {}\n");

  const saving = checkout(join(scratch(t), "saving"));
  const before = readFileSync(saving, "utf8").replace("save-if: false", "save-if: true");
  writeFileSync(saving, before);
  const refused = run(workflowScript, [saving]);
  assert.equal(refused.status, 1);
  assert.match(refused.stderr, /can save an Actions cache/);
  assert.equal(readFileSync(saving, "utf8"), before);

  const noTargets = checkout(join(scratch(t), "no-targets"));
  writeFileSync(join(dirname(noTargets), "..", "..", "scripts", "ci-job.sh"), "#!/bin/sh\n");
  const missing = run(workflowScript, [noTargets]);
  assert.equal(missing.status, 1);
  assert.match(missing.stderr, /names no targets for test-linux/);

  const loose = checkout(join(scratch(t), "loose"));
  writeFileSync(join(dirname(loose), "..", "..", "scripts", "ci-job.sh"), "");
  rmSync(join(dirname(loose), "..", "..", "scripts", "ci-job.sh"));
  assert.match(run(workflowScript, [loose]).stderr, /cannot read .*ci-job\.sh/);

  for (const args of [
    [],
    [file, "--overhead-sample", "hash:0/6"],
    [file, "--overhead-sample", "hash:7/6"],
    [file, "--bogus", "1"],
  ])
    assert.equal(run(workflowScript, args).status, 2, args.join(" "));
});

test("the generated rustc timer times a real compile, passes its exit status through, and says when it cannot write", (t) => {
  const dir = scratch(t);
  const timer = join(dir, "rustc-timer");
  writeFileSync(timer, TIMER);
  chmodSync(timer, 0o755);
  const timings = join(dir, "rustc.tsv");
  const out = join(dir, "out");
  mkdirSync(out);
  writeFileSync(join(dir, "probe.rs"), "pub fn probe() -> u8 { 7 }\n");
  writeFileSync(join(dir, "broken.rs"), "pub fn broken( {\n");
  const rustc = (source, env = {}) =>
    spawnSync(
      "perl",
      [
        timer,
        "rustc",
        "--crate-name",
        "probe",
        "--crate-type",
        "lib",
        "--edition",
        "2021",
        "--out-dir",
        out,
        join(dir, source),
      ],
      { encoding: "utf8", env: { ...process.env, ONEPIPELINE_RUSTC_TIMINGS: timings, ...env } },
    );
  const ok = rustc("probe.rs");
  assert.equal(ok.status, 0, ok.stderr);
  assert.ok(
    readdirSync(out).some((f) => f.endsWith(".rlib")),
    "the compile really ran",
  );
  const failed = rustc("broken.rs");
  assert.notEqual(failed.status, 0);
  const rows = readFileSync(timings, "utf8")
    .trim()
    .split("\n")
    .map((l) => l.split("\t"));
  assert.equal(rows.length, 2);
  for (const [i, code] of [
    [0, "0"],
    [1, String(failed.status)],
  ]) {
    const [start, end, crate, types, instrumented, outDir, status, compiler] = rows[i];
    assert.ok(Number(end) >= Number(start) && Number(start) > 1e9);
    assert.deepEqual(
      [crate, types, instrumented, outDir, status, compiler],
      ["probe", "lib", "0", out, code, "rustc"],
    );
  }
  const unwritable = rustc("probe.rs", {
    ONEPIPELINE_RUSTC_TIMINGS: join(dir, "no", "such", "dir", "t.tsv"),
  });
  assert.equal(unwritable.status, 0);
  assert.match(unwritable.stderr, /rustc-timer: cannot append to .*this compile is not timed/);
});

test("the generated sampler writes timestamped CPU, load and network lines", (t) => {
  const dir = scratch(t);
  const sampler = join(dir, "sampler.sh");
  writeFileSync(sampler, SAMPLER);
  // It loops until the job ends; here, until the timeout ends it.
  spawnSync("bash", [sampler, dir], { timeout: 4500 });
  const lines = readFileSync(join(dir, "samples.txt"), "utf8").trim().split("\n");
  assert.ok(lines.length >= 2, lines.join("\n"));
  for (const line of lines) assert.match(line, /^\d{10} \S.* \| \S.* \| \d+ \d+$/);
});

const REPO = "example/onepipeline";
const RUN = 42;
const T0 = Date.parse("2026-11-13T10:00:00Z");
const iso = (s) => new Date(T0 + s * 1000).toISOString();
const key = (endpoint) => endpoint.replace(/[^\w.-]+/g, "_");
const golden = JSON.parse(
  readFileSync(join(root, "tests", "golden", "test-profile-record-v2.json"), "utf8"),
);

// A record the recorder writes: its golden record with this test's figures.
function record(name, start, wall, parts) {
  const r = structuredClone(golden);
  r.binary = "e2e";
  r.test = name;
  r.run_id = "nextest-run";
  r.started_at = (T0 + start * 1000) / 1000;
  r.wall_s = wall;
  for (const category of Object.keys(r.categories)) {
    r.categories[category].s = parts[category] ?? 0;
    r.categories[category].inclusive_s = parts[category] ?? 0;
  }
  r.unattributed_s = wall - Object.values(parts).reduce((a, b) => a + b, 0);
  r.programs = {
    onepipeline: { count: 4, s: parts.spawn ?? 0 },
    git: { count: 2, s: parts.git ?? 0 },
  };
  return r;
}

// Nx's own task trace for these tasks, written by the pinned Nx's TaskProfilingLifeCycle.
function nxTrace(file, tasks) {
  const require = createRequire(join(root, "package.json"));
  const nx = dirname(require.resolve("nx/package.json"));
  const { TaskProfilingLifeCycle } = require(
    join(nx, "dist", "src", "tasks-runner", "life-cycles", "task-profiling-life-cycle.js"),
  );
  const life = new TaskProfilingLifeCycle(relative(process.cwd(), file));
  const nxTasks = tasks.map(([id, start, end]) => {
    const [project, target] = id.split(":");
    return {
      id,
      target: { project, target },
      startTime: T0 + start * 1000,
      endTime: T0 + end * 1000,
    };
  });
  life.startTasks(nxTasks, { groupId: 0 });
  life.endTasks(
    nxTasks.map((task) => ({ task, status: "success" })),
    { groupId: 0 },
  );
  life.endCommand();
}

const TASKS = [
  ["demo:build", 100, 450],
  ["demo-e2e:test", 300, 680],
  // Another task runs one more test of the e2e binary after the tier has ended.
  ["demo-version:smoke", 682, 688],
];

// What GitHub serves for one finished test-linux job: 60 s of setup, a 30 s rust-cache
// restore, a 600 s gate holding a build task and the e2e tier, and a 10 s upload. Each
// answer is keyed as the harness keys its cache, so it seeds a cache or a stand-in `gh`.
function oneRun(dir, { status = "completed" } = {}) {
  const answers = {};
  const put = (endpoint, body) => {
    answers[key(endpoint)] = typeof body === "string" ? body : JSON.stringify(body);
  };
  put(`repos/${REPO}/actions/runs/${RUN}`, {
    status,
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
      `${iso(91)} ci-job: test-linux runs 3 task(s): demo:build demo-e2e:test demo-version:smoke`,
      `${iso(400)} Starting 2 tests across 1 binary`,
      `${iso(600)} \u001b[32mSummary\u001b[0m [ 200.000s] 2 tests run: 2 passed, 0 skipped`,
      `${iso(689)}   Critical path:     8m 0s (2 tasks)`,
    ].join("\n"),
  );
  const names = ["test-profile-test-linux", "nx-cache-entries-test-linux", "measure-test-linux"];
  put(`repos/${REPO}/actions/runs/${RUN}/artifacts?per_page=100`, {
    artifacts: names.map((name, id) => ({ id, name, expired: false })),
  });
  // The artifacts' contents, one directory each.
  const files = join(dir, "artifact-files");
  const artifact = (name) => {
    const d = join(files, name);
    mkdirSync(d, { recursive: true });
    return d;
  };
  const records = join(artifact(names[0]), "e2e");
  mkdirSync(records);
  writeFileSync(
    join(records, "a.json"),
    JSON.stringify(record("a", 400, 200, { spawn: 120, git: 20, wait: 50 })),
  );
  writeFileSync(
    join(records, "b.json"),
    JSON.stringify(record("b", 400, 100, { spawn: 40, wait: 60 })),
  );
  writeFileSync(join(records, "late.json"), JSON.stringify(record("late", 683, 3, { spawn: 3 })));
  writeFileSync(
    join(artifact(names[1]), "executions.json"),
    JSON.stringify({
      tasks: TASKS.map(([task, a, b]) => ({
        task,
        result: "executed",
        started_at: iso(a),
        finished_at: iso(b),
      })),
      store_attempts: [{ status: "failed", http_status: 403 }],
    }),
  );
  writeFileSync(
    join(files, names[1], "manifest.json"),
    JSON.stringify({ entries: [], stored_bytes: 0 }),
  );
  const measure = artifact(names[2]);
  nxTrace(join(measure, "nx-profile.json"), TASKS);
  const rustcRow = (a, b, dirName) =>
    [
      (T0 + a * 1000) / 1000,
      (T0 + b * 1000) / 1000,
      "onepipeline",
      "lib",
      dirName.includes("llvm-cov") ? 1 : 0,
      `/home/runner/work/onepipeline/onepipeline/target/${dirName}/deps`,
      0,
      "/home/runner/.rustup/toolchains/1.97.1-x86_64-unknown-linux-gnu/bin/rustc",
    ].join("\t");
  // The last line is cut short, as a job ending mid-write leaves it.
  writeFileSync(
    join(measure, "rustc.tsv"),
    `${rustcRow(100, 250, "debug")}\n${rustcRow(300, 380, "llvm-cov-target/debug")}\n17944\t`,
  );
  return { answers, files, names };
}

function seedCache(cache, served) {
  mkdirSync(join(cache, "api"), { recursive: true });
  for (const [k, body] of Object.entries(served.answers))
    writeFileSync(join(cache, "api", `${k}.txt`), body);
  for (const name of served.names) {
    const into = join(cache, "runs", String(RUN), "test-linux", name);
    mkdirSync(into, { recursive: true });
    execFileSync("cp", ["-R", `${join(served.files, name)}/.`, into]);
    writeFileSync(join(into, ".complete"), "");
  }
}

// A stand-in `gh` on PATH: `api rate_limit` answers FAKE_GH_REMAINING, and `api <endpoint>`
// answers the file served for it (an artifact zip as bytes), or fails as GitHub's 404 does.
// Every call is logged.
function standInGh(dir, served) {
  const bin = join(dir, "bin");
  const answers = join(dir, "answers");
  mkdirSync(bin, { recursive: true });
  mkdirSync(answers, { recursive: true });
  for (const [k, body] of Object.entries(served.answers)) writeFileSync(join(answers, k), body);
  served.names.forEach((name, id) => {
    // `zip -` writes the archive to stdout; given a file name without `.zip`, it appends one.
    const archive = execFileSync("zip", ["-qr", "-", "."], { cwd: join(served.files, name) });
    writeFileSync(join(answers, key(`repos/${REPO}/actions/artifacts/${id}/zip`)), archive);
  });
  writeFileSync(
    join(bin, "gh"),
    `#!${process.execPath}
const fs = require("node:fs");
const args = process.argv.slice(2);
fs.appendFileSync(process.env.FAKE_GH_LOG, args.join(" ") + "\\n");
if (args[1] === "rate_limit") { process.stdout.write(process.env.FAKE_GH_REMAINING + "\\n"); process.exit(0); }
const file = ${JSON.stringify(answers)} + "/" + args.at(-1).replace(/[^\\w.-]+/g, "_");
if (!fs.existsSync(file)) { process.stderr.write("gh: Not Found (HTTP 404)\\n"); process.exit(1); }
process.stdout.write(fs.readFileSync(file));
`,
  );
  chmodSync(join(bin, "gh"), 0o755);
  const log = join(dir, "gh.log");
  writeFileSync(log, "");
  return {
    log,
    env: (remaining) => ({
      ...process.env,
      PATH: `${bin}:${process.env.PATH}`,
      FAKE_GH_LOG: log,
      FAKE_GH_REMAINING: String(remaining),
    }),
  };
}

const job = (cache, out, extra = []) => [
  "job",
  "--run",
  String(RUN),
  "--job",
  "test-linux",
  "--repo",
  REPO,
  "--cache",
  cache,
  "--out",
  out,
  ...extra,
];

test("one job's wall clock is divided into steps, Nx tasks, compiles and test categories that sum to it, from the cache alone", (t) => {
  const dir = scratch(t);
  const cache = join(dir, "cache");
  seedCache(cache, oneRun(dir));
  const json = join(dir, "out.json");
  const out = join(dir, "report.md");
  const result = run(profileScript, job(cache, out, ["--json", json]));
  assert.equal(result.status, 0, result.stderr);
  assert.match(
    result.stdout,
    /^ci-time-profile: run 42 test-linux: wall 11\.7 min, gate 10\.0 min, unattributed 1\.0%; report in .*report\.md \(GitHub requests 0, rate checks 0, read from cache 4\)\n$/,
  );
  const report = readFileSync(out, "utf8");
  assert.match(report, /wall clock 11\.7 min/);
  assert.match(report, new RegExp(`GITHUB_SHA ${"b".repeat(40)}`));
  assert.match(report, /Nx selection: 3 task\(s\): demo:build demo-e2e:test demo-version:smoke/);
  assert.match(report, /Nx task times from: NX_PROFILE trace/);
  assert.match(report, /Critical path 8m 0s \(2 tasks\)/);
  assert.match(report, /Restored from cache key "v0-rust-test-linux" full match: true/);
  assert.match(report, /Nx store attempts by status\/http: \{"failed\/403":1\}/);

  const r = JSON.parse(readFileSync(json, "utf8"));
  assert.equal(r.wall_s, 700);
  assert.equal(r.step_classes["gate (Nx tasks)"], 600);
  assert.equal(r.step_classes["cache restore"], 30);
  const gate = Object.values(r.gate_categories).reduce((a, b) => a + b, 0);
  assert.ok(Math.abs(gate - 600) < 1e-6, `gate categories sum to ${gate}`);
  const near = (category, seconds) =>
    assert.ok(
      Math.abs((r.gate_categories[category] ?? 0) - seconds) < 1e-6,
      `${category}: ${r.gate_categories[category]}`,
    );
  near("gate: Nx planning before the first task", 10);
  near("compile: plain (target/debug)", 150);
  // From 300 s to 450 s the build and the e2e tier overlap, so each second is half each's.
  // The instrumented rustc (300-380 s) is both tasks' compile: 40 + 40 s.
  near("compile: instrumented (llvm-cov-target)", 80);
  near("task (no test records): demo:build", 50 + 35);
  // e2e's 380-400 s at half, and the version task's 682-683 s.
  near("task: before its first test, no rustc running (cargo, nextest, archive)", 11);
  // The tests: 400-450 s both tests in half the second (a quarter each), 450-500 s both in the
  // whole (a half each), 500-600 s test a alone. So a has 137.5 s, split 120/20/50/10 of its
  // 200 s record, and b 37.5 s, split 40/0/60/0 of its 100 s; plus the late test's 3 s of
  // spawn, alone, 683-686 s.
  near("test: spawn: onepipeline", (137.5 * 120) / 200 + (37.5 * 40) / 100 + 3);
  near("task: after its last test (profdata merge, LCOV export)", 80);
  near("task: after its last test", 2);
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

test("a malformed measurement line other than the last refuses, naming the file and what to do", (t) => {
  const dir = scratch(t);
  const cache = join(dir, "cache");
  seedCache(cache, oneRun(dir));
  const tsv = join(cache, "runs", String(RUN), "test-linux", "measure-test-linux", "rustc.tsv");
  writeFileSync(tsv, `not a row\n${readFileSync(tsv, "utf8")}`);
  const result = run(profileScript, job(cache, join(dir, "r.md")));
  assert.equal(result.status, 1);
  assert.match(result.stderr, /rustc\.tsv line 1 is malformed\nnext: /);
});

test("a first measurement reads through gh and keeps every answer, so the second reads nothing", (t) => {
  const dir = scratch(t);
  const gh = standInGh(dir, oneRun(dir));
  const cache = join(dir, "cache");
  const first = run(profileScript, job(cache, join(dir, "one.md")), { env: gh.env(4000) });
  assert.equal(first.status, 0, first.stderr);
  // run, jobs, log, artifact list and three artifacts: seven requests, each after a rate check.
  assert.match(first.stdout, /GitHub requests 7, rate checks 7, read from cache 0/);
  const calls = readFileSync(gh.log, "utf8").trim().split("\n");
  assert.equal(calls.filter((c) => c.startsWith("api rate_limit")).length, 7);
  assert.ok(calls.some((c) => c.endsWith(`repos/${REPO}/actions/artifacts/2/zip`)));
  writeFileSync(gh.log, "");
  const second = run(profileScript, job(cache, join(dir, "two.md")), { env: gh.env(4000) });
  assert.equal(second.status, 0, second.stderr);
  assert.match(second.stdout, /GitHub requests 0, rate checks 0, read from cache 4/);
  assert.equal(readFileSync(gh.log, "utf8"), "");
  // The same report, but for the closing line that counts this invocation's requests.
  const body = (f) =>
    readFileSync(join(dir, f), "utf8").replace(/GitHub requests this invocation.*\n$/, "");
  assert.equal(body("one.md"), body("two.md"));
});

test("under the rate floor nothing is read, and the refusal says when to come back", (t) => {
  const dir = scratch(t);
  const gh = standInGh(dir, oneRun(dir));
  const cache = join(dir, "cache");
  const result = run(profileScript, job(cache, join(dir, "r.md")), { env: gh.env(2499) });
  assert.equal(result.status, 1);
  assert.match(
    result.stderr,
    /2499 core requests remaining, under the floor of 2500; nothing more is read\nnext: wait for the allowance's reset/,
  );
  assert.deepEqual(readFileSync(gh.log, "utf8").trim().split("\n"), [
    "api rate_limit --jq .resources.core.remaining",
  ]);
  assert.deepEqual(readdirSync(join(cache, "api")), []);
  assert.ok(!existsSync(join(dir, "r.md")));
});

test("an unfinished run is refused and not kept, so it is read again once it finishes", (t) => {
  const dir = scratch(t);
  const running = standInGh(
    join(dir, "running"),
    oneRun(join(dir, "running"), { status: "in_progress" }),
  );
  const cache = join(dir, "cache");
  const early = run(profileScript, job(cache, join(dir, "r.md")), { env: running.env(4000) });
  assert.equal(early.status, 1);
  assert.match(
    early.stderr,
    /run 42 is in_progress, not completed\nnext: measure it once it has finished/,
  );
  const done = standInGh(join(dir, "done"), oneRun(join(dir, "done")));
  const later = run(profileScript, job(cache, join(dir, "r.md")), { env: done.env(4000) });
  assert.equal(later.status, 0, later.stderr);
  assert.match(later.stdout, /wall 11\.7 min/);
});

test("a run without the named job, or arguments the command does not take, are refused", (t) => {
  const dir = scratch(t);
  const cache = join(dir, "cache");
  seedCache(cache, oneRun(dir));
  const missing = run(
    profileScript,
    [...job(cache, join(dir, "r.md"))].map((a) => (a === "test-linux" ? "test-macos" : a)),
  );
  assert.equal(missing.status, 1);
  assert.match(
    missing.stderr,
    /run 42 has no job test-macos\nnext: name one of its jobs: test-linux/,
  );
  for (const args of [
    ["job", "--run", "x", "--job", "test-linux", "--out", "r"],
    ["job", "--run", "1", "--job", "test-linux"],
    ["job", "--run", "1", "--job", "test-linux", "--out", "r", "--since", "2026-11-01"],
    ["job", "--run", "1", "--job", "test-linux", "--out", "r", "--min-remaining", "-1"],
    [
      "history",
      "--since",
      "2026-11-05",
      "--split",
      "2026-11-04",
      "--until",
      "2026-11-06",
      "--out",
      "r",
    ],
    [
      "history",
      "--since",
      "2026-11-01",
      "--split",
      "2026-11-04",
      "--until",
      "2026-11-06",
      "--out",
      "r",
      "--profiles",
      "x",
    ],
    ["nonsense"],
  ])
    assert.equal(run(profileScript, args).status, 2, args.join(" "));
});

test("the history counts each finished job by what its pull request changed, before and after the split", (t) => {
  const dir = scratch(t);
  // A repository whose main has one commit, and two pull-request heads off it: one editing
  // engine source, one editing documentation.
  const repo = join(dir, "repo");
  const git = (...args) => execFileSync("git", args, { cwd: repo, encoding: "utf8" }).trim();
  mkdirSync(repo);
  git("init", "-q", "-b", "main");
  git("config", "user.email", "spike@example.invalid");
  git("config", "user.name", "spike");
  writeFileSync(join(repo, "README.md"), "x\n");
  git("add", "-A");
  git("commit", "-qm", "base");
  git("checkout", "-qb", "src");
  mkdirSync(join(repo, "src"));
  writeFileSync(join(repo, "src", "lib.rs"), "\n");
  git("add", "-A");
  git("commit", "-qm", "src");
  const srcHead = git("rev-parse", "HEAD");
  git("checkout", "-qb", "docs", "main");
  mkdirSync(join(repo, "docs"));
  writeFileSync(join(repo, "docs", "a.md"), "\n");
  git("add", "-A");
  git("commit", "-qm", "docs");
  const docsHead = git("rev-parse", "HEAD");

  const answers = {};
  const runs = [
    [1, srcHead, "2026-11-08T10:00:00Z", 100],
    [2, srcHead, "2026-11-11T10:00:00Z", 120],
    [3, docsHead, "2026-11-11T11:00:00Z", 30],
    [4, srcHead, "2026-11-12T10:00:00Z", 130],
  ];
  answers[
    key(
      `repos/${REPO}/actions/workflows/ci.yml/runs?event=pull_request&status=completed&per_page=100&page=1&created=2026-11-04T00:00:00..2026-11-13T00:00:00`,
    )
  ] = JSON.stringify({
    workflow_runs: [
      ...runs.map(([id, head, created]) => ({
        id,
        run_attempt: 1,
        head_sha: head,
        created_at: created,
        conclusion: "success",
      })),
      {
        id: 5,
        run_attempt: 1,
        head_sha: srcHead,
        created_at: "2026-11-12T11:00:00Z",
        conclusion: "cancelled",
      },
    ],
  });
  for (const [id, , created, minutes] of runs) {
    const start = Date.parse(created);
    answers[key(`repos/${REPO}/actions/runs/${id}/attempts/1/jobs?per_page=100`)] = JSON.stringify({
      jobs: [
        {
          name: "test-linux",
          conclusion: "success",
          started_at: created,
          completed_at: new Date(start + minutes * 60000).toISOString(),
        },
        { name: "lint", conclusion: "success", started_at: created, completed_at: created },
      ],
    });
  }
  // The src/ runs' e2e records: run 1 spends 2 s a spawn, run 2 and run 4 1 s.
  const artifactFiles = join(dir, "artifact-files");
  const names = [];
  for (const [id, , , , spawnS] of runs
    .filter((r) => r[1] === srcHead)
    .map((r, i) => [...r, i === 0 ? 2 : 1])) {
    const name = `test-profile-test-linux-${id}`;
    const d = join(artifactFiles, name, "e2e");
    mkdirSync(d, { recursive: true });
    const r = record("t", 0, 10, { spawn: 4 * spawnS });
    writeFileSync(join(d, "t.json"), JSON.stringify(r));
    names.push(name);
    answers[key(`repos/${REPO}/actions/runs/${id}/artifacts?per_page=100`)] = JSON.stringify({
      artifacts: [{ id: names.length - 1, name: "test-profile-test-linux", expired: false }],
    });
  }
  const gh = standInGh(join(dir, "gh"), { answers, files: artifactFiles, names });
  const out = join(dir, "history.md");
  const json = join(dir, "history.json");
  const result = run(
    profileScript,
    [
      "history",
      "--since",
      "2026-11-04T00:00:00Z",
      "--split",
      "2026-11-10T00:00:00Z",
      "--until",
      "2026-11-13T00:00:00Z",
      "--jobs",
      "test-linux",
      "--base-ref",
      "main",
      "--profiles",
      "edits src/",
      "--repo",
      REPO,
      "--cache",
      join(dir, "cache"),
      "--out",
      out,
      "--json",
      json,
    ],
    { cwd: repo, env: gh.env(4000) },
  );
  assert.equal(result.status, 0, result.stderr);
  assert.match(
    result.stdout,
    /^ci-time-profile history: 5 runs listed, 4 successful jobs counted; report in /,
  );
  const h = JSON.parse(readFileSync(json, "utf8"));
  const summary = (cls, side) => h.summary[`test-linux\t${cls}\t${side}`];
  assert.deepEqual(summary("edits src/", "before"), {
    n: 1,
    median_min: 100,
    min_min: 100,
    max_min: 100,
  });
  assert.deepEqual(summary("edits src/", "after"), {
    n: 2,
    median_min: 125,
    min_min: 120,
    max_min: 130,
  });
  assert.equal(summary("other", "after").median_min, 30);
  assert.equal(summary("all", "after").n, 3);
  assert.deepEqual(
    h.rows
      .filter((r) => r.class === "edits src/")
      .map((r) => [r.run, r.e2e_tests, r.onepipeline_spawn_s]),
    [
      [1, 1, 2],
      [2, 1, 1],
      [4, 1, 1],
    ],
  );
  assert.match(
    readFileSync(out, "utf8"),
    /\| test-linux \| edits src\/ \| after \| 2 \| 125\.0 \|/,
  );
});
