// Measurement only (spike-child-waits, never lands): the child-wait comparison, run as its
// command over a recorded pair of jobs — two e2e journeys run locally with every
// prototype off (`before`) and on (`after`), each job's test-profile records, launch
// trace and timestamped nextest output — and over copies of it changed the one way each
// part of the comparison exists to notice. Its local timings verify parsing and
// arithmetic only; pull-request savings figures come from hosted runs alone.
import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import {
  appendFileSync,
  cpSync,
  mkdtempSync,
  readFileSync,
  readdirSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { after, before, test } from "node:test";
import { fileURLToPath } from "node:url";

const root = join(dirname(fileURLToPath(import.meta.url)), "..", "..");
const script = join(root, "scripts", "child-waits.mjs");
const recorded = join(root, "npm", "test", "golden", "child-waits");
const CANNOT_START =
  "command_step::a_command_that_cannot_start_says_too_much_or_is_signalled_settles_command_failed";

let scratch;
before(() => {
  scratch = mkdtempSync(join(tmpdir(), "child-waits-"));
});
after(() => rmSync(scratch, { recursive: true, force: true }));

function compare(...args) {
  return spawnSync(process.execPath, [script, "compare", ...args], { encoding: "utf8" });
}

/** The rows of the Markdown table under `heading`, each as its cells. */
function rows(out, heading) {
  const at = out.indexOf(`### ${heading}`);
  assert.notEqual(at, -1, `no '${heading}' section in:\n${out}`);
  const lines = out.slice(at).split("\n").slice(1);
  const start = lines.findIndex((l) => l.startsWith("|"));
  const body = [];
  for (const line of lines.slice(start + 2)) {
    if (!line.startsWith("|")) break;
    body.push(
      line
        .slice(1, -1)
        .split("|")
        .map((c) => c.trim()),
    );
  }
  return body;
}

/** A copy of one recorded side, to change. */
function copyOf(side) {
  const dir = mkdtempSync(join(scratch, `${side}-`));
  cpSync(join(recorded, side), dir, { recursive: true });
  return dir;
}

test("the recorded pair prints both runs, the verdict, the changed waits and the journey's launches", () => {
  const out = compare(
    `before:off=${join(recorded, "before")}`,
    `after:on=${join(recorded, "after")}`,
  );
  assert.equal(out.status, 0, out.stderr);
  const runs = rows(out.stdout, "Runs");
  assert.deepEqual(
    runs.map((r) => [r[0], r[1], r[2], r[3], r[8]]),
    [
      ["before", "off", "0000000", "1111111", "2/2"],
      ["after", "on", "0000000", "1111111", "2/2"],
    ],
  );
  assert.match(out.stdout, /Difference, after − before: -?\d+\.\d\d min — (decided|undecided)/);
  assert.match(out.stdout, /Typical pull request \(estimate, the difference × 0\.900 /);
  const waits = Object.fromEntries(
    rows(out.stdout, "Waits at the changed sites").map((r) => [`${r[0]} ${r[1]}`, r]),
  );
  // The exit-notified command-step and dispatch-env waits are the same waits, polled as
  // often, whatever they cost: the comparison reports both sides' counts at each.
  assert.deepEqual(waits["onepipeline::commandstep::run wait"].slice(2, 4), ["3.0", "6.0"]);
  assert.deepEqual(waits["onepipeline::commandstep::run wait"].slice(5, 7), ["3.0", "6.0"]);
  assert.deepEqual(waits["onepipeline::dispatchenv::run wait"].slice(2, 4), ["2.0", "4.0"]);
  assert.ok(waits["harness::waited_on poll"], "the harness's own polls are a changed site");
  assert.deepEqual(rows(out.stdout, "CLI launches per changed journey"), [
    [CANNOT_START, "5.0", "2.0"],
  ]);
  assert.match(out.stdout, /2 e2e tests passed in a before run\./);
  assert.match(
    out.stdout,
    /after on: 2 passed in the tier; of the before runs' passes, not run and passed here: none;/,
  );
});

test("a test that failed in the tier and passed on the job's re-run is named on its side and as not passed there", () => {
  const dir = copyOf("after");
  appendFileSync(
    join(dir, "job.log"),
    [
      `2026-11-15T16:39:24.0000000Z         FAIL [   1.000s] (1/2) onepipeline::e2e ${CANNOT_START}`,
      `2026-11-15T16:45:00.0000000Z         PASS [   1.000s] (1/1) onepipeline::e2e ${CANNOT_START}`,
      "",
    ].join("\n"),
  );
  const out = compare(`before:off=${join(recorded, "before")}`, `after:retried=${dir}`);
  assert.equal(out.status, 0, out.stderr);
  assert.match(
    out.stdout,
    new RegExp(
      `after retried: 1 passed in the tier; of the before runs' passes, not run and passed here: ${CANNOT_START}; failed in the tier, passed on the job's re-run: ${CANNOT_START}; failed and not passed again: none\\.`,
    ),
  );
});

test("a run's meta.json names it by run, attempt and job, and gives its full GITHUB_SHA", () => {
  const dir = copyOf("before");
  writeFileSync(
    join(dir, "meta.json"),
    JSON.stringify({ run: 1, attempt: 2, job: 3, head: "abc", github_sha: "def" }),
  );
  const out = compare(`before:named=${dir}`, `after:on=${join(recorded, "after")}`);
  assert.equal(out.status, 0, out.stderr);
  assert.deepEqual(rows(out.stdout, "Runs")[0].slice(1, 4), ["1/2/3 (named)", "def", "abc"]);
});

test("a job without its log is refused with what to do, and a missing side is a usage error", () => {
  const dir = copyOf("after");
  rmSync(join(dir, "job.log"));
  const refused = compare(`before:off=${join(recorded, "before")}`, `after:nolog=${dir}`);
  assert.equal(refused.status, 1);
  assert.match(refused.stderr, /after:nolog: .* holds no job\.log\nnext: save the job's log there/);
  const usage = compare(`before:off=${join(recorded, "before")}`);
  assert.equal(usage.status, 2);
  assert.match(usage.stderr, /name at least one before and one after run/);
});

// Alter only the summary's measured duration in copies of the recording: this
// isolates the comparison's decision without inventing test outcomes or traces.
function withMinutes(side, minutes) {
  const dir = copyOf(side);
  const log = join(dir, "job.log");
  writeFileSync(
    log,
    readFileSync(log, "utf8").replace(/Summary \[\s*[\d.]+s\]/, `Summary [ ${minutes * 60}s]`),
  );
  return dir;
}

test("non-overlapping ranges stay undecided when their mean difference is inside either side's spread", () => {
  const out = compare(
    `before:low=${withMinutes("before", 10)}`,
    `before:high=${withMinutes("before", 100)}`,
    `after:low=${withMinutes("after", 101)}`,
    `after:high=${withMinutes("after", 102)}`,
  );
  assert.equal(out.status, 0, out.stderr);
  assert.match(out.stdout, /Difference, after − before: 46.50 min — undecided:/);
});

test("a difference outside both spreads and with non-overlapping ranges is decided", () => {
  const out = compare(
    `before:low=${withMinutes("before", 10)}`,
    `before:high=${withMinutes("before", 11)}`,
    `after:low=${withMinutes("after", 20)}`,
    `after:high=${withMinutes("after", 21)}`,
  );
  assert.equal(out.status, 0, out.stderr);
  assert.match(out.stdout, /Difference, after − before: 10.00 min — decided:/);
});

test("means, ranges, spreads and the share-scaled difference are calculated from every run", () => {
  const out = compare(
    "--share",
    "0.5",
    `before:low=${withMinutes("before", 10)}`,
    `before:high=${withMinutes("before", 20)}`,
    `after:low=${withMinutes("after", 40)}`,
    `after:high=${withMinutes("after", 60)}`,
  );
  assert.equal(out.status, 0, out.stderr);
  assert.deepEqual(rows(out.stdout, "E2e tier minutes"), [
    ["before", "15.00", "10.00–20.00", "10.00"],
    ["after", "50.00", "40.00–60.00", "20.00"],
  ]);
  assert.match(out.stdout, /Difference, after − before: 35.00 min — decided:/);
  assert.match(
    out.stdout,
    /Typical pull request \(estimate, the difference × 0.500[^\n]+17.50 min/,
  );
});

test("a missing baseline test and a failure with no successful rerun are both named", () => {
  const missing = copyOf("after");
  const profiles = join(missing, "test-profile", "e2e");
  const profile = readdirSync(profiles).find((name) => name.startsWith("command_step."));
  assert.ok(profile);
  rmSync(join(profiles, profile));
  const missingLog = join(missing, "job.log");
  writeFileSync(
    missingLog,
    readFileSync(missingLog, "utf8")
      .split("\n")
      .filter((line) => !line.includes(CANNOT_START))
      .join("\n"),
  );
  const out = compare(`before:off=${join(recorded, "before")}`, `after:missing=${missing}`);
  assert.equal(out.status, 0, out.stderr);
  assert.match(out.stdout, new RegExp(`not run and passed here: ${CANNOT_START};`));
  const failed = copyOf("after");
  appendFileSync(
    join(failed, "job.log"),
    `2026-11-15T16:39:24.0000000Z FAIL [ 1.000s] (1/2) onepipeline::e2e ${CANNOT_START}\n`,
  );
  const failure = compare(`before:off=${join(recorded, "before")}`, `after:failed=${failed}`);
  assert.equal(failure.status, 0, failure.stderr);
  assert.match(failure.stdout, new RegExp(`failed and not passed again: ${CANNOT_START}\\.`));
});

test("records without a completed e2e summary are refused with a recovery action", () => {
  const dir = copyOf("after");
  const log = join(dir, "job.log");
  writeFileSync(log, readFileSync(log, "utf8").replace(/^.*Summary.*$/m, ""));
  const out = compare(`before:off=${join(recorded, "before")}`, `after:unfinished=${dir}`);
  assert.equal(out.status, 1);
  assert.match(out.stderr, /holds no completed e2e nextest run\nnext: fold a job/);
});

test("invalid metadata is refused instead of being rendered as a hosted run identity", () => {
  for (const meta of [
    null,
    [],
    { run: 1 },
    { attempt: 1 },
    { job: 1 },
    { run: "one" },
    { attempt: 0 },
    { job: -1 },
    { head: 4 },
    { github_sha: [] },
    { typo: 1 },
  ]) {
    const dir = copyOf("after");
    writeFileSync(join(dir, "meta.json"), JSON.stringify(meta));
    const out = compare(`before:off=${join(recorded, "before")}`, `after:bad=${dir}`);
    assert.equal(out.status, 1, `accepted ${JSON.stringify(meta)}`);
    assert.match(out.stderr, /meta.json.*invalid run metadata/);
    assert.match(out.stderr, /next: use only/);
  }
});

test("an unfinished profile without a nextest pass is not promoted to a passing test", () => {
  const dir = copyOf("after");
  const profiles = join(dir, "test-profile", "e2e");
  const path = join(
    profiles,
    readdirSync(profiles).find((name) => name.startsWith("command_step.")),
  );
  const record = JSON.parse(readFileSync(path, "utf8"));
  record.ended = "unfinished";
  writeFileSync(path, JSON.stringify(record));
  const log = join(dir, "job.log");
  writeFileSync(
    log,
    readFileSync(log, "utf8")
      .split("\n")
      .filter((line) => !line.includes(CANNOT_START))
      .join("\n"),
  );
  const out = compare(`before:off=${join(recorded, "before")}`, `after:unfinished=${dir}`);
  assert.equal(out.status, 0, out.stderr);
  assert.equal(rows(out.stdout, "CLI launches per changed journey")[0][2], "not recorded");
  assert.match(
    out.stdout,
    new RegExp(
      `after unfinished: 1 passed in the tier;[^\n]+not run and passed here: ${CANNOT_START};`,
    ),
  );
});

test("a pass half a second after the tier summary is a job rerun recovery", () => {
  const dir = copyOf("after");
  appendFileSync(
    join(dir, "job.log"),
    [
      `2026-11-15T16:39:24.0000000Z FAIL [ 1.000s] (1/2) onepipeline::e2e ${CANNOT_START}`,
      `2026-11-15T16:39:25.6720000Z PASS [ 0.100s] (1/1) onepipeline::e2e ${CANNOT_START}`,
      "",
    ].join("\n"),
  );
  const out = compare(`before:off=${join(recorded, "before")}`, `after:retried=${dir}`);
  assert.equal(out.status, 0, out.stderr);
  assert.match(
    out.stdout,
    new RegExp(
      `failed in the tier, passed on the job's re-run: ${CANNOT_START}; failed and not passed again: none`,
    ),
  );
});

test("unknown commands, incomplete arguments and invalid shares are usage errors", () => {
  const pairs = [`before:off=${join(recorded, "before")}`, `after:on=${join(recorded, "after")}`];
  for (const args of [
    [],
    ["unknown"],
    ["compare", "bad"],
    ["compare", "--share"],
    ["compare", "--share", "0", ...pairs],
    ["compare", "--share", "1.1", ...pairs],
    ["compare", "--share", "NaN", ...pairs],
  ]) {
    const out = spawnSync(process.execPath, [script, ...args], { encoding: "utf8" });
    assert.equal(out.status, 2, `${args}: ${out.stderr}`);
    assert.match(out.stderr, /next: run 'node scripts\/child-waits.mjs compare/);
  }
});

test("a quiet tier with every failure named confirms the other completed profiles passed", () => {
  const dir = copyOf("after");
  const log = join(dir, "job.log");
  const quiet = readFileSync(log, "utf8")
    .split("\n")
    .filter((line) => !line.includes("PASS"))
    .join("\n")
    .replace("2 passed, 2154 skipped", "1 passed, 1 failed, 2154 skipped")
    .replace(
      /^(.*Summary.*)$/m,
      `2026-11-15T16:39:24.8670000Z FAIL [ 1.000s] (1/2) onepipeline::e2e ${CANNOT_START}\n$1`,
    );
  writeFileSync(log, quiet);
  const out = compare(`before:off=${join(recorded, "before")}`, `after:quiet=${dir}`);
  assert.equal(out.status, 0, out.stderr);
  assert.match(out.stdout, /after quiet: 1 passed in the tier;/);
  assert.match(out.stdout, new RegExp(`not run and passed here: ${CANNOT_START};`));
});

test("terminal non-PASS outcomes survive ANSI formatting, while malformed timestamps and progress do not become failures", () => {
  for (const outcome of [
    "TIMEOUT",
    "SIGSEGV",
    "SIGABRT",
    "SIGKILL",
    "SIGTERM",
    "LEAK-FAIL",
    "EXEC-FAIL",
  ]) {
    const dir = copyOf("after");
    const log = join(dir, "job.log");
    const other =
      "dispatch_env_hook::a_named_hook_runs_before_every_node_dispatch_and_its_env_reaches_that_child_alone";
    const text = readFileSync(log, "utf8")
      .split("\n")
      .map((line) =>
        line.includes(CANNOT_START)
          ? line
              .replace("PASS", `\u001b[31;1m${outcome}\u001b[0m`)
              .replace("onepipeline::e2e", "\u001b[35;1monepipeline::e2e\u001b[0m")
          : line,
      )
      .join("\n")
      .replace("2 passed, 2154 skipped", "1 passed, 1 failed, 2154 skipped");
    writeFileSync(
      log,
      `${text}\nnot-a-timestamp FAIL [ 1.000s] onepipeline::e2e ${other}\n2026-11-15T16:39:24.0000000Z SLOW [> 60.000s] onepipeline::e2e ${other}\n`,
    );
    const out = compare(`before:off=${join(recorded, "before")}`, `after:decorated=${dir}`);
    assert.equal(out.status, 0, out.stderr);
    assert.match(out.stdout, /after decorated: 1 passed in the tier;/);
    assert.match(out.stdout, new RegExp(`failed and not passed again: ${CANNOT_START}\\.`));
  }
});

test("wait seconds and harness totals are means across every supplied run", () => {
  const out = compare(
    `before:one=${join(recorded, "before")}`,
    `before:two=${join(recorded, "before")}`,
    `after:one=${join(recorded, "after")}`,
    `after:two=${join(recorded, "after")}`,
  );
  assert.equal(out.status, 0, out.stderr);
  const waits = Object.fromEntries(
    rows(out.stdout, "Waits at the changed sites").map((r) => [`${r[0]} ${r[1]}`, r.slice(2)]),
  );
  assert.deepEqual(waits["onepipeline::commandstep::run wait"], [
    "3.0",
    "6.0",
    "0.2",
    "3.0",
    "6.0",
    "0.0",
  ]);
  assert.deepEqual(waits["harness::waited_on poll"], ["6.0", "0.0", "0.5", "6.0", "0.0", "0.5"]);
});

test("processor identity and timestamp-derived log spans are printed, with an explicit missing-host fallback", () => {
  const args = [`before:off=${join(recorded, "before")}`, `after:on=${join(recorded, "after")}`];
  const out = compare(...args);
  assert.equal(out.status, 0, out.stderr);
  assert.deepEqual(
    rows(out.stdout, "Runs").map((r) => r.slice(4, 7)),
    [
      ["AMD Ryzen Threadripper 1920X 12-Core Processor (20)", "0.08", "0.02"],
      ["AMD Ryzen Threadripper 1920X 12-Core Processor (20)", "0.07", "0.01"],
    ],
  );
  const dir = copyOf("after");
  rmSync(join(dir, "test-profile", "launch-host.json"));
  const missing = compare(args[0], `after:nohost=${dir}`);
  assert.equal(missing.status, 0, missing.stderr);
  assert.equal(rows(missing.stdout, "Runs")[1][4], "not recorded");
});

test("an artifact with no changed-site or changed-journey records says what was not recorded", () => {
  const dir = copyOf("after");
  const profiles = join(dir, "test-profile", "e2e");
  let log = readFileSync(join(dir, "job.log"), "utf8");
  for (const [index, file] of readdirSync(profiles).entries()) {
    const path = join(profiles, file);
    const record = JSON.parse(readFileSync(path, "utf8"));
    const name = `control::journey_${index}`;
    log = log.replaceAll(record.test, name);
    record.test = name;
    writeFileSync(path, JSON.stringify(record));
  }
  writeFileSync(join(dir, "job.log"), log);
  rmSync(join(dir, "test-profile", "launch"), { recursive: true });
  const out = compare(`before:control=${dir}`, `after:control=${dir}`);
  assert.equal(out.status, 0, out.stderr);
  assert.match(out.stdout, /No wait at a changed site was recorded\./);
  assert.match(out.stdout, /No CLI launch count for a changed journey was recorded\./);
});

test("a successful LEAK warning remains a pass rather than a failed test", () => {
  const dir = copyOf("after");
  const log = join(dir, "job.log");
  writeFileSync(
    log,
    readFileSync(log, "utf8")
      .split("\n")
      .map((line) => (line.includes(CANNOT_START) ? line.replace("PASS", "LEAK") : line))
      .join("\n"),
  );
  const out = compare(`before:off=${join(recorded, "before")}`, `after:leak=${dir}`);
  assert.equal(out.status, 0, out.stderr);
  assert.match(
    out.stdout,
    /after leak: 2 passed in the tier;[^\n]+failed and not passed again: none\./,
  );
});
