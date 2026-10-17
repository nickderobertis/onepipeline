// The test-profile report, run as its command over two platforms' record directories
// in the shape `onepipeline_testfakes::profile` writes them.
import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { after, before, test } from "node:test";
import { fileURLToPath } from "node:url";
import { CATEGORIES, SCHEMA, loadRecords, totals } from "../../scripts/test-profile-report.mjs";

const root = join(dirname(fileURLToPath(import.meta.url)), "..", "..");
const script = join(root, "scripts", "test-profile-report.mjs");
// The record a real test wrote, at the shape `tests/e2e/profiling.rs` holds every
// record the harness writes to: the one source both sides of the format answer to.
const golden = JSON.parse(
  readFileSync(join(root, "tests", "golden", "test-profile-record-v1.json"), "utf8"),
);

let scratch;

function record(test, { run = "now", attempt = 1, startedAt = 100, wall, spent = {} }) {
  const categories = {};
  let attributed = 0;
  for (const category of CATEGORIES) {
    const s = spent[category] ?? 0;
    attributed += s;
    categories[category] = { count: s > 0 ? 1 : 0, s, inclusive_s: s };
  }
  categories.wait.polls = 0;
  return {
    schema: 1,
    binary: "e2e",
    test,
    run_id: run,
    attempt,
    started_at: startedAt,
    wall_s: wall,
    categories,
    unattributed_s: wall - attributed,
  };
}

function write(dir, records) {
  mkdirSync(join(dir, "e2e"), { recursive: true });
  records.forEach((r, i) => {
    writeFileSync(join(dir, "e2e", `${i}.${r.test.replace("::", ".")}.json`), JSON.stringify(r));
  });
  // Neither of these is a record, and both are skipped.
  writeFileSync(join(dir, "e2e", ".staged.json"), "{");
  writeFileSync(join(dir, "e2e", "other.json"), JSON.stringify({ schema: 99, test: "x" }));
}

before(() => {
  scratch = mkdtempSync(join(tmpdir(), "test-profile-report-"));
  write(join(scratch, "linux"), [
    record("readjust::slow", { wall: 10, spent: { git: 4, spawn: 4 } }),
    record("readjust::quick", { wall: 2, spent: { spawn: 1, fs: 1 } }),
    record("views::middling", { wall: 5, spent: { wait: 5 } }),
    // An older run's record of a test the full run did not reach, and a later run
    // of one test again: both set aside for the run with the most records.
    record("views::gone", { run: "before", startedAt: 1, wall: 99 }),
    record("readjust::slow", { run: "again", startedAt: 500, wall: 7 }),
    // The first attempt of a retried test: the last attempt is read.
    record("views::middling", { attempt: 0, wall: 50 }),
  ]);
  write(join(scratch, "windows"), [
    record("readjust::slow", { wall: 30, spent: { git: 16, spawn: 8 } }),
    record("readjust::quick", { wall: 6, spent: { spawn: 4, fs: 2 } }),
    record("views::middling", { wall: 5, spent: { wait: 5 } }),
    record("windows::only", { wall: 1, spent: { fs: 1 } }),
  ]);
});

after(() => rmSync(scratch, { recursive: true, force: true }));

function report(...args) {
  const ran = spawnSync(process.execPath, [script, ...args], { encoding: "utf8" });
  assert.equal(ran.status, 0, ran.stderr);
  return ran.stdout;
}

test("the reader reads the record the harness writes, field for field", () => {
  assert.equal(SCHEMA, golden.schema);
  assert.deepEqual(CATEGORIES, Object.keys(golden.categories));
  const dir = join(scratch, "golden");
  mkdirSync(dir);
  writeFileSync(join(dir, "record.json"), JSON.stringify(golden));
  const [read] = loadRecords(dir);
  assert.deepEqual(read, golden);
  const total = totals([read]);
  assert.equal(total.wall, golden.wall_s);
  assert.equal(total.unattributed, golden.unattributed_s);
  assert.equal(total.counts.polls, golden.categories.wait.polls);
  for (const category of CATEGORIES) {
    assert.equal(total.categories[category], golden.categories[category].s);
    assert.equal(total.counts[category], golden.categories[category].count);
  }
});

test("a platform's breakdown ranks its tests and modules and attributes each one's time", () => {
  const out = JSON.parse(
    report("--json", "--top-tests", "2", "--top-modules", "5", `linux=${join(scratch, "linux")}`),
  );
  const [linux] = out.platforms;
  assert.equal(linux.run, "now");
  assert.equal(linux.setAside, 2);
  assert.equal(linux.breakdown.total.tests, 3);
  assert.equal(linux.breakdown.total.wall, 17);
  assert.deepEqual(
    linux.breakdown.tests.map((t) => [t.name, t.wall]),
    [
      ["e2e::readjust::slow", 10],
      ["e2e::views::middling", 5],
    ],
  );
  assert.deepEqual(
    linux.breakdown.modules.map((m) => [
      m.name,
      m.tests,
      m.wall,
      m.categories.git,
      m.categories.spawn,
    ]),
    [
      ["e2e::readjust", 2, 12, 4, 5],
      ["e2e::views", 1, 5, 0, 0],
    ],
  );
  assert.equal(linux.breakdown.modules[0].unattributed, 2);
  assert.equal(out.missingBaseline, false);
  assert.deepEqual(out.ratios, []);
});

test("each platform's category cost is a ratio to the baseline's over the tests both ran", () => {
  const out = JSON.parse(
    report("--json", `linux=${join(scratch, "linux")}`, `windows=${join(scratch, "windows")}`),
  );
  const [{ label, ratios }] = out.ratios;
  assert.equal(label, "windows");
  assert.equal(ratios.tests, 3);
  assert.equal(ratios.wall, 41 / 17);
  assert.equal(ratios.categories.git, 4);
  assert.equal(ratios.categories.spawn, 12 / 5);
  assert.equal(ratios.categories.fs, 2);
  assert.equal(ratios.categories.wait, 1);
  // Neither platform spent anything building, so there is no ratio to give.
  assert.equal(ratios.categories.build, null);

  const markdown = report(`linux=${join(scratch, "linux")}`, `windows=${join(scratch, "windows")}`);
  assert.match(markdown, /## Against linux/);
  assert.match(markdown, /\| windows \| 3 \| 2\.41x \| 2\.40x \| 4\.00x \| - \|/);
  assert.match(markdown, /\| e2e::readjust::slow \| 1 \| 10\.0 \| 40\.0% \| 40\.0% \|/);
});

test("a missing baseline is named rather than compared against", () => {
  const markdown = report(`windows=${join(scratch, "windows")}`);
  assert.match(markdown, /No records for linux, so no platform is compared with it\./);
});

test("a run nothing recorded is refused by name", () => {
  const ran = spawnSync(
    process.execPath,
    [script, "--run", "linux=never", `linux=${join(scratch, "linux")}`],
    { encoding: "utf8" },
  );
  assert.equal(ran.status, 1);
  assert.match(ran.stderr, /no record belongs to run never/);
});
