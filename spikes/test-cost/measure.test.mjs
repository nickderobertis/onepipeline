// The harness run as its user runs it: `node measure.mjs …` as a subprocess, over a
// cache laid out exactly as its own reads leave one, so no step reaches GitHub.
// Run with `node --test spikes/test-cost/measure.test.mjs`.

import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { mkdirSync, mkdtempSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { describe, it } from "node:test";
import { fileURLToPath } from "node:url";

const HARNESS = join(dirname(fileURLToPath(import.meta.url)), "measure.mjs");
const RUN = "4242";
const HEAD = "a".repeat(40);
const BASE = "b".repeat(40);
const BLOB = "c".repeat(40);

function measure(args) {
  const ran = spawnSync(process.execPath, [HARNESS, ...args], { encoding: "utf8" });
  return { status: ran.status, stdout: ran.stdout, stderr: ran.stderr };
}

function write(path, content) {
  mkdirSync(dirname(path), { recursive: true });
  writeFileSync(path, typeof content === "string" ? content : JSON.stringify(content));
}

function record(test, wallS, programs) {
  return {
    schema: 2,
    binary: "e2e",
    test,
    run_id: "nextest-run",
    attempt: 1,
    os: "linux",
    arch: "x86_64",
    started_at: 0,
    wall_s: wallS,
    ended: "test-thread",
    categories: { wait: { count: 0, s: 0 }, spawn: { count: 1, s: wallS } },
    unattributed_s: 0,
    programs,
  };
}

/// One completed attempt whose `test-linux` job ran in two matrix variants, the
/// second one minute faster in both its job and its e2e tier.
function cache() {
  const root = mkdtempSync(join(tmpdir(), "measure-test-"));
  const run = join(root, RUN);
  const attempt = join(run, "attempt-1");
  write(join(run, "run.json"), { run_attempt: 1 });
  write(join(attempt, "attempt.json"), { status: "completed", head_sha: HEAD, run_started_at: "2026-11-13T10:00:00Z" });
  write(join(attempt, `ci-yml-blob-${HEAD}`), `${BLOB}\n`);
  const jobs = [];
  const artifacts = [];
  for (const [index, variant, e2eS, jobEnd] of [
    [1, "baseline", 600, "2026-11-13T10:30:00Z"],
    [2, "idle", 540, "2026-11-13T10:29:00Z"],
  ]) {
    jobs.push({
      id: index,
      name: `test-linux (${variant})`,
      conclusion: "success",
      created_at: "2026-11-13T10:00:00Z",
      started_at: "2026-11-13T10:00:00Z",
      completed_at: jobEnd,
    });
    write(
      join(attempt, "logs", `${index}_test-linux (${variant}).txt`),
      [
        "2026-11-13T10:00:01.0000000Z HEAD is now at 1234567 Merge " + `${HEAD} into ${BASE}`,
        "2026-11-13T10:00:02.0000000Z ##[group]Run Swatinem/rust-cache@v2.9.2",
        '2026-11-13T10:00:30.0000000Z Restored from cache key "v0-rust" full match: true.',
        "2026-11-13T10:00:31.0000000Z ##[group]Run just ci-test-linux",
        "2026-11-13T10:00:32.0000000Z ci-job: test-linux runs 2 task(s): onepipeline:test onepipeline-e2e:test",
        "2026-11-13T10:01:00.0000000Z github-cache: CACHE_READ_ONLY is set, so this job serves the READ-ONLY Actions-cache backend",
        "2026-11-13T10:02:00.0000000Z > nx run onepipeline-e2e:test",
        `2026-11-13T10:12:00.0000000Z      Summary [ ${e2eS}.000s] 2 tests run: 2 passed, 0 skipped`,
        "2026-11-13T10:13:00.0000000Z lines percent: 96.5000",
        "2026-11-13T10:14:00.0000000Z ##[group]Run just rerun-failed",
        "",
      ].join("\n"),
    );
    const name = `test-profile-test-linux-${variant}`;
    artifacts.push({ id: 100 + index, name, created_at: "2026-11-13T10:20:00Z", expired: false });
    const records = join(attempt, "artifacts", name);
    write(join(records, ".done"), "");
    write(
      join(records, "e2e", "views.one.json"),
      record("views::one", e2eS / 2, { onepipeline: { count: 2, s: 4 }, "onepipeline:start": { count: 1, s: 3 }, "onepipeline:status": { count: 1, s: 1 } }),
    );
    write(join(records, "e2e", "views.two.json"), record("views::two", e2eS / 2, { onepipeline: { count: 1, s: 1 }, "onepipeline:start": { count: 1, s: 1 } }));
  }
  write(join(attempt, "jobs.json"), { jobs });
  write(join(attempt, "artifacts.json"), { artifacts });
  return root;
}

describe("measure.mjs compare", () => {
  it("prints each variant's minutes, coverage, verbs, Nx selection and cache evidence, and the difference between them", () => {
    const root = cache();
    const list = join(root, "set.txt");
    write(list, "# the pair\nviews::one\nviews::two\n");
    const { status, stdout, stderr } = measure([
      "compare",
      "--cache",
      root,
      "--side",
      `baseline=${RUN}#1@baseline`,
      "--side",
      `idle=${RUN}#1@idle`,
      "--tests",
      `pair=${list}`,
    ]);
    assert.equal(status, 0, stderr);
    assert.match(stdout, /\| baseline \| 4242#1@baseline \| 1 \| test-linux \| success \| 1234567 \| aaaaaaaaaaaa \| cccccccccccc \| 2 \|/);
    assert.match(stdout, /\| baseline \| test-linux \| 0\.0 \| 30\.0 \| 10\.0 \| 2\/2\/0 \| 96\.5000 \| 30\.0 \| 10\.0 \|/);
    assert.match(stdout, /\| idle \| test-linux \| -1\.0 \| -1\.0 \|/);
    assert.match(stdout, /test-linux: .* identical test-name sets/);
    assert.match(stdout, /\| idle \| 4242#1@idle \| test-linux \| 2 \| 540\.0 \|/);
    assert.match(stdout, /\| start \| 2 \| 4\.0 \| 2000 \| 80\.0% \|/);
    assert.match(stdout, /test-linux, 2 tasks, selected by 2 run\(s\) .*: onepipeline:test onepipeline-e2e:test/);
    assert.match(stdout, /rust-cache: hit \(full\), 29\.0/);
    assert.match(stdout, /READ-ONLY Actions-cache backend/);
  });

  it("refuses a run it cannot parse, naming the shape it wants", () => {
    const { status, stderr } = measure(["compare", "--side", "baseline=4242#0"]);
    assert.equal(status, 2);
    assert.match(stderr, /not RUN\[#ATTEMPT\]\[@VARIANT\]/);
  });

  it("refuses a cached record that is not a profile record, naming the file", () => {
    const root = cache();
    write(join(root, RUN, "attempt-1", "artifacts", "test-profile-test-linux-idle", "e2e", "views.bad.json"), { schema: 2, test: "views::bad" });
    const { status, stderr } = measure(["compare", "--cache", root, "--side", `idle=${RUN}#1@idle`]);
    assert.equal(status, 2);
    assert.match(stderr, /views\.bad\.json is not a schema-2 profile record/);
  });

  it("refuses a test list naming a test twice", () => {
    const root = cache();
    const list = join(root, "twice.txt");
    write(list, "views::one\nviews::one\n");
    const { status, stderr } = measure(["compare", "--cache", root, "--side", `idle=${RUN}#1@idle`, "--tests", `twice=${list}`]);
    assert.equal(status, 2);
    assert.match(stderr, /names a test twice/);
  });
});

describe("measure.mjs verbs", () => {
  it("prints a record directory's engine runs per verb, as Markdown or JSON", () => {
    const root = cache();
    const dir = join(root, RUN, "attempt-1", "artifacts", "test-profile-test-linux-baseline", "e2e");
    const markdown = measure(["verbs", dir]);
    assert.equal(markdown.status, 0, markdown.stderr);
    assert.match(markdown.stdout, /engine invocations: 3, 5\.0 s/);
    const json = measure(["verbs", "--json", dir]);
    assert.equal(json.status, 0, json.stderr);
    const [summary] = JSON.parse(json.stdout);
    assert.deepEqual(summary.verbs.start, { count: 2, s: 4 });
    assert.equal(summary.records, 2);
  });

  it("refuses a directory that does not exist rather than reporting nothing", () => {
    const { status, stderr } = measure(["verbs", join(tmpdir(), "measure-test-no-such-dir")]);
    assert.equal(status, 2);
    assert.match(stderr, /is not a directory of profile records/);
  });
});
