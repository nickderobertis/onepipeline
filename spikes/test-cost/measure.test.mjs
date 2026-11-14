// The harness run as its user runs it: `node measure.mjs …` as a subprocess, with a
// `gh` on PATH answering from fixtures exactly the endpoints the harness asks, a
// real git repository holding the head commit, and records cloned from the
// repository's own golden profile record.
// Run with `node --test spikes/test-cost/measure.test.mjs`.

import assert from "node:assert/strict";
import { execFileSync, spawnSync } from "node:child_process";
import { chmodSync, existsSync, mkdirSync, mkdtempSync, readFileSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { delimiter, dirname, join, resolve } from "node:path";
import { describe, it } from "node:test";
import { fileURLToPath } from "node:url";

const HERE = dirname(fileURLToPath(import.meta.url));
const HARNESS = join(HERE, "measure.mjs");
const GOLDEN = JSON.parse(readFileSync(resolve(HERE, "../../tests/golden/test-profile-record-v2.json"), "utf8"));
const RUN = "4242";
const REPO = "repos/nickderobertis/onepipeline";

function write(path, content) {
  mkdirSync(dirname(path), { recursive: true });
  writeFileSync(path, typeof content === "string" || Buffer.isBuffer(content) ? content : JSON.stringify(content));
}

/// A record the writer could have written: the golden one with this test's values.
function record(test, wallS, programs) {
  const copy = structuredClone(GOLDEN);
  copy.test = test;
  copy.run_id = "nextest-run";
  copy.wall_s = wallS;
  copy.programs = programs;
  return copy;
}

function zipOf(files) {
  const dir = mkdtempSync(join(tmpdir(), "measure-zip-"));
  for (const [name, content] of Object.entries(files)) write(join(dir, name), content);
  execFileSync("zip", ["-q", "-r", "out.zip", ...Object.keys(files)], { cwd: dir });
  return readFileSync(join(dir, "out.zip"));
}

function log(variant, e2eLine, { cacheLine = 'Restored from cache key "v0-rust" full match: true.', coverage = "96.5000" } = {}) {
  return [
    `2026-11-13T10:00:01.0000000Z HEAD is now at 1234567 Merge ${"a".repeat(40)} into ${"b".repeat(40)}`,
    "2026-11-13T10:00:02.0000000Z ##[group]Run Swatinem/rust-cache@v2.9.2",
    `2026-11-13T10:00:30.0000000Z ${cacheLine}`,
    "2026-11-13T10:00:31.0000000Z ##[group]Run actions/cache/restore@v4",
    "2026-11-13T10:00:32.0000000Z Cache not found for input keys: bootstrap-tools",
    "2026-11-13T10:00:33.0000000Z ##[group]Run just ci-test-linux",
    "2026-11-13T10:00:34.0000000Z ci-job: test-linux runs 2 task(s): onepipeline:test onepipeline-e2e:test",
    "2026-11-13T10:01:00.0000000Z github-cache: CACHE_READ_ONLY is set, so this job serves the READ-ONLY Actions-cache backend",
    "2026-11-13T10:01:01.0000000Z   save-if: false",
    "2026-11-13T10:02:00.0000000Z > nx run onepipeline-e2e:test",
    `2026-11-13T10:12:00.0000000Z ${e2eLine}`,
    `2026-11-13T10:13:00.0000000Z lines percent: ${coverage}`,
    `2026-11-13T10:14:00.0000000Z ##[group]Run variant ${variant} ends`,
    "",
  ].join("\r\n");
}

/// A world: a git repository whose one commit is the run's head, a `gh` answering
/// two attempts of one run in two variants, and an empty directory to keep reads in.
function world({ secondAttempt = "completed", workflowName = "test-profile-${{ github.job }}-${{ matrix.variant }}", logs1, record2 } = {}) {
  const root = mkdtempSync(join(tmpdir(), "measure-test-"));
  const repo = join(root, "repo");
  write(join(repo, ".github/workflows/ci.yml"), `jobs:\n  test-linux:\n    steps:\n      - uses: actions/upload-artifact@v4\n        with:\n          name: ${workflowName}\n  budgets:\n    steps: []\n`);
  const git = (...args) => execFileSync("git", ["-c", "user.name=t", "-c", "user.email=t@example.invalid", ...args], { cwd: repo, encoding: "utf8" }).trim();
  git("init", "-q");
  git("add", ".");
  git("commit", "-q", "-m", "head");
  const head = git("rev-parse", "HEAD");
  const blob = git("rev-parse", "HEAD:.github/workflows/ci.yml");

  const answers = join(root, "answers");
  const answer = (path, body) => write(join(answers, encodeURIComponent(path)), body);
  answer(`${REPO}/actions/runs/${RUN}`, { run_attempt: 2 });
  const attempts = { 1: "completed", 2: secondAttempt };
  for (const [attempt, status] of Object.entries(attempts)) {
    const started = `2026-11-13T1${attempt}:00:00Z`;
    answer(`${REPO}/actions/runs/${RUN}/attempts/${attempt}`, { status, head_sha: head, run_started_at: started });
    const jobs = [];
    const logFiles = {};
    let index = 0;
    for (const [variant, e2eS, end] of [
      ["baseline", 600, "10:30"],
      ["idle", 540, "10:29"],
    ]) {
      index++;
      jobs.push({
        id: index,
        name: `test-linux (${variant})`,
        conclusion: "success",
        created_at: `2026-11-13T1${attempt}:00:00Z`,
        started_at: `2026-11-13T1${attempt}:00:00Z`,
        completed_at: `2026-11-13T1${attempt}:${end.slice(3)}:00Z`,
      });
      const wanted = logs1 && variant in logs1 ? logs1[variant] : log(variant, `     Summary [ ${e2eS}.000s] 2 tests run: 2 passed, 0 skipped`);
      if (wanted !== null) logFiles[`${index}_test-linux (${variant}).txt`] = wanted;
    }
    jobs.push({ id: 9, name: "budgets", conclusion: "skipped", created_at: started });
    answer(`${REPO}/actions/runs/${RUN}/attempts/${attempt}/jobs?per_page=100`, { jobs });
    answer(`${REPO}/actions/runs/${RUN}/attempts/${attempt}/logs`, zipOf(logFiles));
  }
  const artifacts = [];
  for (const [id, attempt, variant, extra] of [
    [101, 1, "baseline", false],
    [102, 1, "idle", true],
    [201, 2, "baseline", false],
    [202, 2, "idle", true],
  ]) {
    artifacts.push({ id, name: `test-profile-test-linux-${variant}`, created_at: `2026-11-13T1${attempt}:20:00Z`, expired: false });
    const files = {
      "e2e/views.one.json": record("views::one", 300, { onepipeline: { count: 2, s: 4 }, "onepipeline:start": { count: 1, s: 3 }, "onepipeline:status": { count: 1, s: 1 } }),
      "e2e/views.two.json": record2 ?? record("views::two", 200, { onepipeline: { count: 1, s: 1 }, "onepipeline:start": { count: 1, s: 1 } }),
      "e2e/stale.json": { ...record("views::stale", 999, {}), run_id: "an-earlier-run" },
    };
    if (extra) files["e2e/views.three.json"] = record("views::three", 50, {});
    answer(`${REPO}/actions/artifacts/${id}/zip`, zipOf(files));
  }
  answer(`${REPO}/actions/runs/${RUN}/artifacts?per_page=100`, { artifacts });

  const bin = join(root, "bin");
  const calls = join(root, "gh-calls");
  write(
    join(bin, "gh"),
    `#!/usr/bin/env node
const fs = require("node:fs");
const path = require("node:path");
const [, , verb, endpoint] = process.argv;
fs.appendFileSync(${JSON.stringify(calls)}, endpoint + "\\n");
const file = path.join(${JSON.stringify(answers)}, encodeURIComponent(endpoint));
if (verb !== "api" || !fs.existsSync(file)) { process.stderr.write("gh: no answer for " + endpoint + "\\n"); process.exit(1); }
process.stdout.write(fs.readFileSync(file));
`,
  );
  chmodSync(join(bin, "gh"), 0o755);
  return { root, repo, head, blob, cache: join(root, "cache"), calls, answers, env: { ...process.env, PATH: `${bin}${delimiter}${process.env.PATH}` } };
}

function measure(w, args) {
  const ran = spawnSync(process.execPath, [HARNESS, ...args], { cwd: w.repo, env: w.env, encoding: "utf8" });
  return { status: ran.status, stdout: ran.stdout, stderr: ran.stderr };
}

const calls = (w) => (existsSync(w.calls) ? readFileSync(w.calls, "utf8").trim().split("\n") : []);
const SIDES = ["--side", `baseline=${RUN}#1@baseline,${RUN}#2@baseline`, "--side", `idle=${RUN}#1@idle,${RUN}#2@idle`];

describe("measure.mjs over a run it reads through gh", () => {
  it("reports each variant's minutes, tiers, coverage, provenance, verbs, Nx selection and caches, and the difference between them", () => {
    const w = world();
    const { status, stdout, stderr } = measure(w, ["--cache", w.cache, ...SIDES]);
    assert.equal(status, 0, stderr);
    assert.match(stdout, new RegExp(`\\| baseline \\| ${RUN}#1@baseline \\| test-linux \\| success \\| 1234567 \\| ${w.head.slice(0, 12)} \\| ${w.blob.slice(0, 12)} \\| 2 \\|`));
    assert.doesNotMatch(stdout, /\| budgets \|/, "only test-* jobs are platform jobs");
    assert.match(stdout, /\| baseline \| test-linux \| 0\.0, 0\.0 \| 30\.0, 30\.0 \| 10\.0, 10\.0 \| 2\/2\/0, 2\/2\/0 \| 96\.5000, 96\.5000 \| 30\.0 \| 10\.0 \|/);
    assert.match(stdout, /\| idle \| test-linux \| -1\.0 \| -1\.0 \| baseline 30\.0–30\.0; idle 29\.0–29\.0 \|/);
    assert.match(stdout, new RegExp(`\\| idle \\| ${RUN}#2@idle \\| test-linux \\| 3 \\| 1, 50\\.0 \\| 0, 0\\.0 \\|`));
    assert.match(stdout, /engine invocations: 3, 5\.0 s of 500\.0 e2e test-s/, "the earlier run's stray record is left out");
    assert.match(stdout, /\| start \| 2 \| 4\.0 \| 2000 \| 80\.0% \|/);
    assert.match(stdout, /test-linux, 2 tasks, selected by 4 run\(s\) .*: onepipeline:test onepipeline-e2e:test/);
    assert.match(stdout, /rust-cache: hit \(full\), 29\.0 s; actions\/cache\/restore: miss, 2\.0 s/);
    assert.match(stdout, /READ-ONLY Actions-cache backend \/ save-if: false/);
  });

  it("asks gh once for each answer, the artifact listing once per attempt, and nothing on the next invocation", () => {
    const w = world();
    assert.equal(measure(w, ["--cache", w.cache, ...SIDES]).status, 0);
    const first = calls(w);
    const listing = `${REPO}/actions/runs/${RUN}/artifacts?per_page=100`;
    assert.equal(first.filter((call) => call === listing).length, 2, "each attempt reads the listing once it has ended");
    const rest = first.filter((call) => call !== listing);
    assert.equal(new Set(rest).size, rest.length, `an answer was asked twice: ${first.join(", ")}`);
    assert.ok(first.includes(`${REPO}/actions/artifacts/202/zip`), "the second attempt's own artifact is read");
    assert.ok(!first.includes(`${REPO}/actions/artifacts/101/zip`) || first.includes(`${REPO}/actions/artifacts/201/zip`));
    assert.equal(measure(w, ["--cache", w.cache, ...SIDES]).status, 0);
    assert.deepEqual(calls(w), first);
  });

  it("reads the artifact name the head's workflow uploads, and the run's latest attempt when none is named", () => {
    const w = world({ workflowName: "test-profile-${{ github.job }}-${{ matrix.variant }}" });
    const { status, stdout, stderr } = measure(w, ["--cache", w.cache, "--side", `idle=${RUN}@idle`]);
    assert.equal(status, 0, stderr);
    assert.match(stdout, new RegExp(`\\| idle \\| ${RUN}#2@idle \\| test-linux \\| success`));
    assert.equal(readFileSync(join(w.cache, RUN, "attempt-2", "profile-artifact-test-linux"), "utf8").trim(), "test-profile-${{ github.job }}-${{ matrix.variant }}");
  });

  it("reports an attempt still running, and a job whose log is missing, without measuring them", () => {
    const w = world({ secondAttempt: "in_progress", logs1: { idle: null } });
    const { status, stdout, stderr } = measure(w, ["--cache", w.cache, ...SIDES]);
    assert.equal(status, 0, stderr);
    assert.match(stdout, new RegExp(`\\| baseline \\| ${RUN}#2@baseline \\| – \\| in_progress \\|`));
    assert.match(stdout, new RegExp(`\\| idle \\| ${RUN}#1@idle \\| test-linux \\| success \\(no log\\) \\|`));
  });

  it("reads a failed tier's counts and a partial cache restore", () => {
    const w = world({
      logs1: { baseline: log("baseline", "     Summary [ 600.000s] 2 tests run: 1 passed, 1 failed, 0 skipped", { cacheLine: 'Restored from cache key "v0" full match: false.' }) },
    });
    const { status, stdout, stderr } = measure(w, ["--cache", w.cache, "--side", `baseline=${RUN}#1@baseline`]);
    assert.equal(status, 0, stderr);
    assert.match(stdout, /\| 2\/1\/1 \|/);
    assert.match(stdout, /rust-cache: hit \(partial key\)/);
  });
});

describe("measure.mjs refusals", () => {
  const refused = (args, pattern, options) => {
    const w = world(options);
    const { status, stderr } = measure(w, args(w));
    assert.equal(status, 2, stderr);
    assert.match(stderr, pattern);
  };

  it("refuses arguments it cannot read", () => {
    refused(() => ["--side", `baseline=${RUN}#0`, "--cache", "x"], /not RUN\[#ATTEMPT\]\[@VARIANT\]/);
    refused(() => ["--cache"], /--cache needs a value/);
    refused(() => ["--cache", "x", "--side", "a=1", "--side", "a=2"], /side 'a' is named twice/);
    refused(() => ["--cache", "x", "--verbs", "y"], /unknown argument --verbs/);
    refused(() => ["--side", "a=1"], /--cache DIR/);
    refused(() => ["--cache", "x"], /at least one --side/);
  });

  it("refuses a record that does not have the golden record's shape, naming the file", () => {
    refused((w) => ["--cache", w.cache, "--side", `baseline=${RUN}#1@baseline`], /views\.two\.json does not have the golden profile record's shape/, {
      record2: { schema: 2, test: "views::two" },
    });
  });

  it("refuses a summary whose counts do not add up", () => {
    refused((w) => ["--cache", w.cache, "--side", `baseline=${RUN}#1@baseline`], /the e2e tier's summary reads/, {
      logs1: { baseline: log("baseline", "     Summary [ 600.000s] 1 test run: 2 passed, 0 skipped") },
    });
  });

  it("refuses a coverage figure over 100", () => {
    refused((w) => ["--cache", w.cache, "--side", `baseline=${RUN}#1@baseline`], /the line coverage reads/, {
      logs1: { baseline: log("baseline", "     Summary [ 600.000s] 2 tests run: 2 passed, 0 skipped", { coverage: "196.5" }) },
    });
  });

  it("refuses a kept answer that is not JSON, saying how to read it again", () => {
    refused(
      (w) => {
        write(join(w.cache, RUN, "run.json"), "not json");
        return ["--cache", w.cache, "--side", `baseline=${RUN}#1@baseline`];
      },
      /run\.json is not JSON; delete it to read it again/,
    );
  });

  it("fails with 1 and gh's own words when gh cannot answer", () => {
    const w = world();
    const { status, stderr } = measure(w, ["--cache", w.cache, "--side", "baseline=999#1@baseline"]);
    assert.equal(status, 1);
    assert.match(stderr, /no answer for .*runs\/999/);
  });
});
