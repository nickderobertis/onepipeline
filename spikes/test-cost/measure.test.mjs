// The harness run as its user runs it: `node measure.mjs …` as a subprocess, with a
// `gh` on PATH answering from fixtures exactly the endpoints the harness asks, a
// real git repository holding the head commit, records cloned from the
// repository's own golden profile record, and the task and coverage lines made by
// the statements in `scripts/ci-job.sh` and `scripts/coverage-merge.mjs` that
// print them.
// Run with `node --test spikes/test-cost/measure.test.mjs`.

import assert from "node:assert/strict";
import { execFileSync, spawnSync } from "node:child_process";
import { chmodSync, existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, symlinkSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { delimiter, dirname, join, resolve } from "node:path";
import { describe, it } from "node:test";
import { fileURLToPath } from "node:url";

const HERE = dirname(fileURLToPath(import.meta.url));
const ROOT = resolve(HERE, "../..");
const HARNESS = join(HERE, "measure.mjs");
const GOLDEN = JSON.parse(readFileSync(join(ROOT, "tests/golden/test-profile-record-v2.json"), "utf8"));
const RUN = "4242";
const REPO = "repos/nickderobertis/onepipeline";

/// `scripts/ci-job.sh`'s own lines that count the tasks and print them, run by bash.
function tasksLine(job, tasks) {
  const lines = readFileSync(join(ROOT, "scripts/ci-job.sh"), "utf8").split("\n");
  const echo = lines.findIndex((line) => /^echo "ci-job: \$job runs/.test(line));
  const counted = lines.findIndex((line) => /^count=0$/.test(line));
  assert.ok(echo > counted && counted >= 0, "scripts/ci-job.sh no longer counts and prints the tasks line this harness reads");
  const script = lines.slice(counted, echo + 1).join("\n");
  return execFileSync("bash", ["-c", `job="$1"; tasks="$2"\n${script}`, "bash", job, tasks], { encoding: "utf8" }).trim();
}

/// The key `designs/measurement.diff`'s profile writer counts an engine verb's runs
/// under: the engine's name and the verb, joined by its own `format!`.
function verbKey(verb) {
  const diff = readFileSync(join(HERE, "designs/measurement.diff"), "utf8");
  const tags = /^\+.*if program == "onepipeline" \{$/m.test(diff) && /^\+\s+return \(category, format!\("\{program\}\\u\{1f\}\{verb\}"\)\);$/m.test(diff);
  const joined = /^\+.*Some\(format!\("\{base\}(.*?)\{verb\}"\)\)/m.exec(diff);
  assert.ok(tags && joined, "designs/measurement.diff no longer counts an engine verb's runs under the key this harness reads");
  return `onepipeline${joined[1]}${verb}`;
}

/// `scripts/coverage-merge.mjs`'s own print of the line percentage, filled.
function coverageLine(percent) {
  const print = /console\.log\(`(lines percent: \$\{percent\})`\);/.exec(readFileSync(join(ROOT, "scripts/coverage-merge.mjs"), "utf8"));
  assert.ok(print, "scripts/coverage-merge.mjs no longer prints the coverage line this harness reads");
  return print[1].replace("${percent}", percent);
}

function write(path, content) {
  mkdirSync(dirname(path), { recursive: true });
  writeFileSync(path, typeof content === "string" || Buffer.isBuffer(content) ? content : JSON.stringify(content));
}

/// A record the writer could have written: the golden one with this test's values.
function record(test, wallS, programs, change = {}) {
  return { ...structuredClone(GOLDEN), test, run_id: "nextest-run", wall_s: wallS, programs, ...change };
}

/// An archive of these files; a value `{ link }` is stored as a link to that path.
function zipOf(files) {
  const dir = mkdtempSync(join(tmpdir(), "measure-zip-"));
  for (const [name, content] of Object.entries(files)) {
    if (content !== null && typeof content === "object" && typeof content.link === "string") {
      mkdirSync(dirname(join(dir, name)), { recursive: true });
      symlinkSync(content.link, join(dir, name));
    } else write(join(dir, name), content);
  }
  execFileSync("zip", ["-q", "-r", "--symlinks", "out.zip", ...Object.keys(files)], { cwd: dir });
  return readFileSync(join(dir, "out.zip"));
}

// llmlint: ignore-block[contracts_have_one_source_or_a_drift_gate] these fixtures stand in for GitHub's REST answers and for the runner's, actions/checkout's, the cache actions' and nextest's log lines, none of which this repository produces or any test here can run; they are written in the shapes the two real attempts this spike measured have, the lines this repository does produce are made by their own producers above, and the records are cloned from the repository's golden record.
/// A job log as the runner writes it: stamped lines, colour codes, CRLF.
function log({
  summary = "     Summary [ 600.000s] 2 tests run: 2 passed, 0 skipped",
  tier = "> nx run onepipeline-e2e:test",
  cache = 'Restored from cache key "v0-rust" full match: true.',
  coverage = coverageLine("96.5000"),
  tasks = tasksLine("test-linux", "onepipeline:test onepipeline-e2e:test"),
  stamp = "2026-11-13T10:00:36.0000000Z",
  ruststart = "2026-11-13T10:00:02.0000000Z",
  checkout = `HEAD is now at 1234567 Merge ${"a".repeat(40)} into ${"b".repeat(40)}`,
  extra = [],
} = {}) {
  return [
    "\uFEFF2026-11-13T10:00:00.5000000Z the runner's first line, after its byte-order mark",
    "an unstamped continuation of a step's multi-line input",
    ...(checkout ? [`2026-11-13T10:00:01.0000000Z ${checkout}`] : []),
    `${ruststart} \x1b[36m##[group]Run Swatinem/rust-cache@v2.9.2\x1b[0m`,
    `2026-11-13T10:00:30.0000000Z ${cache}`,
    "2026-11-13T10:00:31.0000000Z ##[group]Run actions/cache/restore@v4",
    "2026-11-13T10:00:32.0000000Z Cache not found for input keys: bootstrap-tools",
    "2026-11-13T10:00:33.0000000Z ##[group]Run astral-sh/setup-uv@v6",
    "2026-11-13T10:00:34.0000000Z uv cache restored from GitHub Actions cache with key: setup-uv-1",
    "2026-11-13T10:00:35.0000000Z ##[group]Run just ci-test-linux",
    ...(tasks ? [`${stamp} ${tasks}`] : []),
    "2026-11-13T10:01:00.0000000Z github-cache: CACHE_READ_ONLY is set, so this job serves the READ-ONLY Actions-cache backend",
    "2026-11-13T10:01:01.0000000Z   save-if: false",
    ...(tier ? [`2026-11-13T10:02:00.0000000Z ${tier}`] : []),
    ...(tier && summary ? [`2026-11-13T10:12:00.0000000Z ${summary}`] : []),
    ...(coverage ? [`2026-11-13T10:13:00.0000000Z ${coverage}`] : []),
    ...extra.map((line) => `2026-11-13T10:13:30.0000000Z ${line}`),
    "2026-11-13T10:14:00.0000000Z ##[group]Run just rerun-failed",
    "",
  ].join("\r\n");
}

const VARIANTS = [
  ["baseline", "30"],
  ["idle", "29"],
];
const named = (job, variant, change) => (change.plain ? job : `${job} (${variant})`);

/// A world: a git repository whose one commit is the run's head, a `gh` answering
/// two attempts of one run in two variants, and an empty directory to keep reads in.
/// `change` rewrites the answers before they are written, keyed by what they are.
function world(change = {}) {
  const root = mkdtempSync(join(tmpdir(), "measure-test-"));
  const repo = join(root, "repo");
  const upload = change.workflowName ?? (change.plain ? "test-profile-${{ github.job }}" : "test-profile-${{ github.job }}-${{ matrix.variant }}");
  const platforms = change.macos ? ["test-linux", "test-macos"] : ["test-linux"];
  write(
    join(repo, ".github/workflows/ci.yml"),
    `jobs:\n${platforms.map((job) => `  ${job}:\n    steps:\n      - uses: actions/upload-artifact@v4\n        with:\n          name: ${upload}\n`).join("")}  budgets:\n    steps: []\n`,
  );
  const git = (...args) => execFileSync("git", ["-c", "user.name=t", "-c", "user.email=t@example.invalid", ...args], { cwd: repo, encoding: "utf8" }).trim();
  git("init", "-q");
  git("add", ".");
  git("commit", "-q", "-m", "head");
  const head = change.head ?? git("rev-parse", "HEAD");
  const blob = git("rev-parse", "HEAD:.github/workflows/ci.yml");

  const answers = join(root, "answers");
  const answer = (path, body) => write(join(answers, encodeURIComponent(path)), body);
  answer(`${REPO}/actions/runs/${RUN}`, change.run ?? { run_attempt: 2 });
  const artifacts = [];
  for (const attempt of [1, 2]) {
    const started = `2026-11-13T1${attempt}:00:00Z`;
    const status = attempt === 2 ? (change.secondAttempt ?? "completed") : "completed";
    answer(`${REPO}/actions/runs/${RUN}/attempts/${attempt}`, change.attempt?.(attempt) ?? { status, head_sha: head, run_started_at: started });
    let jobs = [];
    const logs = {};
    (change.plain ? VARIANTS.slice(0, 1) : VARIANTS).forEach(([variant, end], index) => {
      jobs.push({
        id: index + 1,
        name: named("test-linux", variant, change),
        conclusion: "success",
        created_at: `2026-11-13T1${attempt}:00:00Z`,
        started_at: `2026-11-13T1${attempt}:0${attempt}:00Z`,
        completed_at: `2026-11-13T1${attempt}:${end}:00Z`,
      });
      const key = `${attempt} ${variant}`;
      const wanted = change.logs && key in change.logs ? change.logs[key] : log({ summary: `     Summary [ ${variant === "idle" ? 540 : 600}.000s] 2 tests run: 2 passed, 0 skipped` });
      if (wanted !== null) logs[`${index + 1}_${named("test-linux", variant, change)}.txt`] = wanted;
      const id = attempt * 100 + index + 1;
      if (change.duplicate && id === 101) {
        artifacts.push({ id: 151, name: "test-profile-test-linux-baseline", created_at: "2026-11-13T11:25:00Z", expired: false });
        answer(`${REPO}/actions/artifacts/151/zip`, zipOf({ "e2e/views.late.json": record("views::late", 5, {}) }));
      }
      artifacts.push({ id, name: change.plain ? "test-profile-test-linux" : `test-profile-test-linux-${variant}`, created_at: `2026-11-13T1${attempt}:20:00Z`, expired: change.expired?.includes(id) ?? false });
      const files = {
        "e2e/views.one.json": record("views::one", 300, {
          onepipeline: { count: 2, s: 4 },
          [verbKey("start")]: { count: 1, s: 3 },
          [verbKey("status")]: { count: 1, s: 1 },
        }),
        "e2e/views.two.json": record("views::two", 200, { onepipeline: { count: 1, s: 1 }, [verbKey("start")]: { count: 1, s: 1 } }),
        "e2e/stale.json": record("views::stale", 999, {}, { run_id: "an-earlier-run" }),
        ...(variant === "idle" ? { "e2e/views.three.json": record("views::three", 50, {}) } : { "e2e/views.four.json": record("views::four", 70, {}) }),
        ...(change.records?.[id] ?? {}),
      };
      answer(`${REPO}/actions/artifacts/${id}/zip`, zipOf(files));
    });
    if (change.macos) {
      jobs.push({
        id: 3,
        name: "test-macos (baseline)",
        conclusion: "success",
        created_at: `2026-11-13T1${attempt}:00:00Z`,
        started_at: `2026-11-13T1${attempt}:00:00Z`,
        completed_at: `2026-11-13T1${attempt}:40:00Z`,
      });
      logs["3_test-macos (baseline).txt"] = log({ summary: "     Summary [ 1200.000s] 2 tests run: 2 passed, 0 skipped", tasks: tasksLine("test-macos", "onepipeline-e2e:test-quick"), coverage: null });
      const id = attempt * 100 + 3;
      artifacts.push({ id, name: "test-profile-test-macos-baseline", created_at: `2026-11-13T1${attempt}:20:00Z`, expired: false });
      answer(`${REPO}/actions/artifacts/${id}/zip`, zipOf({ "e2e/views.one.json": record("views::one", 900, {}) }));
    }
    jobs.push({ id: 9, name: "budgets", conclusion: "skipped", created_at: started });
    jobs = change.jobs?.(jobs, attempt) ?? jobs;
    answer(`${REPO}/actions/runs/${RUN}/attempts/${attempt}/jobs?per_page=100`, change.jobsAnswer?.(attempt) ?? { total_count: jobs.length, jobs });
    answer(`${REPO}/actions/runs/${RUN}/attempts/${attempt}/logs`, zipOf(logs));
  }
  answer(`${REPO}/actions/runs/${RUN}/artifacts?per_page=100`, change.artifactsAnswer ?? { total_count: artifacts.length, artifacts });

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
  return { root, repo, head, blob, answer, cache: join(root, "cache"), calls, env: { ...process.env, PATH: `${bin}${delimiter}${process.env.PATH}` } };
}

// llmlint: ignore-end[contracts_have_one_source_or_a_drift_gate]

function measure(w, args) {
  const ran = spawnSync(process.execPath, [HARNESS, ...args], { cwd: w.repo, env: w.env, encoding: "utf8" });
  return { status: ran.status, stdout: ran.stdout, stderr: ran.stderr };
}

const calls = (w) => (existsSync(w.calls) ? readFileSync(w.calls, "utf8").trim().split("\n") : []);
const SIDES = ["--side", `baseline=${RUN}#1@baseline,${RUN}#2@baseline`, "--side", `idle=${RUN}#1@idle,${RUN}#2@idle`];
const ONE = (variant = "baseline", attempt = 1) => ["--side", `${variant}=${RUN}#${attempt}@${variant}`];

function reported(change, args = SIDES) {
  const w = world(change);
  const ran = measure(w, ["--cache", w.cache, ...args]);
  assert.equal(ran.status, 0, ran.stderr);
  return { ...ran, w };
}

function refused(change, args, pattern, status = 2) {
  const w = world(change);
  const ran = measure(w, ["--cache", w.cache, ...args]);
  assert.equal(ran.status, status, ran.stderr);
  assert.match(ran.stderr, pattern);
}

describe("measure.mjs over a run it reads through gh", () => {
  it("reports each variant's minutes, tiers, coverage, provenance, verbs, Nx selection and caches, and the difference between them", () => {
    const { stdout, w } = reported({});
    assert.match(stdout, new RegExp(`\\| baseline \\| ${RUN}#1@baseline \\| test-linux \\| success \\| 1234567 \\| ${w.head.slice(0, 12)} \\| ${w.blob.slice(0, 12)} \\| 2 \\|`));
    assert.doesNotMatch(stdout, /\| budgets \|/, "only test-* jobs are platform jobs");
    assert.match(stdout, /\| baseline \| test-linux \| 1\.0, 2\.0 \| 29\.0, 28\.0 \| 10\.0, 10\.0 \| 2\/2\/0, 2\/2\/0 \| 96\.5000, 96\.5000 \| 28\.5 \| 10\.0 \|/);
    assert.match(stdout, /\| idle \| test-linux \| -1\.0 \| -1\.0 \| baseline 28\.0–29\.0; idle 27\.0–28\.0 \|/);
    assert.match(stdout, new RegExp(`\\| idle \\| ${RUN}#2@idle \\| test-linux \\| 3 \\| 1, 50\\.0 \\| 1, 70\\.0 \\|`));
    assert.match(stdout, /engine invocations: 3, 5\.0 s of 570\.0 e2e test-s/, "the earlier run's stray record is left out");
    assert.match(stdout, /\| start \| 2 \| 4\.0 \| 2000 \| 80\.0% \|/);
    assert.match(stdout, /test-linux, 2 tasks, selected by 4 run\(s\) .*: onepipeline:test onepipeline-e2e:test/);
    assert.match(stdout, /rust-cache: hit \(full\), 29\.0 s; actions\/cache\/restore: miss, 2\.0 s; setup-uv: hit, 2\.0 s/);
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
    assert.ok(rest.includes(`${REPO}/actions/artifacts/202/zip`), "the second attempt reads its own artifact");
    assert.equal(measure(w, ["--cache", w.cache, ...SIDES]).status, 0);
    assert.deepEqual(calls(w), first);
  });

  it("reads the artifact the head's workflow names, and the run's latest attempt when none is named", () => {
    const w = world();
    const ran = measure(w, ["--cache", w.cache, "--side", `idle=${RUN}@idle`]);
    assert.equal(ran.status, 0, ran.stderr);
    assert.match(ran.stdout, new RegExp(`\\| idle \\| ${RUN}#2@idle \\| test-linux \\| success`));
    assert.equal(readFileSync(join(w.cache, RUN, "attempt-2", "profile-artifact-test-linux"), "utf8").trim(), "test-profile-${{ github.job }}-${{ matrix.variant }}");
  });

  it("finds no records when the workflow names another artifact, or the one it names has expired", () => {
    assert.match(reported({ workflowName: "test-profile-elsewhere" }, ONE()).stdout, /\| baseline \| 4242#1@baseline \| test-linux \| – \| no records \| – \|/);
    const expired = reported({ expired: [201] }, ["--side", `baseline=${RUN}#1@baseline,${RUN}#2@baseline`]);
    assert.match(expired.stdout, /\| baseline \| 4242#2@baseline \| test-linux \| – \| no records \| – \|/);
  });

  it("answers from what it kept about the head once git can no longer reach it", () => {
    const w = world();
    const first = measure(w, ["--cache", w.cache, ...ONE()]);
    assert.equal(first.status, 0, first.stderr);
    const elsewhere = mkdtempSync(join(tmpdir(), "measure-no-repo-"));
    const again = spawnSync(process.execPath, [HARNESS, "--cache", w.cache, ...ONE()], { cwd: elsewhere, env: { ...w.env, GIT_CEILING_DIRECTORIES: tmpdir() }, encoding: "utf8" });
    assert.equal(again.status, 0, again.stderr);
    assert.equal(again.stdout, first.stdout);
  });

  it("refuses a kept log that has become a link since it was extracted", () => {
    const w = world();
    assert.equal(measure(w, ["--cache", w.cache, ...ONE()]).status, 0);
    const kept = join(w.cache, RUN, "attempt-1", "logs", "1_test-linux (baseline).txt");
    rmSync(kept);
    symlinkSync("/etc/hostname", kept);
    const ran = measure(w, ["--cache", w.cache, ...ONE()]);
    assert.equal(ran.status, 2);
    assert.match(ran.stderr, /is a link, which no log or profile archive holds/);
  });

  it("reads the head again when what it kept about it is not an answer", () => {
    const w = world();
    assert.equal(measure(w, ["--cache", w.cache, ...ONE()]).status, 0);
    write(join(w.cache, RUN, "attempt-1", "ci-yml-blob"), "not a blob");
    const ran = measure(w, ["--cache", w.cache, ...ONE()]);
    assert.equal(ran.status, 0, ran.stderr);
    assert.equal(readFileSync(join(w.cache, RUN, "attempt-1", "ci-yml-blob"), "utf8").trim(), w.blob);
  });

  it("reports an attempt still running, and a job whose log is missing, without measuring them", () => {
    const { stdout } = reported({ secondAttempt: "in_progress", logs: { "1 idle": null } });
    assert.match(stdout, new RegExp(`\\| baseline \\| ${RUN}#2@baseline \\| – \\| in_progress \\|`));
    assert.match(stdout, new RegExp(`\\| idle \\| ${RUN}#1@idle \\| test-linux \\| success \\(no log\\) \\|`));
    assert.match(stdout, /\| idle \| test-linux \| – \| – \|/, "a side with nothing measured has no mean");
  });

  it("reads a failed tier, a test-quick tier, a job with no tier, a partial restore and one it cannot tell", () => {
    const failed = reported(
      { logs: { "1 baseline": log({ summary: "     Summary [ 600.000s] 2 tests run: 1 passed, 1 failed, 0 skipped", cache: 'Restored from cache key "v0" full match: false.' }) } },
      ONE(),
    );
    assert.match(failed.stdout, /\| 2\/1\/1 \|/);
    assert.match(failed.stdout, /rust-cache: hit \(partial key\)/);
    const quick = reported({ logs: { "1 baseline": log({ tier: "> nx run onepipeline-e2e:test-quick" }) } }, ONE());
    assert.match(quick.stdout, /\| baseline \| test-linux \| 1\.0 \| 29\.0 \| 10\.0 \|/);
    const none = reported({ logs: { "1 baseline": log({ tier: null, cache: "Restoring nothing in particular" }) } }, ONE());
    assert.match(none.stdout, /\| baseline \| test-linux \| 1\.0 \| 29\.0 \| – \| – \|/);
    assert.match(none.stdout, /rust-cache: unknown/);
  });

  it("reads a job no matrix names, and takes the latest attempt when none is named", () => {
    const { stdout } = reported({ plain: true }, ["--side", `plain=${RUN}`]);
    assert.match(stdout, new RegExp(`\\| plain \\| ${RUN}#2 \\| test-linux \\| success \\| 1234567`));
    assert.match(stdout, /\| plain \| 4242#2 \| test-linux \| 3 \| 0, 0\.0 \| 0, 0\.0 \|/);
  });

  it("counts e2e records alone, sums a test's records, and reports an artifact with none", () => {
    const { stdout } = reported(
      {
        records: {
          101: {
            "e2e/views.one.again.json": record("views::one", 25, {}),
            "note/note.one.json": record("note::one", 1000, { onepipeline: { count: 9, s: 900 } }, { binary: "note" }),
          },
        },
      },
      ONE(),
    );
    assert.match(stdout, /engine invocations: 3, 5\.0 s of 595\.0 e2e test-s/, "the note record is not the e2e tier's, and views::one counts twice");
    const empty = reported(
      {
        records: {
          101: {
            "e2e/views.one.json": record("views::one", 1, {}, { binary: "note" }),
            "e2e/views.two.json": record("views::two", 1, {}, { binary: "note" }),
            "e2e/stale.json": record("views::stale", 1, {}, { binary: "note" }),
            "e2e/views.four.json": record("views::four", 1, {}, { binary: "note" }),
          },
        },
      },
      ONE(),
    );
    assert.match(empty.stdout, /\| baseline \| 4242#1@baseline \| test-linux \| 0 \| 0, 0\.0 \| 0, 0\.0 \|/);
  });

  it("reads what a log leaves out as no measurement", () => {
    const { stdout } = reported({ logs: { "1 baseline": log({ tasks: null, coverage: null, summary: null }) } }, ONE());
    assert.match(stdout, /\| baseline \| 4242#1@baseline \| test-linux \| success \| 1234567 \| [0-9a-f]{12} \| [0-9a-f]{12} \| – \|/);
    assert.match(stdout, /\| baseline \| test-linux \| 1\.0 \| 29\.0 \| – \| – \| – \|/);
  });

  it("compares against the first run of the first side that has records", () => {
    const { stdout } = reported({ expired: [101] }, ["--side", `baseline=${RUN}#1@baseline,${RUN}#2@baseline`, "--side", `idle=${RUN}#1@idle`]);
    assert.match(stdout, /\| baseline \| 4242#1@baseline \| test-linux \| – \| no records \| – \|/);
    assert.match(stdout, /\| idle \| 4242#1@idle \| test-linux \| 3 \| 1, 50\.0 \| 1, 70\.0 \|/);
  });

  it("reads the workflow again when the artifact name it kept is not one", () => {
    const w = world();
    assert.equal(measure(w, ["--cache", w.cache, ...ONE()]).status, 0);
    write(join(w.cache, RUN, "attempt-1", "profile-artifact-test-linux"), "a name nobody uploads");
    const ran = measure(w, ["--cache", w.cache, ...ONE()]);
    assert.equal(ran.status, 0, ran.stderr);
    assert.equal(readFileSync(join(w.cache, RUN, "attempt-1", "profile-artifact-test-linux"), "utf8").trim(), "test-profile-${{ github.job }}-${{ matrix.variant }}");
  });

  it("reads a log with no checkout line as naming no GITHUB_SHA, and the producer's own line for no tasks", () => {
    const { stdout } = reported({ logs: { "1 baseline": log({ checkout: null, tasks: tasksLine("test-linux", "") }) } }, ONE());
    assert.match(stdout, /\| baseline \| 4242#1@baseline \| test-linux \| success \| – \| [0-9a-f]{12} \| [0-9a-f]{12} \| 0 \|/);
  });

  it("quotes each line about saving once, and none of a test's own output", () => {
    const extra = ["save-cache: false", "save-cache: false", "ok 3 - a cache step that sets save-if: false is accepted", "# Subtest: save-if is read"];
    const { stdout } = reported({ logs: { "1 baseline": log({ extra }) } }, ONE());
    const said = /\| baseline \| 4242#1@baseline \| test-linux \| [^|]+ \| (.*) \|$/m.exec(stdout.split("## Cache restores")[1])[1];
    assert.equal(said, "github-cache: CACHE_READ_ONLY is set, so this job serves the READ-ONLY Actions-cache backend / save-if: false / save-cache: false");
  });

  it("compares another side's records when the first side has none for a job", () => {
    const { stdout } = reported({ expired: [101, 201] }, SIDES);
    assert.match(stdout, /\| idle \| 4242#1@idle \| test-linux \| 3 \| – \| – \|/);
  });

  it("groups each platform's runs apart, and shows a side without that platform as having no runs of it", () => {
    const { stdout } = reported({ macos: true });
    assert.match(stdout, /\| baseline \| test-macos \| 0\.0, 0\.0 \| 40\.0, 40\.0 \| 20\.0, 20\.0 \|/);
    assert.match(stdout, /\| idle \| test-macos \|  \|  \|  \|  \|  \| – \| – \|/);
    assert.match(stdout, /\| idle \| test-macos \| – \| – \| baseline 40\.0–40\.0; idle – \|/);
    assert.match(stdout, /test-macos, 1 tasks, selected by 2 run\(s\)/);
  });

  it("keeps a pipe in the job's own words from breaking the table", () => {
    const { stdout } = reported({ logs: { "1 baseline": log({ extra: ["save-if: ${{ a || b }}"] }) } }, ONE());
    assert.match(stdout, /save-if: \$\{\{ a \\\|\\\| b \}\}/);
  });

  it("shows no share for verbs of an engine that took no time", () => {
    const { stdout } = reported({ records: { 101: { "e2e/views.one.json": record("views::one", 1, { onepipeline: { count: 0, s: 0 }, [verbKey("start")]: { count: 0, s: 0 } }), "e2e/views.two.json": record("views::two", 1, {}) } } }, ONE());
    assert.match(stdout, /\| start \| 0 \| 0\.0 \| 0 \| – \|/);
  });

  it("drops an archive that does not extract, fails with 1, and fetches it again on the next invocation", () => {
    const w = world();
    const logs = `${REPO}/actions/runs/${RUN}/attempts/1/logs`;
    const good = readFileSync(join(w.root, "answers", encodeURIComponent(logs)));
    w.answer(logs, "not a zip");
    const broken = measure(w, ["--cache", w.cache, ...ONE()]);
    assert.equal(broken.status, 1);
    assert.match(broken.stderr, /logs\.zip is not an archive and is dropped/);
    w.answer(logs, good);
    const ran = measure(w, ["--cache", w.cache, ...ONE()]);
    assert.equal(ran.status, 0, ran.stderr);
    assert.equal(calls(w).filter((call) => call === logs).length, 2);
  });

  it("drops an archive whose listing reads but whose contents do not extract", () => {
    const w = world();
    const logs = `${REPO}/actions/runs/${RUN}/attempts/1/logs`;
    const dir = mkdtempSync(join(tmpdir(), "measure-crc-"));
    write(join(dir, "1_test-linux (baseline).txt"), "x".repeat(4096));
    execFileSync("zip", ["-q", "-0", "out.zip", "1_test-linux (baseline).txt"], { cwd: dir });
    const archive = readFileSync(join(dir, "out.zip"));
    archive[archive.indexOf("xxxx")] = "y".charCodeAt(0);
    w.answer(logs, archive);
    const broken = measure(w, ["--cache", w.cache, ...ONE()]);
    assert.equal(broken.status, 1);
    assert.match(broken.stderr, /logs\.zip did not extract and is dropped/);
    assert.ok(!existsSync(join(w.cache, RUN, "attempt-1", "logs.zip")));
    assert.ok(!existsSync(join(w.cache, RUN, "attempt-1", "logs")), "nothing it half extracted is kept");
    w.answer(logs, zipOf({ "1_test-linux (baseline).txt": log() }));
    const ran = measure(w, ["--cache", w.cache, ...ONE()]);
    assert.equal(ran.status, 0, ran.stderr);
    assert.match(ran.stdout, /\| baseline \| test-linux \| 1\.0 \| 29\.0 \| 10\.0 \|/);
  });

  it("refuses an archive naming an entry outside the directory it extracts to, before extracting it", () => {
    const w = world();
    const dir = mkdtempSync(join(tmpdir(), "measure-escape-"));
    execFileSync("python3", ["-I", "-c", "import sys, zipfile; z = zipfile.ZipFile(sys.argv[1], 'w'); z.writestr('../escaped.txt', 'x'); z.close()", join(dir, "out.zip")]);
    w.answer(`${REPO}/actions/runs/${RUN}/attempts/1/logs`, readFileSync(join(dir, "out.zip")));
    const ran = measure(w, ["--cache", w.cache, ...ONE()]);
    assert.equal(ran.status, 2);
    assert.match(ran.stderr, /holds \.\.\/escaped\.txt, outside the directory it extracts to/);
    assert.ok(!existsSync(join(w.cache, RUN, "escaped.txt")));
  });

  it("takes an attempt's earliest artifact of a name when it uploaded that name twice", () => {
    const { stdout } = reported({ duplicate: true }, ONE());
    assert.match(stdout, /\| baseline \| 4242#1@baseline \| test-linux \| 3 \| 0, 0\.0 \| 0, 0\.0 \|/);
  });

  it("times a restore that is the log's last step to the log's last line", () => {
    const last = `${log()}2026-11-13T10:15:00.0000000Z ##[group]Run Swatinem/rust-cache@v2.9.2\r\n2026-11-13T10:15:12.0000000Z Cache restored successfully\r\n`;
    assert.match(reported({ logs: { "1 baseline": last } }, ONE()).stdout, /; rust-cache: hit, 12\.0 s \|/);
  });

  it("reads each restore message the cache actions print", () => {
    for (const [line, outcome] of [
      ["No cache found.", "miss"],
      ["No GitHub Actions cache found for key: x", "miss"],
      ["Cache restored from key: v0-rust", "hit"],
      ["Cache restored successfully", "hit"],
    ]) {
      assert.match(reported({ logs: { "1 baseline": log({ cache: line }) } }, ONE()).stdout, new RegExp(`rust-cache: ${outcome.replace(/[()]/g, "\\$&")},`), line);
    }
  });

  it("reads every summary form nextest prints", () => {
    for (const [summary, counts] of [
      ["Summary [ 600.000s] 1165/1442 tests run: 1164 passed (1 slow), 1 failed, 1 skipped", "1165/1164/1"],
      ["Summary [ 600.000s] 3 tests run: 3 passed (2 slow) (1 flaky), 1 skipped", "3/3/0"],
      ["Summary [ 600.000s] 4 tests run: 1 passed, 1 timed out, 1 exec failed, 1 leaky", "4/1/2"],
      ["Summary [ 600.000s] 2 tests run: 1 passed, 1 not run", "2/1/0"],
      ["Summary [ 600.000s] 1 test run: 1 passed (1 leaky)", "1/1/0"],
    ]) {
      assert.match(reported({ logs: { "1 baseline": log({ summary: `     ${summary}` }) } }, ONE()).stdout, new RegExp(`\\| ${counts} \\|`), summary);
    }
  });

  it("shows a verb table no longer than fifteen rows, and says when records carry no verbs", () => {
    const many = Object.fromEntries(Array.from({ length: 20 }, (_, i) => [verbKey(`verb${i}`), { count: 1, s: i + 1 }]));
    const { stdout } = reported({ records: { 101: { "e2e/views.many.json": record("views::many", 10, { onepipeline: { count: 20, s: 210 }, ...many }) } } }, ONE());
    assert.equal((stdout.match(/^\| verb\d+ \|/gm) ?? []).length, 15);
    const bare = reported({ records: { 102: { "e2e/views.one.json": record("views::one", 300, {}), "e2e/views.two.json": record("views::two", 200, {}) } } }, ONE("idle"));
    assert.match(bare.stdout, /\(these records carry no per-verb entries\)/);
  });
});

describe("measure.mjs refusals", () => {
  it("refuses arguments it cannot read", () => {
    refused({}, ["--side", `baseline=${RUN}#0`], /not RUN\[#ATTEMPT\]\[@VARIANT\]/);
    refused({}, ["--side"], /--side needs a value/);
    refused({}, ["--side", "a=1", "--side", "a=2"], /side 'a' is named twice/);
    refused({}, ["--verbs", "y"], /unknown argument --verbs/);
    refused({}, [], /at least one --side/);
    refused({}, ["--side", "no-run-named"], /--side takes NAME=RUN/);
    const w = world();
    const ran = measure(w, ["--side", `baseline=${RUN}`]);
    assert.equal(ran.status, 2);
    assert.match(ran.stderr, /--cache DIR/);
  });

  it("refuses a record that is not the golden record's, naming the file and what is wrong", () => {
    const at = (file) => ({ records: { 101: file } });
    refused(at({ "e2e/views.two.json": { schema: 2, test: "views::two" } }), ONE(), /views\.two\.json does not have the golden profile record's shape/);
    refused(at({ "e2e/views.two.json": record("views::two", 1, {}, { schema: 1 }) }), ONE(), /views\.two\.json is schema 1, not 2/);
    refused(at({ "e2e/views.two.json": record("views::two", -1, {}) }), ONE(), /views\.two\.json: wall_s is not a duration/);
    refused(at({ "e2e/views.two.json": record("views::two", 1, { git: { count: 1.5, s: 1 } }) }), ONE(), /views\.two\.json: program git is not a count and a duration/);
    refused(at({ "e2e/views.two.json": record("views::two", 1, []) }), ONE(), /views\.two\.json does not have the golden profile record's shape/);
    refused(at({ "e2e/views.two.json": "{ half a record" }), ONE(), /views\.two\.json is not JSON/);
    refused(at({ "e2e/views.link.json": { link: "/etc/hostname" } }), ONE(), /holds e2e\/views\.link\.json, a link, which no log or profile archive holds; nothing is extracted/);
    const bad = structuredClone(GOLDEN.categories);
    bad.spawn.count = -1;
    refused(at({ "e2e/views.two.json": record("views::two", 1, {}, { categories: bad }) }), ONE(), /views\.two\.json: category spawn is not a count and a duration/);
  });

  it("refuses answers from gh that are not what it asked for", () => {
    refused({ attempt: () => ({ status: "sleeping", head_sha: "a".repeat(40), run_started_at: "2026-11-13T10:00:00Z" }) }, ONE(), /no known status, head SHA or start time/);
    refused(
      { jobsAnswer: () => ({ jobs: [{ id: 1, name: "test-linux (baseline)", conclusion: "maybe", created_at: "2026-11-13T10:00:00Z" }] }) },
      ONE(),
      /not a list of named jobs with known conclusions/,
    );
    refused({ artifactsAnswer: { total_count: 1, artifacts: [{ id: 1, name: "x" }] } }, ONE(), /its artifact listing is not a list of artifacts/);
    refused({ artifactsAnswer: { total_count: 120, artifacts: [] } }, ONE(), /its artifact listing holds 0 of 120 artifacts/);
    refused({ jobsAnswer: () => ({ total_count: 150, jobs: [] }) }, ONE(), /its job listing holds 0 of 150 jobs/);
    refused({ jobs: (jobs) => jobs.map(({ conclusion, ...job }) => job) }, ONE(), /not a list of named jobs with known conclusions/);
    refused({ run: { run_attempt: 0 } }, ONE(), /its record names no attempt/);
    refused({ attempt: () => ({ status: "completed", head_sha: ["a".repeat(40)], run_started_at: "2026-11-13T10:00:00Z" }) }, ONE(), /no known status, head SHA or start time/);
    refused({ jobs: (jobs) => jobs.map((job) => ({ ...job, completed_at: "soon" })) }, ONE(), /not a list of named jobs with known conclusions/);
    refused({ jobs: (jobs) => jobs.map((job) => ({ ...job, started_at: "2026-11-13T09:00:00Z" })) }, ONE(), /created, started and completed times are not in order/);
  });

  it("refuses a measurement line that is there but does not read", () => {
    refused({ logs: { "1 baseline": log({ summary: "     Summary [ 600.000s] 1 test run: 2 passed, 0 skipped" }) } }, ONE(), /the e2e tier's summary reads/);
    refused({ logs: { "1 baseline": log({ summary: "     Summary [ fast ] everything passed" }) } }, ONE(), /the e2e tier's summary reads/);
    refused({ logs: { "1 baseline": log({ summary: "     Summary [ 600.000s] 2 tests run: 1 passed, nonsense failed" }) } }, ONE(), /the e2e tier's summary reads/);
    refused({ logs: { "1 baseline": log({ summary: "     Summary [ 600.000s] 2 tests run: 1 passed, 1 flubbed" }) } }, ONE(), /the e2e tier's summary reads/);
    refused({ logs: { "1 baseline": log({ coverage: coverageLine("196.5") }) } }, ONE(), /the line coverage reads/);
    refused({ logs: { "1 baseline": log({ coverage: "lines percent: lots" }) } }, ONE(), /the line coverage reads/);
    refused({ logs: { "1 baseline": log({ tasks: "ci-job: test-linux runs 3 task(s): a:b c:d" }) } }, ONE(), /the Nx task selection reads/);
  });

  it("refuses a workflow artifact name it cannot fill, or a workflow that names none", () => {
    refused({ workflowName: "test-profile-${{ matrix.os }}" }, ONE(), /with an expression this does not fill/);
    refused({ workflowName: "profiles" }, ONE(), /workflow names no profile-artifact-test-linux this reads/);
  });

  it("refuses a log line that opens like a time but is not one, and a step that ends before it starts", () => {
    refused({ logs: { "1 baseline": log({ stamp: "2026-11-13T99:00:36.0000000Z" }) } }, ONE(), /does not open with a time/);
    refused({ logs: { "1 baseline": log({ ruststart: "2026-11-13T10:00:31.5000000Z" }) } }, ONE(), /ends before it starts/);
  });

  it("refuses a kept answer that is not JSON, saying how to read it again", () => {
    const w = world();
    write(join(w.cache, RUN, "run.json"), "not json");
    const ran = measure(w, ["--cache", w.cache, ...ONE()]);
    assert.equal(ran.status, 2);
    assert.match(ran.stderr, /run\.json is not JSON; delete it to read it again/);
  });

  it("fails with 1 when gh or git cannot answer", () => {
    refused({}, ["--side", "baseline=999#1@baseline"], /no answer for .*runs\/999/, 1);
    refused({ head: "f".repeat(40) }, ONE(), /git/, 1);
  });
});
