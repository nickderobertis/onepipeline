// Measurement only (spike-child-waits, never lands): the child-wait comparison, run as its
// command over a recorded pair of jobs — two e2e journeys run locally with every
// prototype off (`before`) and on (`after`), each job's test-profile records, launch
// trace and timestamped nextest output — and over copies of it changed the one way each
// part of the comparison exists to notice. The recording's timings are a local slice's
// and are never asserted: the comparison's figures come from hosted runs only.
import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { appendFileSync, cpSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
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
