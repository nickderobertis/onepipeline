// Measurement only (spike-launch-inventory, never lands): the launch inventory's fold, run
// as its command over a recorded job directory — two e2e journeys' launch trace, their
// test-profile records and harness waits, their strace record and a job log — and over
// copies of it changed the one way each table exists to notice.
import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { cpSync, mkdtempSync, readdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { after, before, test } from "node:test";
import { fileURLToPath } from "node:url";

const root = join(dirname(fileURLToPath(import.meta.url)), "..", "..");
const script = join(root, "scripts", "launch-inventory.mjs");
const recorded = join(root, "npm", "test", "golden", "launch-inventory");

let scratch;
before(() => {
  scratch = mkdtempSync(join(tmpdir(), "launch-inventory-"));
});
after(() => rmSync(scratch, { recursive: true, force: true }));

function run(...args) {
  return spawnSync(process.execPath, [script, ...args], { encoding: "utf8" });
}

/** The rows of the Markdown table under `heading`, each as its cells. */
function rows(out, heading) {
  const at = out.indexOf(`### ${heading}`);
  assert.notEqual(at, -1, `no section ${heading} in:\n${out}`);
  const lines = out.slice(at).split("\n").slice(1);
  const start = lines.findIndex((l) => l.startsWith("|"));
  const body = [];
  for (const line of lines.slice(start + 2)) {
    if (!line.startsWith("|")) break;
    body.push(line.slice(2, -2).split(" | "));
  }
  return body;
}

test("the fold prints every program the recorded journeys started, from the trace and the records", () => {
  const r = run("fold", `recorded=${recorded}`);
  assert.equal(r.status, 0, r.stderr);
  const out = r.stdout;
  assert.match(out, /^## recorded \(linux\)$/m);
  assert.match(out, /Runner: AMD EPYC 7763 64-Core Processor, 4 logical CPUs\./);
  // The e2e tier's summary is the run whose lines name an e2e test, not the first one.
  assert.match(
    out,
    /nextest `Summary \[ {2}60\.000s\] 2 tests run: 2 passed \(1 slow\), 1 skipped` = 1\.0 min/,
  );
  const programs = rows(out, "Programs started");
  const row = (from, by, program) =>
    programs.find((c) => c[0] === from && c[1] === by && c[2] === program);
  // The engine's own git reads, and the CLI it starts for a destination's rules.
  assert.equal(row("engine/libraries", "onepipeline (traced)", "git rev-parse")[3], "12");
  assert.equal(row("engine/libraries", "onepipeline (traced)", "onevcs")[3], "4");
  // A git start the onevcs CLI made inside that one, counted where it ran.
  assert.equal(row("engine/libraries", "onevcs (traced)", "git show-ref")[3], "2");
  // The engine's library, called in the test process itself.
  assert.equal(row("engine/libraries", "library, in the test process", "git rev-parse")[3], "9");
  // The tests' own starts, from their records.
  assert.equal(row("tests", "test process", "onepipeline")[3], "6");
  assert.equal(row("tests", "test process", "git")[3], "13");
  // None left unlisted: the rows sum to what the trace and the records hold.
  assert.match(
    out,
    /Rows account for 52 traced starts \(trace holds 52\) and 21 test-process starts \(records hold 21\)\./,
  );
});

test("a nested start's time is counted once, in the innermost program", () => {
  const out = run("fold", `recorded=${recorded}`).stdout;
  const sites = rows(out, "By call site and engine verb");
  const onevcs = sites.find((c) => c[0] === "onevcs" && c[2] === "plan");
  assert.ok(onevcs, out);
  // The onevcs CLI ran a median 43.1 ms, and its own six git starts are subtracted from it.
  assert.equal(onevcs[4], "43.1");
  const job = JSON.parse(
    spawnSync(
      process.execPath,
      [
        "-e",
        `import(${JSON.stringify(script)}).then((m) => {
      const f = m.foldJob(m.readJob("r", ${JSON.stringify(recorded)}));
      const s = f.starts.filter((s) => s.program === "onevcs");
      process.stdout.write(JSON.stringify(s.map((s) => [s.argv[0], s.dur, s.nested, s.self])));
    })`,
      ],
      { encoding: "utf8" },
    ).stdout,
  );
  assert.equal(job.length, 4);
  for (const [verb, dur, nested, self] of job) {
    // `rules check` reads the repository through git; `resolve` starts nothing.
    if (verb === "rules") assert.ok(nested > 0, "the rules check started git");
    else assert.equal(nested, 0);
    assert.equal(self, dur - nested);
  }
});

test("the fold names each wait on a child by site, and the harness's waits apart", () => {
  const out = run("fold", `recorded=${recorded}`).stdout;
  const waits = rows(out, "Waits and polls on a child, by site");
  const bounded = waits.find((c) => c[0].startsWith("onevcs::git::bounded"));
  assert.deepEqual(bounded.slice(1, 5), ["wait", "33", "65", "10.0"]);
  const harness = rows(out, "The e2e harness's waits, by site");
  assert.deepEqual(harness.find((c) => c[1] === "tests/e2e/harness.rs:4276").slice(0, 3), [
    "poll",
    "tests/e2e/harness.rs:4276",
    "4",
  ]);
});

test("every start the OS recorded is matched to the trace, the records, or the program that started it", () => {
  const out = run("fold", `recorded=${recorded}`).stdout;
  assert.match(
    out,
    /86 process starts observed; 51 matched a traced start \(the trace holds 52; the records hold 21 test-process starts\)\./,
  );
  const coverage = rows(out, "Coverage against the operating system's own record");
  assert.ok(!coverage.some((c) => c[0].startsWith("MISSED")), out);
  assert.ok(
    coverage.some((c) => c[0] === "uninstrumented descendant" && c[1] === "git" && c[2] === "sh"),
  );
  // The one traced start the OS never saw exec: a hook that could not start.
  assert.match(out, /has no exec for .*: commit-msg 1\./);
});

test("a start the OS saw and the trace lacks is reported as missed", () => {
  const dir = join(scratch, "missed");
  cpSync(recorded, dir, { recursive: true });
  const launch = join(dir, "test-profile", "launch");
  const file = readdirSync(launch).find(
    (f) => f.startsWith("destination__a_hook") && f.endsWith(".jsonl"),
  );
  const kept = readFileSync(join(launch, file), "utf8")
    .split("\n")
    .filter((l) => !(l.includes('"kind":"start"') && l.includes('"program":"onevcs"')));
  writeFileSync(join(launch, file), kept.join("\n"));
  const out = run("fold", `missed=${dir}`).stdout;
  const coverage = rows(out, "Coverage against the operating system's own record");
  const missed = coverage.find((c) => c[0] === "MISSED by the trace" && c[2] === "onevcs");
  assert.ok(missed, out);
  assert.equal(missed[1], "onepipeline");
});

test("a git read repeated in one process with no write between is counted, and a write resets it", () => {
  const dir = join(scratch, "repeats");
  cpSync(recorded, dir, { recursive: true });
  const launch = join(dir, "test-profile", "launch");
  const file = readdirSync(launch).find(
    (f) => f.startsWith("destination__a_hook") && f.endsWith(".jsonl"),
  );
  const lines = readFileSync(join(launch, file), "utf8").split("\n").filter(Boolean);
  const read = lines.find(
    (l) => l.includes('"sub":"rev-parse"') && l.includes('"--git-common-dir"'),
  );
  const again = (t, seq) => {
    const r = JSON.parse(read);
    return JSON.stringify({ ...r, seq, t: r.t + t, exit_ns: 5_000_000 });
  };
  const write = JSON.stringify({
    ...JSON.parse(read),
    seq: "onevcs:w",
    t: JSON.parse(read).t + 2e9,
    sub: "update-ref",
    argv: ["update-ref", "refs/x", "HEAD"],
    exit_ns: 1,
  });
  writeFileSync(
    join(launch, file),
    [...lines, again(1e9, "onevcs:r1"), write, again(3e9, "onevcs:r2")].join("\n"),
  );
  const out = run("fold", `repeats=${dir}`).stdout;
  assert.match(out, /^1 repeats, 0\.0 s\.$/m);
  const repeats = rows(out, "git invocations repeating an earlier one in the same CLI process");
  assert.deepEqual(
    repeats.map((c) => c.slice(1, 4)),
    [["git rev-parse", "onevcs::git::bounded (vendor/onevcs/src/git.rs:792)", "1"]],
  );
});

test("two jobs fold side by side, and diff names the per-test counts that differ", () => {
  const other = join(scratch, "other");
  cpSync(recorded, other, { recursive: true });
  const out = run("fold", `linux=${recorded}`, `again=${other}`).stdout;
  const side = rows(out, "The 20 tests starting the most on linux, beside the others");
  assert.deepEqual(side[0], [
    "destination::a_rules_check_this_build_cannot_read_a_policy_off_leaves_the_node_unchecked",
    "43",
    "43",
  ]);
  const diff = run("diff", `a=${recorded}`, `b=${other}`);
  assert.equal(diff.status, 0, diff.stderr);
  assert.match(diff.stdout, /^0 \(test, program\) pairs differ\.$/m);
});

test("an input that cannot be read is refused with what to do, and a usage error exits 2", () => {
  const missing = run("fold", `x=${join(scratch, "nowhere")}`);
  assert.equal(missing.status, 1);
  assert.match(
    missing.stderr,
    /launch-inventory: x: .*nowhere does not exist\nnext: unzip the job's test-profile artifact there\n/,
  );
  const empty = join(scratch, "empty");
  cpSync(join(recorded, "os"), join(empty, "os"), { recursive: true });
  const nothing = run("fold", `e=${empty}`);
  assert.equal(nothing.status, 1);
  assert.match(nothing.stderr, /holds no e2e test-profile record and no launch trace/);
  assert.equal(run("fold").status, 2);
  assert.equal(run("fold", "--top", "0", `r=${recorded}`).status, 2);
  assert.equal(run("rotate").status, 2);
});
