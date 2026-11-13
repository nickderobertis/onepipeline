// The Windows job profile harness, run as its command over a job's log, its profile records
// and its Nx executions in the shapes a hosted run leaves them.
import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { after, before, test } from "node:test";
import { fileURLToPath } from "node:url";

const root = join(dirname(fileURLToPath(import.meta.url)), "..", "..");
const script = join(root, "scripts", "windows-job-profile.mjs");
// A record a real test wrote, so the records below are the harness's own shape.
const golden = JSON.parse(
  readFileSync(join(root, "tests", "golden", "test-profile-record-v2.json"), "utf8"),
);

const T0 = Date.parse("2026-11-12T08:00:00Z") / 1000;
const at = (s) => new Date((T0 + s) * 1000).toISOString().replace(".000Z", ".0000000Z");

let scratch;
before(() => {
  scratch = mkdtempSync(join(tmpdir(), "windows-job-profile-"));
});
after(() => rmSync(scratch, { recursive: true, force: true }));

function run(...args) {
  return spawnSync(process.execPath, [script, ...args], { encoding: "utf8" });
}

// A job of 1,000 s: 60 s of setup, 40 s restoring caches, 100 s of bootstrap, a 780 s gate
// and 20 s of post steps, 10 of them saving a cache.
function jobLog({ cpu } = {}) {
  const lines = [
    [0, "Current runner version: '2.338.2'"],
    [10, "##[group]Run actions/checkout@v4"],
    ...(cpu ? [[20, `Name                      : ${cpu}                `]] : []),
    [60, "##[group]Run Swatinem/rust-cache@v2.9.2"],
    [99, "Cache restored successfully"],
    [100, "##[group]Run just bootstrap"],
    [
      200,
      '##[group]Run ONEPIPELINE_TEST_ARCHIVE="$RUNNER_TEMP/tests.tar.zst" just ci-test-windows',
    ],
    [201, "ci-job: test-windows runs 2 task(s): onepipeline:lint onepipeline-e2e:test-quick"],
    [202, "\u001b[2m> \u001b[22mnx run onepipeline:lint"],
    [290, "    Finished `dev` profile [unoptimized + debuginfo] target(s) in 1m 28.50s"],
    [300, "> nx run onepipeline-e2e:test-quick"],
    [400, "    Starting 2 tests across 1 binary (47 binaries skipped)"],
    [450, "        PASS [ 50.000s] (1/2) onepipeline::e2e readjust::a_slow_one"],
    [900, "        PASS [400.000s] (2/2) onepipeline::e2e adoption::a_slower_one"],
    [900, "     Summary [ 500.000s] 2 tests run: 2 passed, 0 skipped"],
    [980, "##[group]Run actions/upload-artifact@v4"],
    [990, "Post job cleanup."],
    [995, "cache saved with the key: setup-uv-1-x86_64"],
    [1000, "Cleaning up orphan processes"],
  ];
  return `﻿${lines.map(([s, text]) => `${at(s)} ${text}`).join("\n")}\n`;
}

function record(test, startedAt, wall, { spawn = 0, git = 0, wait = 0, programs = {} }) {
  const r = structuredClone(golden);
  r.test = test;
  r.run_id = "run-a";
  r.started_at = T0 + startedAt;
  r.wall_s = wall;
  for (const c of Object.keys(r.categories)) {
    Object.assign(r.categories[c], { count: 0, s: 0, inclusive_s: 0 });
  }
  r.categories.spawn.s = spawn;
  r.categories.git.s = git;
  r.categories.wait.s = wait;
  r.programs = programs;
  r.unattributed_s = wall - spawn - git - wait;
  return r;
}

