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
import { artifactNames, stepClass } from "../../scripts/ci-time-profile.mjs";
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
    assert.match(block, /echo "NEXTEST_STATUS_LEVEL=pass" >> "\$GITHUB_ENV"/);
    assert.match(block, /name: measure-\$\{\{ github\.job \}\}/);
    // The graph step plans exactly the targets ci-job.sh runs for the job.
    assert.ok(block.includes(`-t "${targets(job)}" --graph=stdout`), job);
  }
  // The generated graph and overhead steps are held as text here, not run: they plan Nx over
  // the whole project graph and run the instrumented e2e tier, which only the CI job's
  // toolchain and build can do; a hosted demonstration run is what executes them.
  assert.equal(edited.match(/one e2e partition instrumented, then plain/g).length, 1);
  // The rust-cache key is computed before the wrapper is set: the timer step comes after it.
  const linux = edited.slice(edited.indexOf("\n  test-linux:\n"));
  assert.ok(linux.indexOf("Swatinem/rust-cache") < linux.indexOf("rustc-timer"));
  // The overhead step runs the same partition both ways, the gate's records set aside first
  // and each run's moved apart after it, and says when a run fails.
  const overhead = linux.slice(linux.indexOf("one e2e partition instrumented, then plain"));
  assert.match(overhead, /mv target\/test-profile "\$m\/gate-records"/);
  assert.match(
    overhead,
    /run instrumented env RUSTFLAGS="-D warnings" .*bash scripts\/coverage-unit\.sh onepipeline-e2e-sample -- nextest --locked -E "\$filter" --partition hash:1\/6 --no-fail-fast/,
  );
  assert.match(
    overhead,
    /run plain bash scripts\/nextest-run\.sh cargo nextest run --locked -E "\$filter" --partition hash:1\/6 --no-fail-fast/,
  );
  assert.match(overhead, /mv target\/test-profile "\$m\/\$label-records"/);
  assert.match(overhead, /echo "\$label \$start \$\(date \+%s\) \$status" >> "\$m\/times\.txt"/);
  assert.match(overhead, /::warning::measurement: \$label exited \$status.*ACTION: /);
  // A graph Nx cannot write is reported with how to reproduce it, not dropped.
  assert.match(
    linux,
    /if ! ONEPIPELINE_NX_SHOW_OUTPUT=1 env -u NX_PROFILE bash scripts\/nx-affected\.sh -t "[^"]+" --graph=stdout > "\$m\/graph\.json" 2> "\$m\/graph\.err"; then\n\s+echo "::warning::measurement: Nx could not write the task graph.*ACTION: .*reproduce with: ONEPIPELINE_NX_SHOW_OUTPUT=1 bash scripts\/nx-affected\.sh -t [^"]+"\n\s+fi/,
  );
  // The sampler's errors go to sampler.err, which the graph step reports with an ACTION.
  assert.match(linux, /nohup bash "\$m\/sampler\.sh" "\$m" > \/dev\/null 2> "\$m\/sampler\.err" &/);
  assert.match(
    linux,
    /if \[ -s "\$m\/sampler\.err" \]; then\n\s+echo "::warning::measurement: the CPU and network sampler reported errors.*ACTION: [^\n]+"\n\s+head -5 "\$m\/sampler\.err"\n\s+fi/,
  );

  const plain = checkout(join(scratch(t), "plain"));
  assert.equal(run(workflowScript, [plain]).status, 0);
  assert.doesNotMatch(readFileSync(plain, "utf8"), /one e2e partition/);
  assert.doesNotMatch(readFileSync(plain, "utf8"), /cargo --timings of the workspace tests/);

  // With --timings, test-linux builds the workspace's tests after the gate with cargo's
  // --timings, plain and then through the coverage build, each from a fresh source edit.
  const timed = checkout(join(scratch(t), "timed"));
  assert.equal(run(workflowScript, [timed, "--timings"]).status, 0);
  const timedText = readFileSync(timed, "utf8");
  const step = timedText.slice(timedText.indexOf("cargo --timings of the workspace tests"));
  assert.equal(timedText.match(/cargo --timings of the workspace tests/g).length, 1);
  assert.ok(
    timedText.indexOf("\n  test-macos:\n") >
      timedText.indexOf("cargo --timings of the workspace tests"),
  );
  assert.match(step, /touch src\/lib\.rs/);
  assert.match(
    step,
    /run plain env -u RUSTFLAGS cargo test --workspace --no-run --locked --timings/,
  );
  assert.match(step, /cp target\/cargo-timings\/cargo-timing\.html "\$m\/plain\.html"/);
  assert.match(
    step,
    /run instrumented env RUSTFLAGS="-D warnings" bash scripts\/coverage-unit\.sh --cargo -- test --workspace --no-run --locked --timings/,
  );
  assert.match(
    step,
    /cp target\/llvm-cov-target\/cargo-timings\/cargo-timing\.html "\$m\/instrumented\.html"/,
  );
  assert.match(step, /::warning::measurement: the \$label timed build exited \$status.*ACTION: /);

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

  const unsafe = checkout(join(scratch(t), "unsafe"));
  const ciJobPath = join(dirname(unsafe), "..", "..", "scripts", "ci-job.sh");
  writeFileSync(
    ciJobPath,
    readFileSync(ciJobPath, "utf8").replace(
      'test-linux) targets="test,',
      'test-linux) targets="test;rm -rf /,',
    ),
  );
  const shell = run(workflowScript, [unsafe]);
  assert.equal(shell.status, 1);
  assert.match(shell.stderr, /targets for test-linux are not Nx target names/);

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
  // A stand-in compiler for what a real compile here does not show: a compiler ended by a
  // signal, a test target, several crate types, and cargo-llvm-cov's instrumented flags.
  const fake = join(dir, "fake-rustc");
  writeFileSync(fake, '#!/bin/sh\n[ "$FAKE_SIGNAL" = 1 ] && kill -TERM $$\nexit 0\n');
  chmodSync(fake, 0o755);
  const fakeTimings = join(dir, "fake.tsv");
  const timed = (args, env = {}) =>
    spawnSync("perl", [timer, fake, "--crate-name", "x", ...args], {
      encoding: "utf8",
      env: { ...process.env, ONEPIPELINE_RUSTC_TIMINGS: fakeTimings, ...env },
    });
  assert.equal(timed([], { FAKE_SIGNAL: "1" }).status, 128 + 15);
  assert.equal(timed(["--test"]).status, 0);
  assert.equal(timed(["--crate-type", "lib", "--crate-type", "rlib"]).status, 0);
  assert.equal(timed(["-Cinstrument-coverage", "--crate-type", "lib"]).status, 0);
  assert.deepEqual(
    readFileSync(fakeTimings, "utf8")
      .trim()
      .split("\n")
      .map((l) => l.split("\t").slice(3, 7).join(" ")),
    [" 0  143", "test 0  0", "lib,rlib 0  0", "lib 1  0"],
  );
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
  // The analysis reads what the sampler wrote: its lines become a run's samples.
  const cacheDir = join(dir, "cache");
  seedCache(cacheDir, oneRun(join(dir, "run")));
  const shifted = lines.map((line, i) =>
    line.replace(/^\d+/, String((T0 + (30 + 2 * i) * 1000) / 1000)),
  );
  writeFileSync(
    join(artifactDir(cacheDir, "measure-test-linux"), "samples.txt"),
    `${shifted.join("\n")}\n`,
  );
  const json = join(dir, "out.json");
  const read = run(profileScript, job(cacheDir, join(dir, "r.md"), ["--json", json]));
  assert.equal(read.status, 0, read.stderr);
  const sampled = JSON.parse(readFileSync(json, "utf8")).samples;
  assert.ok(sampled.sampled_s >= 2, JSON.stringify(sampled));
  assert.ok(
    sampled.cpu_busy_fraction === null ||
      (sampled.cpu_busy_fraction >= 0 && sampled.cpu_busy_fraction <= 1),
  );
  // The macOS branch runs only on macOS, which this target does not: its commands are held
  // here, and the lines it wrote on a hosted macOS runner are the fixture's macOS samples.
  const mac = SAMPLER.slice(SAMPLER.indexOf("else"));
  assert.match(mac, /iostat -n 0 -c 2 -w 1 \| tail -1/);
  assert.match(mac, /sysctl -n vm\.loadavg/);
  assert.match(mac, /netstat -ibn \| awk '\$3 ~ \/\^<Link\/ && \$1 !~ \/\^lo\//);
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

// The Nx sidecar's report on this job, written by the sidecar itself
// (`scripts/nx-remote-cache.mjs report`) from the telemetry its Nx hooks and proxy record:
// one invocation running TASKS, and one store attempt the read-only cache refused. With
// `cancelled`, a second invocation began and never reported, as a failed gate's does.
function sidecarReport(dir, into, { cancelled = false } = {}) {
  const temp = join(dir, "runner-temp");
  const state = join(temp, "nx-cache-recording", "test-linux");
  mkdirSync(state, { recursive: true });
  const line = (v) => `${JSON.stringify(v)}\n`;
  writeFileSync(
    join(state, "started.jsonl"),
    line({ id: "inv-1", argv: ["nx"] }) + (cancelled ? line({ id: "inv-2", argv: ["nx"] }) : ""),
  );
  writeFileSync(
    join(state, "invocations.jsonl"),
    line({
      id: "inv-1",
      started_at: iso(91),
      finished_at: iso(689),
      tasks: TASKS.map(([task, a, b], i) => ({
        task,
        hash: `ab${i}`,
        cacheable: true,
        status: "success",
        code: 0,
        started_at: iso(a),
        finished_at: iso(b),
      })),
    }),
  );
  writeFileSync(
    join(state, "requests.jsonl"),
    line({ id: "put-1", hash: "ab0", method: "PUT", attempted_at: iso(451), http_status: 403 }),
  );
  const event = join(dir, "event.json");
  writeFileSync(
    event,
    JSON.stringify({ pull_request: { number: 7, head: { sha: "a".repeat(40) } } }),
  );
  execFileSync(
    process.execPath,
    [join(root, "scripts", "nx-remote-cache.mjs"), "report", "--job", "test-linux", "--out", into],
    {
      cwd: dir,
      env: {
        ...process.env,
        RUNNER_TEMP: temp,
        GITHUB_JOB: "test-linux",
        GITHUB_EVENT_PATH: event,
        GITHUB_REPOSITORY: REPO,
        GITHUB_REF: "refs/pull/7/merge",
        GITHUB_RUN_ID: String(RUN),
        GITHUB_RUN_ATTEMPT: "1",
        GITHUB_SHA: "d".repeat(40),
        NX_BASE: "c".repeat(40),
        NX_HEAD: "a".repeat(40),
      },
      stdio: "ignore",
    },
  );
  assert.ok(existsSync(join(into, "executions.json")), "the sidecar wrote its report");
}

// What GitHub serves for one finished test-linux job: 60 s of setup, a 30 s rust-cache
// restore, a 600 s gate holding a build task and the e2e tier, and a 10 s upload, with the
// Nx sidecar's start step running in the background beside them. Each answer is keyed as
// the harness keys its cache, so it seeds a cache or a stand-in `gh`. `sidecarOnly` leaves
// out Nx's trace and the sidecar's manifest and gives the sidecar a cancelled invocation.
function oneRun(
  dir,
  {
    status = "completed",
    sidecarOnly = false,
    noTrace = false,
    log = null,
    macSamples = false,
  } = {},
) {
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
    ["Run op-nx/github-cache/start-cache-server@91839c24885842cac86567c30e23b37fb85f7e33", 60, 695],
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
    log ??
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
    artifacts: names.map((name, id) => ({ id, name, expired: false, created_at: iso(700) })),
  });
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
  // A record cut short: left out, and said so.
  writeFileSync(join(records, "broken.json"), "{");
  const sidecar = artifact(names[1]);
  sidecarReport(dir, sidecar, { cancelled: sidecarOnly });
  if (sidecarOnly) rmSync(join(sidecar, "manifest.json"));
  const measure = artifact(names[2]);
  if (!sidecarOnly && !noTrace) nxTrace(join(measure, "nx-profile.json"), TASKS);
  writeFileSync(join(measure, "host.txt"), "Linux runner 6.8.0 x86_64 GNU/Linux\n4\n1794560000\n");
  const toolchain = "/home/runner/.rustup/toolchains/1.97.1-x86_64-unknown-linux-gnu/bin/rustc";
  const rustcRow = (a, b, crate, dirName, compiler = toolchain) =>
    [
      (T0 + a * 1000) / 1000,
      (T0 + b * 1000) / 1000,
      crate,
      "lib",
      dirName.includes("llvm-cov") ? 1 : 0,
      dirName.startsWith("/")
        ? dirName
        : `/home/runner/work/onepipeline/onepipeline/target/${dirName}/deps`,
      0,
      compiler,
    ].join("\t");
  // The gate's two compiles, then one compile of every other tree during setup, where no Nx
  // task runs. The last line is cut short, as a job ending mid-write leaves it.
  writeFileSync(
    join(measure, "rustc.tsv"),
    [
      rustcRow(100, 250, "onepipeline", "debug"),
      rustcRow(300, 380, "onepipeline", "llvm-cov-target/debug"),
      rustcRow(
        10,
        20,
        "onepipeline",
        "debug",
        "/home/runner/.rustup/toolchains/1.97.1-x86_64-unknown-linux-gnu/bin/clippy-driver",
      ),
      rustcRow(
        10,
        22,
        "serde",
        "msrv/debug",
        "/home/runner/.rustup/toolchains/1.97-x86_64-unknown-linux-gnu/bin/rustc",
      ),
      rustcRow(20, 50, "onepipeline", "release"),
      rustcRow(20, 24, "git2", "/tmp/cargo-installXyz/release/deps"),
      rustcRow(24, 27, "zerocopy", "npm-staged/x86_64-unknown-linux-gnu/debug"),
      // Cargo asking rustc what it can build: no out dir, and its own tree.
      [(T0 + 5000) / 1000, (T0 + 6000) / 1000, "___", "bin,rlib", 0, "", 0, toolchain].join("\t"),
      // A macOS runner's checkout is under /Users/runner/work: still the checkout's target.
      rustcRow(
        27,
        29,
        "onepipeline",
        "/Users/runner/work/onepipeline/onepipeline/target/debug/deps",
      ),
      "17944\t",
    ].join("\n"),
  );
  // Nx's graph: the version task waits on the e2e tier, which waits on nothing.
  writeFileSync(
    join(measure, "graph.json"),
    `Nx planned 3 tasks\n${JSON.stringify({ tasks: { dependencies: { "demo-version:smoke": ["demo-e2e:test"], "demo-e2e:test": [], "demo:build": [] } } })}`,
  );
  // Three samples, 2 s apart, while both tasks run: 60% of the CPU busy, 1 MB in a second.
  // macOS's sampler writes iostat's percentages (us sy id) and load averages instead of
  // /proc/stat's counters. Its lines here are four the macOS sampler wrote on a hosted runner
  // (run 43400483794), at this fixture's times, then a fifth whose network counter fell.
  const at = (i) => (T0 + (400 + 2 * i) * 1000) / 1000;
  const macLines = [
    " 34  2 64  1.80 7.20 17.92 |  1.80 7.20 17.92  | 1667506687 6889586",
    " 44 30 25  1.80 7.20 17.92 |  1.80 7.20 17.92  | 1667506687 6889586",
    " 33  1 65  1.74 7.09 17.82 |  1.74 7.09 17.82  | 1667506687 6889586",
    " 34  3 63  1.74 7.09 17.82 |  1.74 7.09 17.82  | 1667506687 6889586",
    " 34  3 63  1.74 7.09 17.82 |  1.74 7.09 17.82  | 1 6889586",
  ];
  writeFileSync(
    join(measure, "samples.txt"),
    `${(
      macSamples
        ? macLines.map((line, i) => `${at(i)}${line}`)
        : [0, 1, 2].map(
            (i) => `${at(i)} cpu ${600 * i} 0 0 ${400 * i} 0 0 0 | 3/200 | ${2_000_000 * i} 0`,
          )
    ).join("\n")}\n`,
  );

  // The overhead partition's one test, 25% slower instrumented than plain.
  const overhead = join(measure, "overhead");
  for (const [label, wall] of [
    ["instrumented-records", 10],
    ["plain-records", 8],
  ]) {
    mkdirSync(join(overhead, label, "e2e"), { recursive: true });
    writeFileSync(
      join(overhead, label, "e2e", "x.json"),
      JSON.stringify(record("x", 1000, wall, { spawn: wall / 2 })),
    );
  }
  writeFileSync(
    join(overhead, "times.txt"),
    "instrumented 1000 1060 0\nplain-build 1060 1061 0\nplain 1061 1121 0\n",
  );
  return { answers, files, names };
}

