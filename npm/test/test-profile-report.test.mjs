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
  readFileSync(join(root, "tests", "golden", "test-profile-record-v2.json"), "utf8"),
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
    schema: SCHEMA,
    binary: "e2e",
    test,
    run_id: run,
    attempt,
    started_at: startedAt,
    ended: "test-thread",
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
  // A record its writer is still staging, which the reader passes over.
  writeFileSync(join(dir, "e2e", ".staged.json"), "{");
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

function run(...args) {
  return spawnSync(process.execPath, [script, ...args], { encoding: "utf8" });
}

let outputs = 0;

/** The report the command writes to its --out file; what it prints is one line. */
function report(...args) {
  const out = join(scratch, `report-${outputs++}`);
  const ran = run("--out", out, ...args);
  assert.equal(ran.status, 0, ran.stderr);
  assert.equal(ran.stdout.trim().split("\n").length, 1, ran.stdout);
  assert.match(ran.stdout, new RegExp(`wrote ${out}`));
  return readFileSync(out, "utf8");
}

function refused(args, message, next) {
  const ran = run(...args);
  assert.equal(ran.status, 1, ran.stdout);
  assert.match(ran.stderr, message);
  assert.match(ran.stderr, next);
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

test("a run named by --run is read, and one nothing recorded is refused naming the runs there are", () => {
  const out = JSON.parse(
    report("--json", "--run", "linux=before", `linux=${join(scratch, "linux")}`),
  );
  assert.equal(out.platforms[0].run, "before");
  assert.equal(out.platforms[0].breakdown.total.tests, 1);
  refused(
    ["--out", join(scratch, "x"), "--run", "linux=never", `linux=${join(scratch, "linux")}`],
    /no record belongs to run never/,
    /next: name one of the runs these records hold: .*now/,
  );
});

test("of two runs with as many records, the newer is read", () => {
  const dir = join(scratch, "tie");
  write(dir, [
    record("a::one", { run: "older", startedAt: 1, wall: 1 }),
    record("a::one", { run: "newer", startedAt: 2, wall: 2 }),
  ]);
  const out = JSON.parse(report("--json", `tie=${dir}`));
  assert.equal(out.platforms[0].run, "newer");
});

test("a directory with no records reports none rather than inventing a run", () => {
  const dir = join(scratch, "empty");
  mkdirSync(dir);
  const out = JSON.parse(report("--json", `empty=${dir}`));
  assert.equal(out.platforms[0].run, null);
  assert.equal(out.platforms[0].breakdown.total.tests, 0);
  assert.match(report(`empty=${dir}`), /No records\./);
});

test("a file that is not a record of this schema is refused by its path", () => {
  const dir = join(scratch, "other");
  write(dir, [record("a::one", { wall: 1 })]);
  writeFileSync(join(dir, "e2e", "newer.json"), JSON.stringify({ ...golden, schema: SCHEMA + 1 }));
  refused(
    ["--out", join(scratch, "x"), `other=${dir}`],
    /newer\.json is not a test-profile record: it is schema 3, and this reads 2/,
    /next: remove .*newer\.json, or rerun the test that wrote it/,
  );
  const broken = join(scratch, "broken");
  write(broken, [{ ...golden, wall_s: -1 }]);
  refused(
    ["--out", join(scratch, "x"), `broken=${broken}`],
    /wall_s is not a nonnegative number/,
    /next:/,
  );
});

test("an invocation that does not say what to read or where to write is refused with one to copy", () => {
  const linux = `linux=${join(scratch, "linux")}`;
  const usage = /next: .*node scripts\/test-profile-report\.mjs --out report\.md/;
  refused([linux], /--out names no file for the report/, usage);
  refused(["--out", join(scratch, "x")], /no LABEL=DIR of records was named/, usage);
  refused(
    ["--out", join(scratch, "x"), "--top-tests", "-3", linux],
    /--top-tests -3 is not a whole number/,
    /next:/,
  );
  refused(
    ["--out", join(scratch, "x"), "--top-tests", "1.5", linux],
    /is not a whole number/,
    /next:/,
  );
  refused(["--out", join(scratch, "x"), linux, linux], /linux names two directories/, /next:/);
  refused(["--out", join(scratch, "x"), "--run", "linux", linux], /is not a LABEL=RUN_ID/, /next:/);
  refused(
    ["--out", join(scratch, "x"), "--run", "mac=r", linux],
    /--run names mac, which no LABEL=DIR does/,
    /next:/,
  );
  refused(["--out", join(scratch, "x"), "=dir"], /is not a LABEL=DIR/, /next:/);
  refused(
    ["--out", join(scratch, "x"), join(scratch, "x"), linux],
    /is not an option or a LABEL=DIR/,
    usage,
  );
  refused(
    ["--out", join(scratch, "x"), `gone=${join(scratch, "no-such")}`],
    /cannot be read/,
    /next: name a directory of records/,
  );
});

test("a record file that is not JSON is refused by its path", () => {
  const dir = join(scratch, "torn");
  write(dir, [record("a::one", { wall: 1 })]);
  writeFileSync(join(dir, "e2e", "torn.json"), '{"schema": 1, "test"');
  refused(
    ["--out", join(scratch, "x"), `torn=${dir}`],
    /torn\.json is not JSON/,
    /next: remove .*torn\.json, or rerun the test that wrote it/,
  );
});

test("a report that cannot be written says where it was going and what to fix", () => {
  refused(
    ["--out", join(scratch, "no-such-dir", "report.md"), `linux=${join(scratch, "linux")}`],
    /--out .*no-such-dir.*report\.md cannot be written/,
    /next: point --out at a file in a directory that exists/,
  );
});

test("records a test binary run by hand wrote, with no run id, are read as one run", () => {
  const dir = join(scratch, "by-hand");
  write(dir, [
    { ...record("a::one", { wall: 1 }), run_id: null, attempt: null },
    { ...record("a::two", { wall: 2 }), run_id: null, attempt: null },
    record("a::one", { run: "nextest", wall: 9 }),
  ]);
  const [read] = JSON.parse(report("--json", `by-hand=${dir}`)).platforms;
  assert.equal(read.run, "(no run id)");
  assert.equal(read.breakdown.total.tests, 2);
  assert.equal(read.setAside, 1);
});

test("a label an object already has is a label like any other", () => {
  const linux = join(scratch, "linux");
  const out = JSON.parse(
    report(
      "--json",
      "--baseline",
      "__proto__",
      "--run",
      "__proto__=now",
      `__proto__=${linux}`,
      `constructor=${linux}`,
    ),
  );
  assert.deepEqual(
    out.platforms.map((p) => [p.label, p.run]),
    [
      ["__proto__", "now"],
      ["constructor", "now"],
    ],
  );
  assert.equal(out.ratios[0].label, "constructor");
});

test("each field a record must carry is checked, and the record that lacks it refused by name", () => {
  const cases = [
    [{ binary: "" }, /binary is not a name/],
    [{ test: 7 }, /test is not a name/],
    [{ started_at: "today" }, /started_at is not a nonnegative number/],
    [{ unattributed_s: -0.5 }, /unattributed_s is not a nonnegative number/],
    [{ run_id: 12 }, /run_id is not a string/],
    [{ attempt: 1.5 }, /attempt is not a count/],
    [
      { categories: { ...golden.categories, git: { ...golden.categories.git, count: -1 } } },
      /categories\.git\.count is not a count/,
    ],
    [
      { categories: { ...golden.categories, fs: { ...golden.categories.fs, s: "fast" } } },
      /categories\.fs\.s is not a nonnegative number/,
    ],
    [
      { categories: { ...golden.categories, wait: { ...golden.categories.wait, polls: null } } },
      /categories\.wait\.polls is not a count/,
    ],
    [{ categories: undefined }, /categories\.spawn\.count is not a count/],
    [{ ended: "crashed" }, /ended is not one of test-thread, process-exit, unfinished/],
    [{ wall_s: 0.5 }, /its categories hold .* of a 0\.5s test/],
    [{ unattributed_s: 0.5 }, /unattributed_s 0\.5 is not what its categories leave of wall_s/],
  ];
  cases.forEach(([change, message], i) => {
    const dir = join(scratch, `field-${i}`);
    mkdirSync(dir);
    writeFileSync(join(dir, "record.json"), JSON.stringify({ ...golden, ...change }));
    refused(["--out", join(scratch, "x"), `field=${dir}`], message, /next: remove /);
  });
  const array = join(scratch, "field-array");
  mkdirSync(array);
  writeFileSync(join(array, "a.json"), "[]");
  refused(["--out", join(scratch, "x"), `field=${array}`], /it is schema undefined/, /next:/);
});

test("--top-tests and --top-modules keep only that many, costliest first", () => {
  const out = JSON.parse(
    report(
      "--json",
      "--top-tests",
      "1",
      "--top-modules",
      "1",
      `windows=${join(scratch, "windows")}`,
    ),
  );
  const [windows] = out.platforms;
  assert.deepEqual(
    windows.breakdown.tests.map((t) => t.name),
    ["e2e::readjust::slow"],
  );
  assert.deepEqual(
    windows.breakdown.modules.map((m) => m.name),
    ["e2e::readjust"],
  );
  assert.equal(windows.breakdown.allModules.length, 3);
});

test("a test the runner ended is read from its last heartbeat and counted as unfinished", () => {
  const dir = join(scratch, "unfinished");
  write(dir, [
    { ...record("a::killed", { wall: 360 }), ended: "unfinished" },
    record("a::done", { wall: 1 }),
  ]);
  const [read] = JSON.parse(report("--json", `unfinished=${dir}`)).platforms;
  assert.equal(read.breakdown.total.tests, 2);
  assert.equal(read.breakdown.total.unfinished, 1);
  assert.match(report(`unfinished=${dir}`), /2 tests, 361\.0 test-s, 1 of them unfinished/);
});