function job(name, { records = true, executions = true, cpu } = {}) {
  const dir = join(scratch, name);
  mkdirSync(join(dir, "test-profile", "e2e"), { recursive: true });
  writeFileSync(join(dir, "job.log"), jobLog({ cpu }));
  if (records) {
    // 600 test-seconds in a 500 s test phase: two thirds onepipeline, a sixth git, a
    // sixth waiting.
    const rs = [
      record("readjust::a_slow_one", 401, 200, {
        spawn: 150,
        git: 50,
        programs: { onepipeline: { count: 3, s: 150 }, git: { count: 10, s: 50 } },
      }),
      {
        // A second nextest run inside the same window still counts.
        ...record("adoption::a_slower_one", 402, 400, {
          spawn: 250,
          git: 50,
          wait: 100,
          programs: { onepipeline: { count: 1, s: 200 }, onevcs: { count: 1, s: 50 } },
        }),
        run_id: "run-c",
      },
      // A record a restored cache carried in from another job, outside every test phase,
      // which the reader sets aside.
      { ...record("old::one", 10, 999, {}), run_id: "run-b" },
    ];
    rs.forEach((r, i) => {
      writeFileSync(join(dir, "test-profile", "e2e", `${i}.json`), JSON.stringify(r));
    });
    writeFileSync(join(dir, "test-profile", "e2e", ".staged.json"), "{");
  }
  if (executions) {
    writeFileSync(
      join(dir, "executions.json"),
      JSON.stringify({
        schema: 2,
        tasks: [
          {
            task: "onepipeline:lint",
            result: "executed",
            started_at: at(202),
            finished_at: at(298),
          },
          {
            task: "onepipeline-npm:test-launcher",
            result: "executed",
            started_at: at(205),
            finished_at: at(260),
          },
          {
            task: "onepipeline-e2e:test-quick",
            result: "executed",
            started_at: at(300),
            finished_at: at(910),
          },
        ],
      }),
    );
  }
  return dir;
}

/** The `| part | ... |` rows of the division table, as numbers by part. */
function division(stdout, column = 1) {
  const table = stdout.split("\n## ")[0];
  const rows = new Map();
  for (const line of table
    .split("\n")
    .filter((l) => l.startsWith("| ") && !l.startsWith("| part"))) {
    const cells = line.split(" | ").map((c) => c.replace(/^\| | \|$/g, ""));
    if (cells[0].startsWith("---")) continue;
    rows.set(cells[0], Number(cells[column]));
  }
  return rows;
}

test("a job's wall clock is divided into parts that add up to it, the tests by what they spent", () => {
  const r = run("fold", `windows=${job("windows")}`);
  assert.equal(r.status, 0, r.stderr);
  const rows = division(r.stdout);
  assert.equal(rows.get("**job wall clock**"), 16.7);
  assert.equal(rows.get("step: setup"), 1.0);
  assert.equal(rows.get("step: cache-restore"), 0.7);
  assert.equal(rows.get("step: bootstrap"), 1.7);
  assert.equal(rows.get("step: cache-save"), 0.2);
  // The critical path is lint, then the e2e tier: test-launcher ran beside lint.
  assert.equal(rows.get("gate: onepipeline lints"), undefined);
  assert.equal(rows.get("gate: onepipeline:lint"), 1.6);
  assert.equal(rows.get("gate: onepipeline-e2e tests build, list and archive"), 1.7);
  // The 500 s test phase split by the 600 test-seconds its records spent.
  assert.equal(rows.get("tests: spawn: onepipeline"), 4.9);
  assert.equal(rows.get("tests: git"), 1.4);
  assert.equal(rows.get("tests: spawn: onevcs"), 0.7);
  assert.equal(rows.get("tests: wait"), 1.4);
  const sum = [...rows].filter(([k]) => k !== "**job wall clock**").reduce((a, [, v]) => a + v, 0);
  assert.ok(Math.abs(sum - 16.7) < 0.3, `the parts sum to ${sum}, not the job's 16.7 minutes`);
  assert.match(r.stdout, /\| onepipeline-npm:test-launcher \| executed \| 0\.1 \| 0\.9 \| {2}\|/);
  assert.match(r.stdout, /\| onepipeline-e2e:test-quick \| executed \| 1\.6 \| 10\.2 \| yes \|/);
  assert.match(r.stdout, /\| dev \| 1\.5 \| 1\.5 \|/);
  assert.match(r.stdout, /\| e2e::adoption::a_slower_one \| 400\.0 \|/);
  assert.doesNotMatch(r.stdout, /old::one/);
});