function seedCache(cache, served) {
  mkdirSync(join(cache, "api"), { recursive: true });
  for (const [k, body] of Object.entries(served.answers))
    writeFileSync(join(cache, "api", `${k}.txt`), body);
  served.names.forEach((name, id) => {
    const into = join(cache, "artifacts", REPO.replace("/", "__"), String(id));
    mkdirSync(into, { recursive: true });
    execFileSync("cp", ["-R", `${join(served.files, name)}/.`, into]);
    writeFileSync(join(into, ".complete"), "");
  });
}

// Where the analysis extracts one of oneRun's artifacts: by repository and artifact id, the
// artifact's position in oneRun's names.
const ARTIFACTS = ["test-profile-test-linux", "nx-cache-entries-test-linux", "measure-test-linux"];
const artifactDir = (cache, name) =>
  join(cache, "artifacts", REPO.replace("/", "__"), String(ARTIFACTS.indexOf(name)));

// A stand-in `gh` on PATH: `api rate_limit` answers FAKE_GH_REMAINING, and `api <endpoint>`
// answers the file served for it (an artifact zip as bytes), or fails as GitHub's 404 does.
// Every call is logged.
function standInGh(dir, served, repo = REPO) {
  const bin = join(dir, "bin");
  const answers = join(dir, "answers");
  mkdirSync(bin, { recursive: true });
  mkdirSync(answers, { recursive: true });
  for (const [k, body] of Object.entries(served.answers)) writeFileSync(join(answers, k), body);
  served.names.forEach((name, id) => {
    // `zip -` writes the archive to stdout; given a file name without `.zip`, it appends one.
    const archive = execFileSync("zip", ["-qr", "-", "."], { cwd: join(served.files, name) });
    writeFileSync(join(answers, key(`repos/${repo}/actions/artifacts/${id}/zip`)), archive);
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
    /^ci-time-profile: run 42 test-linux: wall 11\.7 min, gate 10\.0 min, unattributed 1\.0%; report in .*report\.md \(GitHub requests 0, rate checks 0, read from cache 4\); 1 note\(s\) on what could not be read, in the report\n$/,
  );
  const report = readFileSync(out, "utf8");
  assert.match(report, /wall clock 11\.7 min/);
  assert.match(report, new RegExp(`GITHUB_SHA ${"b".repeat(40)}`));
  assert.match(report, /Nx selection: 3 task\(s\): demo:build demo-e2e:test demo-version:smoke/);
  assert.match(report, /Nx task times from: NX_PROFILE trace/);
  assert.match(report, /Critical path 8m 0s \(2 tasks\)/);
  assert.match(report, /Restored from cache key "v0-rust-test-linux" full match: true/);
  // The sidecar's own report: the PUT it was refused, and the two tasks it never tried to store.
  assert.match(report, /Nx store attempts by status\/http: \{"failed\/403":1,"failed\/null":2\}/);
  assert.match(report, /## Notes\n\n- the test record .*broken\.json was left out: it is not JSON/);

  const r = JSON.parse(readFileSync(json, "utf8"));
  assert.equal(r.wall_s, 700);
  assert.equal(r.step_classes["gate (Nx tasks)"], 600);
  assert.equal(r.step_classes["cache restore"], 30);
  // The sidecar's background step overlaps the rest and is not summed: the steps are the job.
  assert.ok(r.steps.find((st) => st.class === "Nx cache sidecar").background);
  assert.equal(
    Object.values(r.step_classes).reduce((a, b) => a + b, 0) + r.between_steps_s,
    r.wall_s,
  );
  assert.deepEqual(
    Object.fromEntries(
      Object.entries(r.compile).map(([tree, c]) => [tree, [c.rustc_s, c.dependency_s]]),
    ),
    {
      "plain (target/debug)": [152, 0],
      "instrumented (llvm-cov-target)": [80, 0],
      clippy: [10, 0],
      "msrv toolchain": [12, 12],
      "release (target/release)": [30, 0],
      "outside the checkout (cargo install)": [4, 4],
      "target/npm-staged": [3, 3],
      "cargo's target-info probe": [1, 1],
    },
  );
  assert.deepEqual(
    r.critical_path.map((c) => [c.task, c.after, c.after_dependencies_s]),
    [
      ["demo-e2e:test", "(gate start)", 210],
      ["demo-version:smoke", "demo-e2e:test", 2],
    ],
  );
  assert.equal(r.samples.cpu_busy_fraction, 0.6);
  assert.equal(r.samples.network_bytes, 4_000_000);
  assert.equal(r.samples.network_active_s, 4);
  assert.deepEqual(r.samples.network_by_place, {
    "gate: demo-e2e:test + demo:build": { bytes: 4_000_000, active_s: 4 },
  });
  assert.deepEqual(r.samples.cpu_busy_by_concurrency, { 2: { s: 4, busy_fraction: 0.6 } });
  assert.deepEqual(
    [
      r.overhead.tests_in_both,
      r.overhead.instrumented_test_seconds,
      r.overhead.plain_test_seconds,
      r.overhead.ratio,
    ],
    [1, 10, 8, 1.25],
  );
  assert.equal(r.overhead.runs.instrumented.s, 60);
  assert.deepEqual(r.measure_host, ["Linux runner 6.8.0 x86_64 GNU/Linux", "4", "1794560000"]);
  assert.match(
    report,
    /## Network and CPU \(sampler\)\n\nsampled 0\.1 min; non-loopback bytes 4 MB; seconds at >= 256 KiB\/s: 4; CPU busy 60%\nCPU busy in the gate by number of Nx tasks running: k=2 60% over 0\.1 min\n/,
  );
  assert.match(report, /\| gate: demo-e2e:test \+ demo:build \| 4\.0 \| 4 \|/);
  assert.match(report, /tests in both 1; test-seconds instrumented 10, plain 8, ratio 1\.250/);
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

test("a measurement file not in its writer's shape refuses, naming the file and what to do", (t) => {
  for (const [file, body, message] of [
    // A rustc row anywhere but last.
    ["rustc.tsv", (old) => `not a row\n${old}`, /rustc\.tsv line 1 is malformed\nnext: /],
    // An Nx trace whose task has no name.
    [
      "nx-profile.json",
      () => '[{"ph":"X","name":5,"ts":1,"dur":1}]',
      /nx-profile\.json is malformed\nnext: /,
    ],
    // An Nx trace whose task carries null where its args belong.
    [
      "nx-profile.json",
      () => '[{"ph":"X","name":"demo:build","ts":1,"dur":1,"args":null}]',
      /nx-profile\.json is malformed\nnext: /,
    ],
    // A rustc row with no out dir, which the timer always writes for a crate.
    [
      "rustc.tsv",
      (old) => `${(T0 + 10000) / 1000}\t${(T0 + 20000) / 1000}\tx\tlib\t0\t\t0\trustc\n${old}`,
      /rustc\.tsv line 1 is malformed\nnext: /,
    ],
    // A sample whose CPU percentages cannot belong to one interval.
    [
      "samples.txt",
      (old) => `${(T0 + 30000) / 1000} 80 80 0 | 1 1 1 | 0 0\n${old}`,
      /samples\.txt line 1 is malformed\nnext: /,
    ],
    // A sample with no timestamp, whose empty field must not read as zero.
    [
      "samples.txt",
      (old) => ` cpu 1 0 0 1 0 0 0 | 1/1 | 0 0\n${old}`,
      /samples\.txt line 1 is malformed\nnext: /,
    ],
    // An overhead run with no exit status.
    [
      "overhead/times.txt",
      (old) => `instrumented 1000 1060\n${old}`,
      /times\.txt line 1 is malformed\nnext: /,
    ],
    // An overhead row with a field the step does not write.
    [
      "overhead/times.txt",
      (old) => `instrumented 1000 1060 0 extra\n${old}`,
      /times\.txt line 1 is malformed\nnext: /,
    ],
    // A graph Nx did not finish writing.
    [
      "graph.json",
      () => 'Nx planned 3 tasks\n{"tasks": {"dependencies":',
      /graph\.json is malformed\nnext: /,
    ],
  ]) {
    const dir = scratch(t);
    const cache = join(dir, "cache");
    seedCache(cache, oneRun(dir));
    const path = join(artifactDir(cache, "measure-test-linux"), file);
    writeFileSync(path, body(readFileSync(path, "utf8")));
    const result = run(profileScript, job(cache, join(dir, "r.md")));
    assert.equal(result.status, 1, file);
    assert.match(result.stderr, message);
  }
});

test("a test retried in another record directory counts once, at its later attempt", (t) => {
  const dir = scratch(t);
  const cache = join(dir, "cache");
  seedCache(cache, oneRun(dir));
  const retry = join(artifactDir(cache, "test-profile-test-linux"), "retry", "e2e");
  mkdirSync(retry, { recursive: true });
  const again = record("b", 400, 100, { spawn: 100 });
  again.attempt = 2;
  writeFileSync(join(retry, "b.json"), JSON.stringify(again));
  // A first attempt of b written after its second, even later in the run, does not displace it.
  mkdirSync(join(retry, "..", "older", "e2e"), { recursive: true });
  writeFileSync(
    join(retry, "..", "older", "e2e", "b.json"),
    JSON.stringify(record("b", 410, 100, { spawn: 1 })),
  );
  // And test a written again by a later run of the same attempt: the later one counts.
  writeFileSync(join(retry, "a.json"), JSON.stringify(record("a", 405, 200, { spawn: 150 })));
  const json = join(dir, "out.json");
  assert.equal(run(profileScript, job(cache, join(dir, "r.md"), ["--json", json])).status, 0);
  const r = JSON.parse(readFileSync(json, "utf8"));
  assert.equal(r.tests, 3);
  assert.equal(r.slowest.find((x) => x.test === "e2e::b").parts_s["spawn: onepipeline"], 100);
  assert.equal(r.slowest.find((x) => x.test === "e2e::a").parts_s["spawn: onepipeline"], 150);
});

test("without cargo the workspace's crates are the instrumented ones, and the report says so", (t) => {
  const dir = scratch(t);
  const cache = join(dir, "cache");
  seedCache(cache, oneRun(dir));
  const json = join(dir, "out.json");
  // Only node on PATH: no cargo to list the workspace's crates.
  const result = run(profileScript, job(cache, join(dir, "r.md"), ["--json", json]), {
    env: { ...process.env, PATH: dirname(process.execPath) },
  });
  assert.equal(result.status, 0, result.stderr);
  assert.match(result.stdout, /2 note\(s\)/);
  const r = JSON.parse(readFileSync(json, "utf8"));
  assert.ok(r.notes.some((n) => /^cargo metadata could not list the workspace's crates/.test(n)));
  // onepipeline was instrumented, so it is still the workspace's; serde and git2 are not.
  assert.equal(r.compile["msrv toolchain"].dependency_s, 12);
  assert.equal(r.compile["plain (target/debug)"].dependency_s, 0);
});

test("a record in the recorder's syntax but not its schema is left out and noted", (t) => {
  const dir = scratch(t);
  const cache = join(dir, "cache");
  seedCache(cache, oneRun(dir));
  const bad = record("unschemed", 400, 5, { spawn: 5 });
  bad.schema = 99;
  writeFileSync(
    join(artifactDir(cache, "test-profile-test-linux"), "e2e", "unschemed.json"),
    JSON.stringify(bad),
  );
  const json = join(dir, "out.json");
  assert.equal(run(profileScript, job(cache, join(dir, "r.md"), ["--json", json])).status, 0);
  const r = JSON.parse(readFileSync(json, "utf8"));
  assert.ok(!r.slowest.some((x) => x.test === "e2e::unschemed"));
  assert.ok(r.notes.some((n) => /unschemed\.json was left out: it is schema 99/.test(n)));
});

test("a job whose gate step is not named as one is all steps, and says so", (t) => {
  const dir = scratch(t);
  const served = oneRun(dir);
  const jobsKey = key(`repos/${REPO}/actions/runs/${RUN}/jobs?per_page=100`);
  const jobs = JSON.parse(served.answers[jobsKey]);
  jobs.jobs[0].steps.find((st) => /affected projects/.test(st.name)).name = "Run the tests";
  served.answers[jobsKey] = JSON.stringify(jobs);
  const cache = join(dir, "cache");
  seedCache(cache, served);
  const json = join(dir, "out.json");
  assert.equal(run(profileScript, job(cache, join(dir, "r.md"), ["--json", json])).status, 0);
  const r = JSON.parse(readFileSync(json, "utf8"));
  assert.equal(r.gate_s, 0);
  assert.deepEqual(r.gate_categories, {});
  assert.equal(r.step_classes["setup and teardown"], 660);
  assert.ok(r.notes.some((n) => /has no step named as a gate/.test(n)));
});

test("a job that uploaded nothing is reported as one unattributed gate", (t) => {
  const dir = scratch(t);
  const served = oneRun(dir);
  served.answers[key(`repos/${REPO}/actions/runs/${RUN}/artifacts?per_page=100`)] = JSON.stringify({
    artifacts: [],
  });
  served.names = [];
  const cache = join(dir, "cache");
  seedCache(cache, served);
  const json = join(dir, "out.json");
  const result = run(profileScript, job(cache, join(dir, "r.md"), ["--json", json]));
  assert.equal(result.status, 0, result.stderr);
  assert.match(result.stdout, /unattributed 85\.7%/);
  const r = JSON.parse(readFileSync(json, "utf8"));
  assert.deepEqual(r.gate_categories, { "gate without Nx task telemetry (unattributed)": 600 });
  assert.equal(r.task_source, "none");
  assert.equal(r.tests, 0);
});

test("macOS's iostat samples are read as CPU percentages, and a counter that falls is noted, not counted", (t) => {
  const dir = scratch(t);
  const cache = join(dir, "cache");
  seedCache(cache, oneRun(dir, { macSamples: true }));
  const json = join(dir, "out.json");
  assert.equal(run(profileScript, job(cache, join(dir, "r.md"), ["--json", json])).status, 0);
  const r = JSON.parse(readFileSync(json, "utf8"));
  // Each interval is the later sample's busy share: 74%, 34% and 37%.
  assert.ok(Math.abs(r.samples.cpu_busy_fraction - (0.74 + 0.34 + 0.37) / 3) < 1e-9);
  assert.equal(r.samples.network_bytes, 0);
  assert.equal(r.samples.counter_resets, 1);
  assert.ok(
    r.notes.some((n) => /^1 sampling interval\(s\) saw a CPU or network counter fall/.test(n)),
  );
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

test("a rate check that answers no count refuses rather than reading", (t) => {
  const dir = scratch(t);
  const gh = standInGh(dir, oneRun(dir));
  const result = run(
    profileScript,
    job(join(dir, "cache"), join(dir, "r.md"), ["--min-remaining", "0"]),
    {
      env: gh.env(""),
    },
  );
  assert.equal(result.status, 1);
  assert.match(result.stderr, /gh api rate_limit answered "", not a count of requests\nnext: /);
  assert.deepEqual(readFileSync(gh.log, "utf8").trim().split("\n"), [
    "api rate_limit --jq .resources.core.remaining",
  ]);
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
    ["job", "--run", "1", "--job", "test-linux", "--out", "r", "--json", "./r"],
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

test("a run with no Nx trace takes its task times from the sidecar's telemetry", (t) => {
  const dir = scratch(t);
  const cache = join(dir, "cache");
  seedCache(cache, oneRun(dir, { noTrace: true }));
  const json = join(dir, "out.json");
  assert.equal(run(profileScript, job(cache, join(dir, "r.md"), ["--json", json])).status, 0);
  const r = JSON.parse(readFileSync(json, "utf8"));
  assert.equal(r.task_source, "sidecar telemetry");
  assert.deepEqual(
    r.tasks.map((x) => [x.task, x.result, x.s]),
    TASKS.map(([task, a, b]) => [task, "executed", b - a]),
  );
  const gate = Object.values(r.gate_categories).reduce((a, b) => a + b, 0);
  assert.ok(Math.abs(gate - 600) < 1e-6);
});

test("a failed gate's run falls back to the sidecar's telemetry, and says that its tasks are incomplete", (t) => {
  const dir = scratch(t);
  const cache = join(dir, "cache");
  seedCache(cache, oneRun(dir, { sidecarOnly: true }));
  const json = join(dir, "out.json");
  const result = run(profileScript, job(cache, join(dir, "r.md"), ["--json", json]));
  assert.equal(result.status, 0, result.stderr);
  const r = JSON.parse(readFileSync(json, "utf8"));
  assert.match(r.task_source, /^sidecar telemetry without the gate's invocation/);
  assert.deepEqual(
    r.tasks.map((x) => x.task),
    TASKS.map(([task]) => task),
  );
  const gate = Object.values(r.gate_categories).reduce((a, b) => a + b, 0);
  assert.ok(Math.abs(gate - 600) < 1e-6);
  assert.match(readFileSync(join(dir, "r.md"), "utf8"), /Nx sidecar manifest: not uploaded/);
});

test("nextest's own output, from the pinned cargo-nextest, is read into runs and per-binary seconds", (t) => {
  const dir = scratch(t);
  // A crate with one lib unit test and one integration test, run by the real nextest with a
  // line per test, as the cross-platform `ci` profile prints.
  const crate = join(dir, "probe");
  mkdirSync(join(crate, "src"), { recursive: true });
  mkdirSync(join(crate, "tests"));
  writeFileSync(
    join(crate, "Cargo.toml"),
    '[package]\nname = "probe"\nversion = "0.1.0"\nedition = "2021"\n\n[workspace]\n',
  );
  writeFileSync(
    join(crate, "src", "lib.rs"),
    "#[cfg(test)]\nmod tests {\n    #[test]\n    fn unit() {}\n}\n",
  );
  writeFileSync(join(crate, "tests", "it.rs"), "#[test]\nfn integration() {}\n");
  const env = { ...process.env, CARGO_TARGET_DIR: join(dir, "target") };
  delete env.RUSTC_WRAPPER;
  delete env.NEXTEST_PROFILE;
  const nextest = spawnSync(
    "cargo",
    ["nextest", "run", "--status-level", "pass", "--no-fail-fast", "--color", "never"],
    {
      cwd: crate,
      env,
      encoding: "utf8",
    },
  );
  assert.equal(nextest.status, 0, nextest.stderr);
  const output = `${nextest.stdout}${nextest.stderr}`.split("\n").filter((l) => /\S/.test(l));
  // A summary whose run's start line the log lost: kept, with no start and no task.
  output.push("     Summary [   5.000s] 9 tests run: 9 passed, 0 skipped");
  const log = output.map((line, i) => `${iso(400 + i / 1000)} ${line}`).join("\n");
  const cache = join(dir, "cache");
  seedCache(cache, oneRun(dir, { log }));
  const json = join(dir, "out.json");
  const result = run(profileScript, job(cache, join(dir, "r.md"), ["--json", json]));
  assert.equal(result.status, 0, result.stderr);
  const r = JSON.parse(readFileSync(json, "utf8"));
  const summary = Number(/Summary \[\s*([\d.]+)s\] 2 tests run/.exec(output.join("\n"))[1]);
  assert.deepEqual(
    r.nextest_runs.map((n) => [n.tests, n.binaries, n.s, n.started_at, n.tasks.length]),
    [
      [2, 2, summary, iso(400 + output.findIndex((l) => /Starting 2 tests/.test(l)) / 1000), 2],
      [9, null, 5, null, 0],
    ],
  );
  assert.deepEqual(Object.keys(r.log_binaries).sort(), ["it", "probe"]);
  assert.deepEqual(
    Object.values(r.log_binaries).map((b) => b.tests),
    [1, 1],
  );
});

test("cargo's own --timings report, from the pinned cargo, is read by unit kind and crate", (t) => {
  const dir = scratch(t);
  // The probe crate, built with --timings as the measurement's timed builds are.
  const crate = join(dir, "probe");
  mkdirSync(join(crate, "src"), { recursive: true });
  mkdirSync(join(crate, "tests"));
  writeFileSync(
    join(crate, "Cargo.toml"),
    '[package]\nname = "probe"\nversion = "0.1.0"\nedition = "2021"\n\n[workspace]\n',
  );
  writeFileSync(join(crate, "src", "lib.rs"), "pub fn probe() -> u8 { 7 }\n");
  writeFileSync(join(crate, "tests", "it.rs"), "#[test]\nfn integration() {}\n");
  const env = { ...process.env, CARGO_TARGET_DIR: join(dir, "target") };
  delete env.RUSTC_WRAPPER;
  const built = spawnSync("cargo", ["test", "--no-run", "--timings"], {
    cwd: crate,
    env,
    encoding: "utf8",
  });
  assert.equal(built.status, 0, built.stderr);
  const report = readFileSync(join(dir, "target", "cargo-timings", "cargo-timing.html"), "utf8");
  const cache = join(dir, "cache");
  seedCache(cache, oneRun(join(dir, "run")));
  const timings = join(artifactDir(cache, "measure-test-linux"), "timings");
  mkdirSync(timings);
  writeFileSync(join(timings, "plain.html"), report);
  writeFileSync(join(timings, "instrumented.html"), report);
  writeFileSync(join(timings, "times.txt"), "plain 1100 1160 0\ninstrumented 1160 1250 101\n");
  const json = join(dir, "out.json");
  const result = run(profileScript, job(cache, join(dir, "r.md"), ["--json", json]));
  assert.equal(result.status, 0, result.stderr);
  const r = JSON.parse(readFileSync(json, "utf8"));
  const plain = r.cargo_timings.plain;
  // The lib, the lib's unit-test target and the integration test: three units of probe.
  assert.equal(plain.units, 3);
  assert.deepEqual(Object.keys(plain.by_kind_s).sort(), ["lib", "test"]);
  assert.ok(plain.top.every((u) => u.unit.startsWith("probe")));
  assert.equal(plain.step_s, 60);
  assert.equal(r.cargo_timings.instrumented.step_s, 90);
  assert.ok(r.notes.some((n) => /the instrumented --timings build exited 101/.test(n)));
  assert.match(readFileSync(join(dir, "r.md"), "utf8"), /## Compile and link by cargo --timings/);
  // A report with no unit data is not cargo's.
  writeFileSync(join(timings, "plain.html"), "<html></html>");
  const refused = run(profileScript, job(cache, join(dir, "r.md")));
  assert.equal(refused.status, 1);
  assert.match(refused.stderr, /plain\.html is malformed\nnext: /);
});

test("a GitHub read that fails, or a report that cannot be written, refuses with what to do", (t) => {
  const dir = scratch(t);
  const served = oneRun(dir);
  delete served.answers[key(`repos/${REPO}/actions/jobs/9/logs`)];
  const gh = standInGh(dir, served);
  const failed = run(profileScript, job(join(dir, "cache"), join(dir, "r.md")), {
    env: gh.env(4000),
  });
  assert.equal(failed.status, 1);
  assert.match(
    failed.stderr,
    /gh api --allow-escape-sequences repos\/example\/onepipeline\/actions\/jobs\/9\/logs failed: gh: Not Found \(HTTP 404\)\nnext: check `gh auth status`/,
  );

  const cache = join(dir, "seeded");
  seedCache(cache, oneRun(join(dir, "second")));
  const unwritable = run(profileScript, job(cache, join(dir, "no", "such", "dir", "r.md")));
  assert.equal(unwritable.status, 1);
  assert.match(
    unwritable.stderr,
    /cannot write .*r\.md: .*\nnext: name a file in a directory that exists/,
  );
});

test("the workflow edits refuse a workflow that cannot be read or rewritten", (t) => {
  const dir = scratch(t);
  const absent = run(workflowScript, [join(checkout(dir), "..", "missing.yml")]);
  assert.equal(absent.status, 1);
  assert.match(absent.stderr, /cannot read .*missing\.yml: .*\nACTION: /);
  const file = checkout(join(dir, "read-only"));
  chmodSync(file, 0o444);
  const readOnly = run(workflowScript, [file]);
  assert.equal(readOnly.status, 1);
  assert.match(
    readOnly.stderr,
    /cannot write .*ci\.yml: .*\nACTION: make the workflow file writable/,
  );
});

test("the artifacts the analysis reads are the ones ci.yml and the measurement edits upload", (t) => {
  const file = checkout(scratch(t));
  assert.equal(run(workflowScript, [file]).status, 0);
  const edited = readFileSync(file, "utf8");
  for (const job of ["test-linux", "test-macos"]) {
    const start = edited.indexOf(`\n  ${job}:\n`);
    const next = edited.slice(start + 1).search(/\n {2}[a-z][\w-]*:\n/);
    const block = edited.slice(start, next < 0 ? undefined : start + 1 + next);
    const uploads = [...block.matchAll(/\n {10}name: (.+)\n/g)].map((m) =>
      // biome-ignore lint/suspicious/noTemplateCurlyInString: a GitHub Actions expression matched literally, not a JS template
      m[1].replace("${{ github.job }}", job),
    );
    for (const name of Object.values(artifactNames(job)))
      assert.ok(uploads.includes(name), `${job} uploads ${name}: ${uploads.join(", ")}`);
  }
});

function gitRepo(dir) {
  const git = (...args) => execFileSync("git", args, { cwd: dir, encoding: "utf8" }).trim();
  mkdirSync(dir);
  git("init", "-q", "-b", "main");
  git("config", "user.email", "spike@example.invalid");
  git("config", "user.name", "spike");
  writeFileSync(join(dir, "README.md"), "x\n");
  git("add", "-A");
  git("commit", "-qm", "base");
  // One pull-request head off main per kind of change.
  const head = (branch, files) => {
    git("checkout", "-qb", branch, "main");
    for (const [path, body] of Object.entries(files)) {
      mkdirSync(dirname(join(dir, path)), { recursive: true });
      writeFileSync(join(dir, path), body);
    }
    git("add", "-A");
    git("commit", "-qm", branch);
    return git("rev-parse", "HEAD");
  };
  const heads = {
    src: head("src", { "src/lib.rs": "\n" }),
    docs: head("docs", { "docs/a.md": "\n" }),
    crates: head("crates", { "crates/x/lib.rs": "\n" }),
    release: head("release", { "Cargo.toml": "\n", "CHANGELOG.md": "\n" }),
  };
  // A head pushed to origin after this clone last fetched: not in the clone until fetched.
  const origin = `${dir}-origin.git`;
  execFileSync("git", ["clone", "-q", "--bare", dir, origin]);
  const other = `${dir}-other`;
  execFileSync("git", ["clone", "-q", "-b", "main", origin, other]);
  const otherGit = (...args) => execFileSync("git", args, { cwd: other, encoding: "utf8" }).trim();
  otherGit("config", "user.email", "spike@example.invalid");
  otherGit("config", "user.name", "spike");
  mkdirSync(join(other, "src"));
  writeFileSync(join(other, "src", "late.rs"), "\n");
  otherGit("add", "-A");
  otherGit("commit", "-qm", "late");
  otherGit("push", "-q", "origin", "HEAD:refs/heads/late");
  heads.late = otherGit("rev-parse", "HEAD");
  git("remote", "add", "origin", origin);
  return heads;
}

test("every step of ci.yml's test jobs is classed as the analysis means it", (t) => {
  const file = checkout(scratch(t));
  assert.equal(run(workflowScript, [file]).status, 0);
  const edited = readFileSync(file, "utf8");
  for (const job of ["test-linux", "test-macos"]) {
    const start = edited.indexOf(`\n  ${job}:\n`);
    const next = edited.slice(start + 1).search(/\n {2}[a-z][\w-]*:\n/);
    const block = edited.slice(start, next < 0 ? undefined : start + 1 + next);
    // What GitHub calls each step: its name, or `Run <action>` for an unnamed one.
    const steps = [...block.matchAll(/\n {6}- (name|uses): (.+)\n/g)].map(([, kind, value]) =>
      kind === "name" ? value.replace(/^'(.*)'$/, "$1") : `Run ${value}`,
    );
    const classes = new Map(steps.map((name) => [name, stepClass(name)]));
    assert.equal(
      [...classes.values()].filter((c) => c === "gate (Nx tasks)").length,
      1,
      `${job} has one gate step`,
    );
    const expect = {
      "Run Swatinem/rust-cache@v2.9.2": "cache restore",
      "Restore the bootstrap tools": "cache restore",
      "Decide cache trust and configure the sidecar": "Nx cache sidecar",
      "Wait for the cache recording proxy": "Nx cache sidecar",
      "Keep the per-test profile records": "artifact upload",
      "Does each failed test fail again on this build": "rerun of failed tests",
      "Report cache entries and task executions": "cache save",
      "Run actions/upload-artifact@v4": "cache save",
    };
    for (const [name, cls] of Object.entries(expect))
      assert.equal(classes.get(name), cls, `${job}: ${name}`);
    for (const [name, cls] of classes)
      if (/measurement-only/.test(name)) assert.equal(cls, "measurement-only", name);
  }
});

test("each kind of step is summed into its class, and the classes are the job", (t) => {
  const dir = scratch(t);
  const served = oneRun(dir);
  const jobsKey = key(`repos/${REPO}/actions/runs/${RUN}/jobs?per_page=100`);
  const jobs = JSON.parse(served.answers[jobsKey]);
  jobs.jobs[0].steps = [
    ["Set up job", 0, 50],
    ["'Measure: sample CPU and network (measurement-only (spike-onepipeline-profile))'", 50, 60],
    ["Run Swatinem/rust-cache@v2.9.2", 60, 90],
    [
      "Tiers, MSRV, smoke, pinned-wheel journeys, install proofs and wheel (affected projects)",
      90,
      690,
    ],
    ["Does each failed test fail again on this build", 690, 692],
    ["Keep the per-test profile records", 692, 694],
    ["Report cache entries and task executions", 694, 696],
    ["Run actions/upload-artifact@v4", 696, 698],
    ["Post Run Swatinem/rust-cache@v2.9.2", 698, 700],
  ].map(([name, a, b], i) => ({
    name,
    number: i + 1,
    conclusion: "success",
    started_at: iso(a),
    completed_at: iso(b),
  }));
  served.answers[jobsKey] = JSON.stringify(jobs);
  const cache = join(dir, "cache");
  seedCache(cache, served);
  const json = join(dir, "out.json");
  assert.equal(run(profileScript, job(cache, join(dir, "r.md"), ["--json", json])).status, 0);
  const r = JSON.parse(readFileSync(json, "utf8"));
  assert.deepEqual(r.step_classes, {
    "setup and teardown": 50,
    "measurement-only": 10,
    "cache restore": 30,
    "gate (Nx tasks)": 600,
    "rerun of failed tests": 2,
    "artifact upload": 2,
    "cache save": 6,
  });
});

test("a GitHub answer or sidecar document not in its writer's shape refuses", (t) => {
  const cases = [
    ["run", `repos/${REPO}/actions/runs/${RUN}`, (v) => ({ ...v, head_sha: "x" })],
    ["jobs", `repos/${REPO}/actions/runs/${RUN}/jobs?per_page=100`, () => ({ jobs: "nope" })],
    [
      "artifact list",
      `repos/${REPO}/actions/runs/${RUN}/artifacts?per_page=100`,
      (v) => ({ artifacts: v.artifacts.map(({ expired, ...rest }) => rest) }),
    ],
    [
      "artifact list without creation times",
      `repos/${REPO}/actions/runs/${RUN}/artifacts?per_page=100`,
      (v) => ({ artifacts: v.artifacts.map(({ created_at, ...rest }) => rest) }),
    ],
  ];
  for (const [what, endpoint, change] of cases) {
    const dir = scratch(t);
    const served = oneRun(dir);
    served.answers[key(endpoint)] = JSON.stringify(
      change(JSON.parse(served.answers[key(endpoint)])),
    );
    const cache = join(dir, "cache");
    seedCache(cache, served);
    const result = run(profileScript, job(cache, join(dir, "r.md")));
    assert.equal(result.status, 1, what);
    assert.match(result.stderr, /is malformed\nnext: /, what);
  }
  // A manifest whose entries are not stored or replayed ones.
  const manifestDir = scratch(t);
  const manifestCache = join(manifestDir, "cache");
  seedCache(manifestCache, oneRun(manifestDir));
  const manifest = join(artifactDir(manifestCache, "nx-cache-entries-test-linux"), "manifest.json");
  writeFileSync(
    manifest,
    JSON.stringify({
      ...JSON.parse(readFileSync(manifest, "utf8")),
      entries: [{ source: "maybe" }],
    }),
  );
  const badManifest = run(profileScript, job(manifestCache, join(manifestDir, "r.md")));
  assert.equal(badManifest.status, 1);
  assert.match(badManifest.stderr, /manifest\.json is malformed\nnext: /);
  // The sidecar's report of another job is not this job's telemetry.
  const dir = scratch(t);
  const cache = join(dir, "cache");
  seedCache(cache, oneRun(dir));
  const executions = join(artifactDir(cache, "nx-cache-entries-test-linux"), "executions.json");
  writeFileSync(
    executions,
    JSON.stringify({ ...JSON.parse(readFileSync(executions, "utf8")), job: "test-macos" }),
  );
  const result = run(profileScript, job(cache, join(dir, "r.md")));
  assert.equal(result.status, 1);
  assert.match(result.stderr, /executions\.json is malformed/);
});

test("how test records split into categories, including unnamed spawns and a test that took no time", (t) => {
  const dir = scratch(t);
  const cache = join(dir, "cache");
  seedCache(cache, oneRun(dir));
  const records = join(artifactDir(cache, "test-profile-test-linux"), "e2e");
  rmSync(records, { recursive: true });
  mkdirSync(records);
  const multi = record("multi", 400, 40, { spawn: 30, git: 5 });
  multi.programs = {
    onepipeline: { count: 2, s: 20 },
    node: { count: 1, s: 10 },
    git: { count: 1, s: 5 },
  };
  const unnamed = record("unnamed", 400, 20, { spawn: 10 });
  unnamed.programs = {};
  const categories = record("categories", 400, 10, { build: 5, fixture: 3, fs: 2 });
  const zero = record("zero", 400, 0, {});
  for (const r of [multi, unnamed, categories, zero])
    writeFileSync(join(records, `${r.test}.json`), JSON.stringify(r));
  const json = join(dir, "out.json");
  assert.equal(run(profileScript, job(cache, join(dir, "r.md"), ["--json", json])).status, 0);
  const r = JSON.parse(readFileSync(json, "utf8"));
  const parts = (name) => r.slowest.find((x) => x.test === `e2e::${name}`).parts_s;
  assert.deepEqual(parts("multi"), {
    "spawn: onepipeline": 20,
    "spawn: node": 10,
    git: 5,
    "in-test computation (recorder-unattributed)": 5,
  });
  assert.deepEqual(parts("unnamed"), {
    "spawn: (unnamed)": 10,
    "in-test computation (recorder-unattributed)": 10,
  });
  assert.deepEqual(parts("categories"), {
    "build (cargo run by a test)": 5,
    fixture: 3,
    fs: 2,
  });
  assert.deepEqual(parts("zero"), {});
  const gate = Object.values(r.gate_categories).reduce((a, b) => a + b, 0);
  assert.ok(Math.abs(gate - 600) < 1e-6, `gate categories sum to ${gate}`);
});

test("time between a tier's tests, rustc in two trees at once, and a stray record are each counted once", (t) => {
  const dir = scratch(t);
  const cache = join(dir, "cache");
  seedCache(cache, oneRun(dir));

  // A third e2e test 20 s after the tier's others end, and a record no task's window holds.
  writeFileSync(
    join(artifactDir(cache, "test-profile-test-linux"), "e2e", "c.json"),
    JSON.stringify(record("c", 620, 20, { spawn: 20 })),
  );
  mkdirSync(join(artifactDir(cache, "test-profile-test-linux"), "stray"));
  const stray = record("s", 30, 5, { spawn: 5 });
  stray.binary = "stray";
  writeFileSync(
    join(artifactDir(cache, "test-profile-test-linux"), "stray", "s.json"),
    JSON.stringify(stray),
  );
  // A plain rustc beside the instrumented one, and one during the e2e tier's LCOV export.
  const tsv = join(artifactDir(cache, "measure-test-linux"), "rustc.tsv");
  const toolchain = "/home/runner/.rustup/toolchains/1.97.1-x86_64-unknown-linux-gnu/bin/rustc";
  const row = (a, b, dirName) =>
    [
      (T0 + a * 1000) / 1000,
      (T0 + b * 1000) / 1000,
      "onepipeline",
      "lib",
      0,
      `/home/runner/work/onepipeline/onepipeline/target/${dirName}/deps`,
      0,
      toolchain,
    ].join("\t");
  writeFileSync(
    tsv,
    `${row(300, 380, "debug")}\n${row(650, 670, "debug")}\n${readFileSync(tsv, "utf8")}`,
  );
  const json = join(dir, "out.json");
  assert.equal(run(profileScript, job(cache, join(dir, "r.md"), ["--json", json])).status, 0);
  const r = JSON.parse(readFileSync(json, "utf8"));
  const near = (category, seconds) =>
    assert.ok(
      Math.abs((r.gate_categories[category] ?? 0) - seconds) < 1e-6,
      `${category}: ${r.gate_categories[category]}`,
    );
  near("task: between tests (nextest scheduling)", 20);
  // The export runs from c's end at 640 s to the tier's end, rustc or not.
  near("task: after its last test (profdata merge, LCOV export)", 40);
  // 300-380 s: two tasks, each half the second, each half to each running tree.
  near("compile: plain (target/debug)", 150 + 40);
  near("compile: instrumented (llvm-cov-target)", 40);
  assert.equal(r.binaries.find((b) => b.binary === "stray").task, null);
  const gate = Object.values(r.gate_categories).reduce((a, b) => a + b, 0);
  assert.ok(Math.abs(gate - 600) < 1e-6, `gate categories sum to ${gate}`);
});

test("samples taken twice at one moment, a counter reset, an idle CPU, quiet network and setup time are read as such", (t) => {
  const dir = scratch(t);
  const served = oneRun(dir);
  // The upload starts 2 s after the gate ends, so an interval falls between steps.
  const jobsKey = key(`repos/${REPO}/actions/runs/${RUN}/jobs?per_page=100`);
  const jobs = JSON.parse(served.answers[jobsKey]);
  jobs.jobs[0].steps.find((st) => /Keep the per-test/.test(st.name)).started_at = iso(692);
  served.answers[jobsKey] = JSON.stringify(jobs);
  const cache = join(dir, "cache");
  seedCache(cache, served);
  const at = (s) => (T0 + s * 1000) / 1000;
  writeFileSync(
    join(artifactDir(cache, "measure-test-linux"), "samples.txt"),
    [
      `${at(30)} cpu 100 0 0 100 0 0 0 | 1/100 | 1000 0`,
      `${at(30)} cpu 100 0 0 100 0 0 0 | 1/100 | 1000 0`,
      `${at(32)} cpu 50 0 0 0 0 0 0 | 1/100 | 1000 0`,
      `${at(34)} cpu 50 0 0 0 0 0 0 | 1/100 | 1100 0`,
      `${at(680)} cpu 50 0 0 0 0 0 0 | 1/100 | 1100 0`,
      // 680-682 s: in the gate, between the e2e tier and the version task.
      `${at(682)} cpu 50 0 0 0 0 0 0 | 1/100 | 1300 0`,
      `${at(690)} cpu 50 0 0 0 0 0 0 | 1/100 | 1300 0`,
      // 690-692 s: between the gate step and the upload.
      `${at(692)} cpu 50 0 0 0 0 0 0 | 1/100 | 1700 0`,
      "",
    ].join("\n"),
  );
  const json = join(dir, "out.json");
  assert.equal(run(profileScript, job(cache, join(dir, "r.md"), ["--json", json])).status, 0);
  const s = JSON.parse(readFileSync(json, "utf8")).samples;
  assert.equal(s.counter_resets, 1);
  assert.equal(s.cpu_busy_fraction, null);
  assert.equal(s.network_bytes, 700);
  assert.equal(s.network_active_s, 0);
  assert.deepEqual(s.network_by_place, {
    "step: setup and teardown": { bytes: 100, active_s: 0 },
    "gate: demo-e2e:test + demo:build": { bytes: 0, active_s: 0 },
    "gate: no task": { bytes: 200, active_s: 0 },
    "gate: demo-version:smoke": { bytes: 0, active_s: 0 },
    "between steps": { bytes: 400, active_s: 0 },
  });
});

test("a graph step that wrote no graph leaves no critical path, and the report says why", (t) => {
  const dir = scratch(t);
  const cache = join(dir, "cache");
  seedCache(cache, oneRun(dir));
  writeFileSync(
    join(artifactDir(cache, "measure-test-linux"), "graph.json"),
    "NX   Could not plan\n",
  );
  const json = join(dir, "out.json");
  assert.equal(run(profileScript, job(cache, join(dir, "r.md"), ["--json", json])).status, 0);
  const r = JSON.parse(readFileSync(json, "utf8"));
  assert.deepEqual(r.critical_path, []);
  assert.ok(
    r.notes.some((n) => /graph\.json holds no graph, so the report has no critical path/.test(n)),
  );
});

test("a CPU counter whose idle falls, or grows past the total, is a reset", (t) => {
  for (const [what, second] of [
    ["idle falls", "cpu 300 0 0 50 0 0 0"],
    ["idle outgrows the total", "cpu 50 0 0 300 0 0 0"],
  ]) {
    const dir = scratch(t);
    const cache = join(dir, "cache");
    seedCache(cache, oneRun(dir));
    const at = (s) => (T0 + s * 1000) / 1000;
    writeFileSync(
      join(artifactDir(cache, "measure-test-linux"), "samples.txt"),
      `${at(30)} cpu 100 0 0 100 0 0 0 | 1/1 | 0 0\n${at(32)} ${second} | 1/1 | 0 0\n`,
    );
    const json = join(dir, "out.json");
    assert.equal(
      run(profileScript, job(cache, join(dir, "r.md"), ["--json", json])).status,
      0,
      what,
    );
    const s = JSON.parse(readFileSync(json, "utf8")).samples;
    assert.equal(s.counter_resets, 1, what);
    assert.equal(s.cpu_busy_fraction, null, what);
  }
});

test("the graph Nx itself writes for a task is read as the dependency map", (t) => {
  const dir = scratch(t);
  const cache = join(dir, "cache");
  seedCache(cache, oneRun(dir));
  // The pinned Nx's own --graph=stdout, as the measurement's graph step runs it.
  const graph = execFileSync(
    "bash",
    [
      join(root, "scripts", "nx.sh"),
      "run-many",
      "-t",
      "test",
      "-p",
      "onepipeline-ci-time-profile",
      "--graph=stdout",
    ],
    {
      cwd: root,
      encoding: "utf8",
      env: { ...process.env, ONEPIPELINE_NX_SHOW_OUTPUT: "1", NX_DAEMON: "false" },
    },
  );
  const parsed = JSON.parse(graph.slice(graph.indexOf("{")));
  assert.ok(Object.hasOwn(parsed.tasks.dependencies, "onepipeline-ci-time-profile:test"));
  writeFileSync(join(artifactDir(cache, "measure-test-linux"), "graph.json"), graph);
  const json = join(dir, "out.json");
  const result = run(profileScript, job(cache, join(dir, "r.md"), ["--json", json]));
  assert.equal(result.status, 0, result.stderr);
  // That graph names none of this run's tasks, so no path runs through it.
  assert.deepEqual(
    JSON.parse(readFileSync(json, "utf8")).critical_path.map((c) => c.task),
    ["demo-version:smoke"],
  );
});

test("a run listing or job list not in GitHub's shape refuses the history", (t) => {
  const listingKey = key(
    `repos/${REPO}/actions/workflows/ci.yml/runs?event=pull_request&status=completed&per_page=100&page=1&created=2026-11-04T00:00:00..2026-11-13T00:00:00`,
  );
  const listed = {
    id: 1,
    run_attempt: 1,
    head_sha: "a".repeat(40),
    created_at: "2026-11-11T10:00:00Z",
    conclusion: "success",
  };
  for (const [what, answers, message] of [
    [
      "a run with no head",
      { [listingKey]: JSON.stringify({ workflow_runs: [{ ...listed, head_sha: "" }] }) },
      /ci\.yml's run listing, page 1 is malformed/,
    ],
    [
      "a successful job that ends before it starts",
      {
        [listingKey]: JSON.stringify({ workflow_runs: [listed] }),
        [key(`repos/${REPO}/actions/runs/1/attempts/1/jobs?per_page=100`)]: JSON.stringify({
          jobs: [
            {
              name: "test-linux",
              conclusion: "success",
              started_at: "2026-11-11T11:00:00Z",
              completed_at: "2026-11-11T10:00:00Z",
            },
          ],
        }),
      },
      /run 1's jobs is malformed/,
    ],
  ]) {
    const dir = scratch(t);
    const gh = standInGh(dir, { answers, files: dir, names: [] });
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
        "--repo",
        REPO,
        "--cache",
        join(dir, "cache"),
        "--out",
        join(dir, "h.md"),
      ],
      { env: gh.env(4000) },
    );
    assert.equal(result.status, 1, what);
    assert.match(result.stderr, message, what);
    assert.match(result.stderr, /\nnext: /, what);
  }
});

test("a listing at GitHub's 1,000-run limit is noted, not silently cut", (t) => {
  const dir = scratch(t);
  const answers = {};
  for (let page = 1; page <= 10; page++)
    answers[
      key(
        `repos/${REPO}/actions/workflows/ci.yml/runs?event=pull_request&status=completed&per_page=100&page=${page}&created=2026-11-04T00:00:00..2026-11-13T00:00:00`,
      )
    ] = JSON.stringify({
      workflow_runs: Array.from({ length: 100 }, (_, i) => ({
        id: page * 1000 + i,
        run_attempt: 1,
        head_sha: "f".repeat(40),
        created_at: "2026-11-04T01:00:00Z",
        conclusion: "success",
      })),
    });
  const gh = standInGh(dir, { answers, files: dir, names: [] });
  const result = run(
    profileScript,
    [
      "history",
      "--since",
      "2026-11-04T00:00:00Z",
      "--compare-since",
      "2026-11-05T00:00:00Z",
      "--split",
      "2026-11-10T00:00:00Z",
      "--until",
      "2026-11-13T00:00:00Z",
      "--repo",
      REPO,
      "--cache",
      join(dir, "cache"),
      "--out",
      join(dir, "h.md"),
    ],
    { env: gh.env(4000) },
  );
  assert.equal(result.status, 0, result.stderr);
  assert.match(
    result.stdout,
    /1000 runs listed, 0 successful jobs counted; .*\(GitHub requests 10, .*1 note\(s\)/,
  );
  assert.match(readFileSync(join(dir, "h.md"), "utf8"), /GitHub's limit for a filtered listing/);
});

test("an overhead partition whose runs share no test reports no ratio", (t) => {
  const dir = scratch(t);
  const cache = join(dir, "cache");
  seedCache(cache, oneRun(dir));
  const plain = join(artifactDir(cache, "measure-test-linux"), "overhead", "plain-records", "e2e");
  rmSync(plain, { recursive: true });
  mkdirSync(plain);
  writeFileSync(join(plain, "y.json"), JSON.stringify(record("y", 1061, 8, { spawn: 4 })));
  const json = join(dir, "out.json");
  assert.equal(run(profileScript, job(cache, join(dir, "r.md"), ["--json", json])).status, 0);
  const o = JSON.parse(readFileSync(json, "utf8")).overhead;
  assert.deepEqual(
    [o.tests_in_both, o.instrumented_test_seconds, o.plain_test_seconds, o.ratio],
    [0, 0, 0, null],
  );
});

test("a gate that does not end on a whole second is accounted to its last fraction", (t) => {
  const dir = scratch(t);
  const served = oneRun(dir);
  const jobsKey = key(`repos/${REPO}/actions/runs/${RUN}/jobs?per_page=100`);
  const jobs = JSON.parse(served.answers[jobsKey]);
  const steps = jobs.jobs[0].steps;
  const gate = steps.find((st) => /affected projects/.test(st.name));
  const upload = steps.find((st) => /Keep the per-test/.test(st.name));
  gate.completed_at = iso(690.5);
  upload.started_at = iso(690.5);
  served.answers[jobsKey] = JSON.stringify(jobs);
  const cache = join(dir, "cache");
  seedCache(cache, served);
  const json = join(dir, "out.json");
  assert.equal(run(profileScript, job(cache, join(dir, "r.md"), ["--json", json])).status, 0);
  const r = JSON.parse(readFileSync(json, "utf8"));
  const sum = (o) => Object.values(o).reduce((a, b) => a + b, 0);
  assert.equal(r.gate_s, 600.5);
  assert.ok(Math.abs(sum(r.gate_categories) - 600.5) < 1e-6, `${sum(r.gate_categories)}`);
  assert.ok(Math.abs(sum(r.concurrency_profile) - 600.5) < 1e-6, `${sum(r.concurrency_profile)}`);
  assert.ok(
    Math.abs(r.gate_categories["gate: after the last task (lockfile check, exit)"] - 2.5) < 1e-6,
  );
});

test("a GitHub answer that is not JSON refuses, naming the endpoint", (t) => {
  const dir = scratch(t);
  const served = oneRun(dir);
  served.answers[key(`repos/${REPO}/actions/runs/${RUN}/artifacts?per_page=100`)] =
    "<html>rate limited</html>";
  const cache = join(dir, "cache");
  seedCache(cache, served);
  const result = run(profileScript, job(cache, join(dir, "r.md")));
  assert.equal(result.status, 1);
  assert.match(
    result.stderr,
    /GitHub's answer to repos\/example\/onepipeline\/actions\/runs\/42\/artifacts\?per_page=100 is malformed\nnext: /,
  );
});

test("an overhead run that failed is noted beside the figures it leaves short", (t) => {
  const dir = scratch(t);
  const cache = join(dir, "cache");
  seedCache(cache, oneRun(dir));
  writeFileSync(
    join(artifactDir(cache, "measure-test-linux"), "overhead", "times.txt"),
    "instrumented 1000 1060 0\nplain-build 1060 1061 0\nplain 1061 1121 101\n",
  );
  const json = join(dir, "out.json");
  assert.equal(run(profileScript, job(cache, join(dir, "r.md"), ["--json", json])).status, 0);
  const r = JSON.parse(readFileSync(json, "utf8"));
  assert.equal(r.overhead.runs.plain.status, 101);
  assert.ok(r.notes.some((n) => /the overhead's plain run exited 101/.test(n)));
});

test("a re-run's newer artifact is read over the earlier attempt's, and that is noted", (t) => {
  const dir = scratch(t);
  const served = oneRun(dir);
  // The earlier attempt's records: test a only, ten times slower.
  const older = join(served.files, "older-profile", "e2e");
  mkdirSync(older, { recursive: true });
  writeFileSync(join(older, "a.json"), JSON.stringify(record("a", 400, 2000, { spawn: 2000 })));
  served.names.push("older-profile");
  const listKey = key(`repos/${REPO}/actions/runs/${RUN}/artifacts?per_page=100`);
  const list = JSON.parse(served.answers[listKey]);
  // The stand-in serves an artifact's zip by its position in `names`: the older one is id 3.
  // Listed first, as an earlier attempt's would be.
  list.artifacts = [
    { id: 3, name: "test-profile-test-linux", expired: false, created_at: "2026-11-13T09:00:00Z" },
    ...list.artifacts.map((a) => ({ ...a, created_at: "2026-11-13T12:00:00Z" })),
  ];
  served.answers[listKey] = JSON.stringify(list);
  const gh = standInGh(join(dir, "gh"), served);
  const json = join(dir, "out.json");
  const result = run(profileScript, job(join(dir, "cache"), join(dir, "r.md"), ["--json", json]), {
    env: gh.env(4000),
  });
  assert.equal(result.status, 0, result.stderr);
  const r = JSON.parse(readFileSync(json, "utf8"));
  assert.equal(r.slowest.find((x) => x.test === "e2e::a").wall_s, 200);
  assert.ok(
    r.notes.some((n) =>
      /2 unexpired artifacts named test-profile-test-linux; the newest, created 2026-11-13T12:00:00Z, was read/.test(
        n,
      ),
    ),
  );
});

test("a download that does not unzip is dropped, so the next read fetches it afresh", (t) => {
  const dir = scratch(t);
  const cache = join(dir, "cache");
  const served = oneRun(dir);
  const gh = standInGh(join(dir, "gh"), served);
  const zip = join(dir, "gh", "answers", key(`repos/${REPO}/actions/artifacts/0/zip`));
  const good = readFileSync(zip);
  writeFileSync(zip, "not a zip");
  const broken = run(profileScript, job(cache, join(dir, "r.md")), { env: gh.env(4000) });
  assert.equal(broken.status, 1);
  assert.match(
    broken.stderr,
    /artifact test-profile-test-linux \(0\) did not unzip: .*\nnext: run the same command again/,
  );
  writeFileSync(zip, good);
  const again = run(profileScript, job(cache, join(dir, "r.md")), { env: gh.env(4000) });
  assert.equal(again.status, 0, again.stderr);
});

test("one cache serves two repositories' runs of the same id without mixing them", (t) => {
  const dir = scratch(t);
  const cache = join(dir, "cache");
  const first = oneRun(join(dir, "first"));
  const ours = standInGh(join(dir, "gh-ours"), first);
  assert.equal(
    run(profileScript, job(cache, join(dir, "ours.md")), { env: ours.env(4000) }).status,
    0,
  );
  // The other repository's run 42: test a ten times slower.
  const other = "other/onepipeline";
  const second = oneRun(join(dir, "second"));
  writeFileSync(
    join(second.files, "test-profile-test-linux", "e2e", "a.json"),
    JSON.stringify(record("a", 400, 2000, { spawn: 2000 })),
  );
  second.answers = Object.fromEntries(
    Object.entries(second.answers).map(([k, v]) => [k.replace(key(REPO), key(other)), v]),
  );
  const theirs = standInGh(join(dir, "gh-theirs"), second, other);
  const json = join(dir, "theirs.json");
  const args = job(cache, join(dir, "theirs.md"), ["--json", json]).map((a) =>
    a === REPO ? other : a,
  );
  assert.equal(run(profileScript, args, { env: theirs.env(4000) }).status, 0);
  assert.equal(JSON.parse(readFileSync(json, "utf8")).slowest[0].wall_s, 2000);
});

test("a newer artifact listed for a run already measured is extracted afresh, not served from the old one", (t) => {
  const dir = scratch(t);
  const cache = join(dir, "cache");
  const served = oneRun(dir);
  const gh = standInGh(join(dir, "gh"), served);
  assert.equal(
    run(profileScript, job(cache, join(dir, "one.md")), { env: gh.env(4000) }).status,
    0,
  );
  // GitHub now lists a newer test-profile artifact, id 3, from a re-run: test a ten times slower.
  const newer = join(served.files, "newer-profile", "e2e");
  mkdirSync(newer, { recursive: true });
  writeFileSync(join(newer, "a.json"), JSON.stringify(record("a", 400, 2000, { spawn: 2000 })));
  served.names.push("newer-profile");
  const listKey = key(`repos/${REPO}/actions/runs/${RUN}/artifacts?per_page=100`);
  const list = JSON.parse(served.answers[listKey]);
  list.artifacts.push({
    id: 3,
    name: "test-profile-test-linux",
    expired: false,
    created_at: iso(800),
  });
  served.answers[listKey] = JSON.stringify(list);
  const regh = standInGh(join(dir, "regh"), served);
  rmSync(join(cache, "api", `${listKey}.txt`));
  const json = join(dir, "two.json");
  const again = run(profileScript, job(cache, join(dir, "two.md"), ["--json", json]), {
    env: regh.env(4000),
  });
  assert.equal(again.status, 0, again.stderr);
  assert.equal(JSON.parse(readFileSync(json, "utf8")).slowest[0].wall_s, 2000);
});

test("the history counts each finished job by what its pull request changed, before and after the split", (t) => {
  const dir = scratch(t);
  const repo = join(dir, "repo");
  const heads = gitRepo(repo);
  const unknownHead = "e".repeat(40);
  const answers = {};
  const real = [
    // [run, head, created, test-linux minutes, conclusion]
    [1, heads.src, "2026-11-08T10:00:00Z", 100],
    [2, heads.src, "2026-11-11T10:00:00Z", 120],
    [3, heads.docs, "2026-11-11T11:00:00Z", 30],
    [4, heads.src, "2026-11-12T10:00:00Z", 130],
    [6, heads.crates, "2026-11-12T11:00:00Z", 50],
    [7, heads.release, "2026-11-12T12:00:00Z", 40],
    [8, unknownHead, "2026-11-12T13:00:00Z", 60],
    [10, heads.late, "2026-11-12T14:00:00Z", 125],
    [11, heads.docs, "2026-11-12T15:00:00Z", 50],
  ];
  const listed = (id, head, created, conclusion = "success") => ({
    id,
    run_attempt: 1,
    head_sha: head,
    created_at: created,
    conclusion,
  });
  // A full first page: 98 runs from before --compare-since, which are listed and not read,
  // then two of the counted runs; the second page holds the rest.
  const page1 = [
    ...Array.from({ length: 98 }, (_, i) =>
      listed(
        1000 + i,
        heads.src,
        new Date(Date.parse("2026-11-04T01:00:00Z") + i * 60000).toISOString(),
      ),
    ),
    ...real.slice(0, 2).map(([id, head, created]) => listed(id, head, created)),
  ];
  const page2 = [
    ...real.slice(2).map(([id, head, created]) => listed(id, head, created)),
    listed(5, heads.src, "2026-11-12T11:30:00Z", "cancelled"),
    listed(9, heads.src, "2026-11-12T11:40:00Z", "skipped"),
  ];
  const listing = (page) =>
    key(
      `repos/${REPO}/actions/workflows/ci.yml/runs?event=pull_request&status=completed&per_page=100&page=${page}&created=2026-11-04T00:00:00..2026-11-13T00:00:00`,
    );
  answers[listing(1)] = JSON.stringify({ workflow_runs: page1 });
  answers[listing(2)] = JSON.stringify({ workflow_runs: page2 });
  for (const [id, , created, minutes] of real) {
    const start = Date.parse(created);
    const end = (m) => new Date(start + m * 60000).toISOString();
    answers[key(`repos/${REPO}/actions/runs/${id}/attempts/1/jobs?per_page=100`)] = JSON.stringify({
      jobs: [
        {
          name: "test-linux",
          conclusion: "success",
          started_at: created,
          completed_at: end(minutes),
        },
        // A failed job is not a duration of record.
        {
          name: "test-linux-extra",
          conclusion: "failure",
          started_at: created,
          completed_at: end(1),
        },
        { name: "test-macos", conclusion: "failure", started_at: created, completed_at: end(5) },
      ],
    });
  }
  // The src/ runs' e2e records: run 1 spends 2 s a spawn and run 2 1 s; run 4's has expired.
  const artifactFiles = join(dir, "artifact-files");
  const names = [];
  for (const [id, spawnS] of [
    [1, 2],
    [2, 1],
  ]) {
    const name = `test-profile-test-linux-${id}`;
    const d = join(artifactFiles, name, "e2e");
    mkdirSync(d, { recursive: true });
    writeFileSync(join(d, "t.json"), JSON.stringify(record("t", 0, 10, { spawn: 4 * spawnS })));
    names.push(name);
    answers[key(`repos/${REPO}/actions/runs/${id}/artifacts?per_page=100`)] = JSON.stringify({
      artifacts: [
        {
          id: names.length - 1,
          name: "test-profile-test-linux",
          expired: false,
          created_at: "2026-11-12T00:00:00Z",
        },
      ],
    });
  }
  answers[key(`repos/${REPO}/actions/runs/4/artifacts?per_page=100`)] = JSON.stringify({
    artifacts: [
      {
        id: 99,
        name: "test-profile-test-linux",
        expired: true,
        created_at: "2026-10-01T00:00:00Z",
      },
    ],
  });
  answers[key(`repos/${REPO}/actions/runs/11/artifacts?per_page=100`)] = JSON.stringify({
    artifacts: [],
  });
  answers[key(`repos/${REPO}/actions/runs/10/artifacts?per_page=100`)] = JSON.stringify({
    artifacts: [],
  });
  const gh = standInGh(join(dir, "gh"), { answers, files: artifactFiles, names });
  const out = join(dir, "history.md");
  const json = join(dir, "history.json");
  const result = run(
    profileScript,
    [
      "history",
      "--since",
      "2026-11-04T00:00:00Z",
      "--compare-since",
      "2026-11-05T00:00:00Z",
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
  // Two listing pages, nine job lists, four artifact lists, two artifacts.
  assert.match(
    result.stdout,
    /^ci-time-profile history: 109 runs listed, 9 successful jobs counted; report in .* \(GitHub requests 17, rate checks 17, read from cache 0\); 3 note\(s\)/,
  );
  const h = JSON.parse(readFileSync(json, "utf8"));
  const summary = (cls, side) => h.summary[`test-linux\t${cls}\t${side}`];
  assert.deepEqual(summary("edits src/", "before"), {
    n: 1,
    median_min: 100,
    min_min: 100,
    max_min: 100,
  });
  // Run 10's head was fetched from origin, then classed.
  assert.deepEqual(summary("edits src/", "after"), {
    n: 3,
    median_min: 125,
    min_min: 120,
    max_min: 130,
  });
  // Two docs runs, 30 and 50 min: an even population's median is the middle pair's mean.
  assert.deepEqual(summary("other", "after"), { n: 2, median_min: 40, min_min: 30, max_min: 50 });
  assert.equal(summary("edits crates/ or Cargo files, not src/", "after").median_min, 50);
  assert.equal(summary("release", "after").median_min, 40);
  assert.equal(summary("unknown", "after").median_min, 60);
  assert.equal(summary("all", "after").n, 8);
  assert.deepEqual(
    h.rows
      .filter((r) => r.class === "edits src/")
      .map((r) => [r.run, r.e2e_tests ?? null, r.onepipeline_spawn_s ?? null]),
    [
      [1, 1, 2],
      [2, 1, 1],
      [4, null, null],
      [10, null, null],
    ],
  );
  assert.match(
    h.notes[0],
    new RegExp(`head ${unknownHead} could not be classed: .*\\(next: run from a clone`),
  );
  assert.match(h.notes[1], /run 4 has no unexpired test-profile-test-linux artifact/);
  assert.match(h.notes[2], /run 10 has no unexpired test-profile-test-linux artifact/);
  assert.match(
    readFileSync(out, "utf8"),
    /\| test-linux \| edits src\/ \| after \| 3 \| 125\.0 \|/,
  );
});