test("set beside a baseline, each part's difference is what the other job adds", () => {
  const r = run(
    "fold",
    "--baseline",
    "linux",
    `windows=${job("w2")}`,
    `linux=${job("l2", { records: false })}`,
  );
  assert.equal(r.status, 0, r.stderr);
  const added = division(r.stdout, 5);
  assert.equal(added.get("tests: spawn: onepipeline"), 4.9);
  assert.equal(added.get("tests: onepipeline-e2e tests (no profile records)"), -8.3);
  assert.equal(added.get("**job wall clock**"), 0);
});

test("each job names the processor its runner printed, or says it printed none", () => {
  const r = run(
    "fold",
    `xeon=${job("c1", { cpu: "INTEL(R) XEON(R) PLATINUM 8573C" })}`,
    `quiet=${job("c2")}`,
  );
  assert.equal(r.status, 0, r.stderr);
  assert.match(
    r.stdout,
    /\n## xeon\n\n[^\n]*; runner processor INTEL\(R\) XEON\(R\) PLATINUM 8573C\.\n/,
  );
  assert.match(r.stdout, /\n## quiet\n\n[^\n]*; runner processor not printed by the job\.\n/);
});

test("without Nx's executions, the test phases are the path and the rest is named scheduling", () => {
  const r = run("fold", `windows=${job("w3", { executions: false })}`);
  assert.equal(r.status, 0, r.stderr);
  const rows = division(r.stdout);
  assert.equal(rows.get("gate: onepipeline-e2e tests build, list and archive"), 1.7);
  assert.equal(rows.get("gate: between critical-path tasks (Nx scheduling)"), 3.0);
});

test("every baseline pass that another job did not pass is named", () => {
  const base = join(job("o1"), "job.log");
  const other = join(scratch, "o2.log");
  writeFileSync(
    other,
    jobLog().replace(
      "PASS [400.000s] (2/2) onepipeline::e2e adoption::a_slower_one",
      "FAIL [400.000s] (2/2) onepipeline::e2e adoption::a_slower_one",
    ),
  );
  const r = run("outcomes", `baseline=${base}`, `same=${base}`, `other=${other}`);
  assert.equal(r.status, 0, r.stderr);
  assert.match(r.stdout, /\| same \| 2 \| 2 \| 0 \| none \|/);
  assert.match(
    r.stdout,
    /\| other \| 2 \| 1 \| 1 \| 1: onepipeline::e2e adoption::a_slower_one \|/,
  );
});

test("spawn times a real program's starts and says where it resolved", () => {
  const r = run("spawn", "--runs", "3", "--json", "--program", `self=${process.execPath}`);
  assert.equal(r.status, 0, r.stderr);
  const doc = JSON.parse(r.stdout);
  const self = doc.results.find((x) => x.name === "self");
  assert.equal(self.path, process.execPath);
  assert.equal(self.runs, 3);
  assert.ok(self.median_ms > 0 && self.p10_ms <= self.median_ms && self.median_ms <= self.p90_ms);
  const missing = run(
    "spawn",
    "--runs",
    "1",
    "--program",
    `gone=${join(scratch, "no-such-program")}`,
  );
  assert.equal(missing.status, 0, missing.stderr);
  assert.match(missing.stdout, /\| gone \| --version \| not found \|/);
});

test("a missing log is refused naming it, and a usage error exits 2", () => {
  const empty = join(scratch, "empty");
  mkdirSync(empty, { recursive: true });
  const missing = run("fold", `windows=${empty}`);
  assert.equal(missing.status, 1);
  assert.match(missing.stderr, /job\.log does not exist\nnext: /);
  for (const args of [
    [],
    ["fold"],
    ["fold", "--top", "0", `w=${empty}`],
    ["spawn", "--runs", "x"],
    ["dance"],
  ]) {
    const r = run(...args);
    assert.equal(r.status, 2, `${args.join(" ")}: ${r.stderr}`);
    assert.match(r.stderr, /next: run 'node scripts\/windows-job-profile\.mjs/);
  }
  const baseline = run("fold", "--baseline", "nobody", `windows=${job("b1")}`);
  assert.equal(baseline.status, 2);
  assert.match(baseline.stderr, /--baseline nobody names no job/);
});
