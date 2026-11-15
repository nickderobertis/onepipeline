// Measurement only (spike-launch-inventory, never lands): the launch inventory's fold, run
// as its command over a recorded job directory — two e2e journeys' launch trace, their
// test-profile records and harness waits, their strace record and a job log — and over
// copies of it changed the one way each table exists to notice.
import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import {
  chmodSync,
  copyFileSync,
  cpSync,
  existsSync,
  mkdtempSync,
  readdirSync,
  readFileSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { after, before, test } from "node:test";
import { fileURLToPath } from "node:url";

const root = join(dirname(fileURLToPath(import.meta.url)), "..", "..");
const script = join(root, "scripts", "launch-inventory.mjs");
const recorded = join(root, "npm", "test", "golden", "launch-inventory");
const runner = join(root, "scripts", "launch-os-trace.sh");

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
  // The e2e tier's summary is the largest nextest run, not the first one.
  assert.match(
    out,
    /largest nextest run \(the e2e tier in this repository's test jobs\) `Summary \[ {2}60\.000s\] 2 tests run: 2 passed \(1 slow\), 1 skipped` = 1\.0 min/,
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

test("the journeys re-launching the CLI for a read-only verb are listed with the verbs", () => {
  const out = run("fold", `recorded=${recorded}`).stdout;
  assert.deepEqual(rows(out, "Journeys re-launching the CLI for read-only verbs"), [
    [
      "destination::a_rules_check_this_build_cannot_read_a_policy_off_leaves_the_node_unchecked",
      "4",
      "plan×4",
    ],
    [
      "destination::a_hook_git_would_skip_and_a_hook_that_cannot_start_are_told_apart",
      "2",
      "plan×2",
    ],
  ]);
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

test("the run verb reads a job's checkout, selection, caches and e2e tier off its log", () => {
  const r = run("run", `linux=${join(recorded, "job.log")}`);
  assert.equal(r.status, 0, r.stderr);
  assert.match(
    r.stdout,
    /Checked out 02f6e2e, merging head c6deec83e950b8ae76ec1ab3ce7b3143cdaef1a4 into base 5aa41f09d1e1b3ad368428a6a2f7b2f09707cea2\./,
  );
  assert.match(
    r.stdout,
    /Nx selection: 3 tasks: onepipeline-e2e:test onepipeline-npm:test onepipeline:test-rest\./,
  );
  assert.match(
    r.stdout,
    /Nx remote cache: stored 0 entries \(0 bytes\), replayed 0\. Actions-cache saves in the log: 0\./,
  );
  assert.match(
    r.stdout,
    /Largest nextest run \(the e2e tier in this repository's test jobs\): summary 1\.0 min, 2 run, 2 passed; it started 25\.8 min into the job \(20\.0 min into the gate step/,
  );
  const caches = rows(`### x\n${r.stdout.slice(r.stdout.indexOf("| cache step"))}`, "x");
  assert.deepEqual(
    caches.map((c) => [c[0], c[1], c[2]]),
    [
      ["Swatinem/rust-cache@v2.9.2", "69.3", "partial (restore key)"],
      ["actions/setup-node@v4", "3.9", "no restore"],
      ["actions/cache/restore@v4", "11.8", "hit"],
      ["actions/cache/restore@v4", "0.1", "miss"],
    ],
  );
  const missing = run("run", `x=${join(scratch, "no.log")}`);
  assert.equal(missing.status, 1);
  assert.match(missing.stderr, /next: save the job's log there/);
});

test("ETW process starts fold to the same coverage as strace's", () => {
  const dir = join(scratch, "etw");
  cpSync(recorded, dir, { recursive: true });
  const os = join(dir, "os");
  const lines = [];
  for (const f of readdirSync(os)) {
    for (const line of readFileSync(join(os, f), "utf8").split("\n")) {
      // Each successful exec, with the process that cloned it: what ETW's ProcessStart names.
      const exec = /^(\d+) execve\("([^"]*)".*= 0$/.exec(line);
      if (exec) lines.push({ pid: Number(exec[1]), exe: exec[2] });
    }
  }
  const strace = spawnSync(
    process.execPath,
    [
      "-e",
      `import(${JSON.stringify(script)}).then((m) => {
    const s = m.readOs(${JSON.stringify(os)}).filter((e) => !e.reexec);
    process.stdout.write(JSON.stringify(s.map((e) => ({ pid: e.pid, ppid: e.ppid ?? 0, image: "C:\\\\x\\\\" + e.exe + ".exe" }))));
  })`,
    ],
    { encoding: "utf8" },
  );
  const starts = JSON.parse(strace.stdout);
  assert.ok(starts.length >= lines.length / 2);
  for (const f of readdirSync(os)) rmSync(join(os, f));
  writeFileSync(
    join(os, "etw-starts.jsonl"),
    `${starts.map((e) => JSON.stringify(e)).join("\n")}\nnot json\n{"pid":"x"}\n`,
  );
  const out = run("fold", `etw=${dir}`).stdout;
  assert.match(out, /86 process starts observed; 51 matched a traced start/);
  assert.match(out, /skipped 1 lines that are not JSON, 1 records of no known shape/);
});

test("a git argv is a read only in a form that writes nothing", () => {
  const reads = JSON.parse(
    spawnSync(
      process.execPath,
      [
        "-e",
        `import(${JSON.stringify(script)}).then((m) => {
      const cases = [
        ["rev-parse", ["rev-parse", "HEAD"]],
        ["config", ["config", "--get", "user.name"]],
        ["config", ["config", "user.name", "x"]],
        ["config", ["config", "--show-origin", "user.name", "x"]],
        ["branch", ["branch", "--show-current"]],
        ["branch", ["branch", "new-branch"]],
        ["branch", ["branch", "-l", "new-branch"]],
        ["branch", ["branch", "-D", "old"]],
        ["symbolic-ref", ["symbolic-ref", "HEAD"]],
        ["symbolic-ref", ["symbolic-ref", "HEAD", "refs/heads/x"]],
        ["symbolic-ref", ["symbolic-ref", "--delete", "HEAD"]],
        ["worktree", ["worktree", "list"]],
        ["worktree", ["worktree", "add", "x"]],
        ["remote", ["remote", "get-url", "origin"]],
        ["remote", ["remote", "add", "x", "y"]],
        ["tag", ["tag", "--list"]],
        ["tag", ["tag", "v1"]],
        ["stash", ["stash", "list"]],
        ["stash", ["stash", "push"]],
        ["hash-object", ["hash-object", "f"]],
        ["hash-object", ["hash-object", "-w", "f"]],
        ["update-ref", ["update-ref", "refs/x", "HEAD"]],
        ["diff", ["diff", "--output=patch.diff", "HEAD"]],
      ];
      process.stdout.write(JSON.stringify(cases.map(([sub, argv]) => m.gitReads(sub, argv))));
    })`,
      ],
      { encoding: "utf8" },
    ).stdout,
  );
  assert.deepEqual(reads, [
    true,
    true,
    false,
    false,
    true,
    false,
    false,
    false,
    true,
    false,
    false,
    true,
    false,
    true,
    false,
    true,
    false,
    true,
    false,
    true,
    false,
    false,
    false,
  ]);
});

test("host names this runner's processor and times each CLI's cheapest start", () => {
  const r = run("host", "--runs", "2");
  assert.equal(r.status, 0, r.stderr);
  const doc = JSON.parse(r.stdout);
  assert.equal(doc.platform, process.platform);
  assert.equal(typeof doc.cpu_model, "string");
  assert.ok(doc.cpu_model.length > 0);
  assert.ok(doc.cpus >= 1);
  // macOS names its processor through sysctl, with the physical core count beside it.
  if (process.platform === "darwin") {
    assert.deepEqual(doc.unanswered, []);
    assert.ok(doc.physical_cpus >= 1);
  }
  const node = doc.spawn.results.find((x) => x.name === "node");
  assert.equal(node.runs, 2);
  assert.ok(node.median_ms > 0);
  assert.equal(run("host", "--runs", "0").status, 2);
});

test("the OS trace runner passes a non-e2e binary through, and traces or refuses an e2e one", () => {
  const passed = spawnSync("bash", [runner, process.execPath, "-e", "process.exit(3)"], {
    encoding: "utf8",
  });
  assert.equal(passed.status, 3, passed.stderr);
  const dir = mkdtempSync(join(scratch, "runner-"));
  const e2e = join(dir, "e2e-0123456789abcdef");
  copyFileSync(process.execPath, e2e);
  chmodSync(e2e, 0o755);
  const out = join(dir, "os");
  const env = { ...process.env, ONEPIPELINE_OS_TRACE_DIR: out, NEXTEST_TEST_NAME: "m::t" };
  const ran = spawnSync(
    "bash",
    [runner, e2e, "-e", "require('node:child_process').spawnSync('git', ['--version'])"],
    { encoding: "utf8", env },
  );
  const strace = spawnSync("bash", ["-c", "command -v strace"]).status === 0;
  if (strace) {
    assert.equal(ran.status, 0, ran.stderr);
    const [file] = readdirSync(out);
    assert.match(file, /^m__t\.\d+\.strace$/);
    const starts = spawnSync(
      process.execPath,
      [
        "-e",
        `import(${JSON.stringify(script)}).then((m) => process.stdout.write(JSON.stringify(m.readOs(${JSON.stringify(out)}).map((e) => e.exe))))`,
      ],
      { encoding: "utf8" },
    );
    assert.ok(JSON.parse(starts.stdout).includes("git"), starts.stdout);
  } else {
    // Where nothing can record it, the e2e test is refused rather than run untraced.
    assert.equal(ran.status, 2);
    assert.match(ran.stderr, /strace is not installed/);
    assert.ok(!existsSync(out) || readdirSync(out).length === 0);
  }
});

/** Run a snippet against the fold's module, and parse what it prints as JSON. */
function withModule(body) {
  const r = spawnSync(
    process.execPath,
    ["-e", `import(${JSON.stringify(script)}).then((m) => { ${body} })`],
    {
      encoding: "utf8",
    },
  );
  assert.equal(r.status, 0, r.stderr);
  return JSON.parse(r.stdout);
}

test("every verb the built CLI lists is classified as a read or a write, and none it lacks", () => {
  const exe = join(
    root,
    "target",
    "debug",
    process.platform === "win32" ? "onepipeline.exe" : "onepipeline",
  );
  assert.ok(
    existsSync(exe),
    `${exe} is not built: run 'just _crate-build' (npm/project.json's test depends on it)`,
  );
  const help = spawnSync(exe, ["--help"], { encoding: "utf8" });
  assert.equal(help.status, 0, help.stderr);
  const listed = help.stdout
    .slice(help.stdout.indexOf("Commands:"), help.stdout.indexOf("Options:"))
    .split("\n")
    .slice(1)
    .map((l) => /^ {2}(\S+)/.exec(l)?.[1])
    .filter(Boolean);
  const { reads, writes } = withModule(
    "process.stdout.write(JSON.stringify({ reads: [...m.READ_VERBS], writes: [...m.WRITE_VERBS] }))",
  );
  assert.deepEqual([...reads, ...writes].sort(), [...listed].sort());
  assert.deepEqual(
    reads.filter((v) => writes.includes(v)),
    [],
  );
});

test("the repository's own test-profile golden passes the fold's record check", () => {
  const golden = join(root, "tests", "golden", "test-profile-record-v2.json");
  const ok = withModule(
    `process.stdout.write(JSON.stringify(m.validProfile(JSON.parse(require("node:fs").readFileSync(${JSON.stringify(golden)}, "utf8")))))`,
  );
  assert.equal(ok, true);
});

test("a recording flag makes a reading verb a write, so its re-launch is not counted as a read", () => {
  const got = withModule(
    'process.stdout.write(JSON.stringify([m.launchReads("supersessions", ["supersessions", "r"]), m.launchReads("supersessions", ["supersessions", "r", "--record"]), m.launchReads("start", ["start"])]))',
  );
  assert.deepEqual(got, [true, false, false]);
});

test("diff names each test and program whose start counts differ between two jobs", () => {
  const dir = join(scratch, "fewer");
  cpSync(recorded, dir, { recursive: true });
  const launch = join(dir, "test-profile", "launch");
  const file = readdirSync(launch).find(
    (f) => f.startsWith("destination__a_hook") && f.endsWith(".jsonl"),
  );
  const lines = readFileSync(join(launch, file), "utf8").split("\n");
  let dropped = 0;
  const kept = lines.filter((l) => {
    if (dropped < 2 && l.includes('"kind":"start"') && l.includes('"sub":"symbolic-ref"')) {
      dropped++;
      return false;
    }
    return true;
  });
  writeFileSync(join(launch, file), kept.join("\n"));
  const out = run("diff", `full=${recorded}`, `fewer=${dir}`).stdout;
  assert.match(
    out,
    /\| destination::a_hook_git_would_skip_and_a_hook_that_cannot_start_are_told_apart \| git symbolic-ref \| 4 \| 2 \| -2 \|/,
  );
  assert.match(out, /^1 \(test, program\) pairs differ\.$/m);
});

test("sleep tallies, a detached start's later exit, and inputs of no known shape are each accounted for", () => {
  const dir = join(scratch, "shapes");
  cpSync(recorded, dir, { recursive: true });
  const launch = join(dir, "test-profile", "launch");
  const file = readdirSync(launch).find(
    (f) => f.startsWith("destination__a_hook") && f.endsWith(".jsonl"),
  );
  const lines = readFileSync(join(launch, file), "utf8").split("\n").filter(Boolean);
  const engine = JSON.parse(
    lines.find((l) => l.includes('"kind":"proc"') && l.includes('"exe":"onepipeline"')),
  );
  const at = engine.t + 1e6;
  const added = [
    {
      v: 1,
      kind: "sleeps",
      pid: engine.pid,
      site: "src/engine.rs:3265",
      function: "onepipeline::engine::InPass::run",
      count: 3,
      slept_ns: 600e6,
      interval_ns: 200e6,
    },
    {
      v: 1,
      kind: "start",
      pid: engine.pid,
      site: "src/driver.rs:2436",
      function: "onepipeline::driver::retain_driver",
      program: "onepipeline",
      argv: ["drive-run", "x"],
      t: at,
      child: 4242,
      spawn_ns: 1e6,
      exit_ns: null,
    },
    {
      v: 1,
      kind: "exit",
      pid: engine.pid,
      child: 4242,
      site: "src/agentgraph.rs:1651",
      function: "w",
      t: at + 250e6,
      status: 0,
    },
    { v: 1, kind: "start", pid: engine.pid, site: "x", program: "git", argv: "not a list", t: at },
    { v: 1, kind: "end", pid: 12.5, t: at },
  ].map((r) => JSON.stringify(r));
  writeFileSync(join(launch, file), [...lines, ...added, "{cut short"].join("\n"));
  const waitsFile = readdirSync(launch).find(
    (f) => f.startsWith("destination__a_rules") && f.endsWith(".waits.json"),
  );
  const waitsDoc = JSON.parse(readFileSync(join(launch, waitsFile), "utf8"));
  waitsDoc.sites.push({ kind: "nap", site: "x", count: 1, s: 1 });
  writeFileSync(join(launch, waitsFile), JSON.stringify(waitsDoc));
  const e2e = join(dir, "test-profile", "e2e");
  writeFileSync(join(e2e, "old.json"), JSON.stringify({ schema: 1, test: "old", wall_s: 1 }));
  const out = run("fold", `shapes=${dir}`).stdout;
  assert.match(
    out,
    /skipped 1 lines that are not JSON, 2 records of no known shape, 1 test-profile records not of schema 2 and 1 harness wait entries; left out 0 trace and waits files/,
  );
  const waits = rows(out, "Waits and polls on a child, by site");
  assert.deepEqual(
    waits.find((c) => c[0].startsWith("onepipeline::engine::InPass::run")).slice(1, 6),
    ["sleep", "3", "-", "200.0", "0.6"],
  );
  const sites = rows(out, "By call site and engine verb");
  const driver = sites.find((c) => c[1].startsWith("onepipeline::driver::retain_driver"));
  // Its exit, recorded where the child was collected, gives the start its 250 ms.
  assert.equal(driver[4], "250.0");
});

test("the OS trace runner runs an e2e binary untraced when no trace is asked for, and refuses a directory it cannot make", () => {
  const dir = mkdtempSync(join(scratch, "runner-"));
  const e2e = join(dir, "e2e-0123456789abcdef");
  copyFileSync(process.execPath, e2e);
  chmodSync(e2e, 0o755);
  const { ONEPIPELINE_OS_TRACE_DIR: _, ...unset } = process.env;
  const plain = spawnSync("bash", [runner, e2e, "-e", "process.exit(4)"], {
    encoding: "utf8",
    env: unset,
  });
  assert.equal(plain.status, 4, plain.stderr);
  const file = join(dir, "a-file");
  writeFileSync(file, "");
  const refused = spawnSync("bash", [runner, e2e, "-e", "0"], {
    encoding: "utf8",
    env: { ...process.env, ONEPIPELINE_OS_TRACE_DIR: join(file, "os") },
  });
  assert.equal(refused.status, 2);
  const strace = spawnSync("bash", ["-c", "command -v strace"]).status === 0;
  assert.match(
    refused.stderr,
    strace
      ? /cannot create .* for the strace record: point ONEPIPELINE_OS_TRACE_DIR/
      : /strace is not installed/,
  );
});

test("a document that is not JSON, or a host document of no known shape, is refused naming the file", () => {
  const broken = join(scratch, "broken");
  cpSync(recorded, broken, { recursive: true });
  const host = join(broken, "test-profile", "launch-host.json");
  writeFileSync(host, "{ not json");
  const notJson = run("fold", `b=${broken}`);
  assert.equal(notJson.status, 1);
  assert.match(
    notJson.stderr,
    /launch-host\.json is not a JSON document: .*\nnext: re-download the job's test-profile artifact\n/,
  );
  writeFileSync(host, JSON.stringify({ cpus: "four", spawn: { results: [] } }));
  const shapeless = run("fold", `b=${broken}`);
  assert.equal(shapeless.status, 1);
  assert.match(
    shapeless.stderr,
    /launch-host\.json holds no spawn results\nnext: regenerate it with 'node scripts\/launch-inventory\.mjs host'\n/,
  );
  const empty = join(scratch, "empty.log");
  writeFileSync(empty, "no timestamps here\n");
  const log = run("run", `e=${empty}`);
  assert.equal(log.status, 1);
  assert.match(log.stderr, /the job log holds no timestamped line/);
  assert.equal(spawnSync("bash", [runner]).status, 2);
});

test("a repeat is the same argv, cwd and GIT_* values: the same argv under another GIT_* set is not one", () => {
  const dir = join(scratch, "envs");
  cpSync(recorded, dir, { recursive: true });
  const launch = join(dir, "test-profile", "launch");
  const file = readdirSync(launch).find(
    (f) => f.startsWith("destination__a_hook") && f.endsWith(".jsonl"),
  );
  const lines = readFileSync(join(launch, file), "utf8").split("\n").filter(Boolean);
  const read = JSON.parse(
    lines.find((l) => l.includes('"sub":"rev-parse"') && l.includes('"--git-common-dir"')),
  );
  const other = {
    v: 1,
    kind: "env",
    pid: read.pid,
    id: "onevcs:other",
    env: { GIT_DIR: "/elsewhere" },
  };
  const same = { ...read, seq: "onevcs:s", t: read.t + 1e9, exit_ns: 1 };
  const elsewhere = { ...read, seq: "onevcs:e", t: read.t + 2e9, exit_ns: 1, env: "onevcs:other" };
  writeFileSync(
    join(launch, file),
    [...lines, JSON.stringify(other), JSON.stringify(elsewhere), JSON.stringify(same)].join("\n"),
  );
  const out = run("fold", `envs=${dir}`).stdout;
  // The same-env read repeats; the one under GIT_DIR=/elsewhere does not.
  assert.match(out, /^1 repeats, 0\.0 s\.$/m);
});

test("a test process's start beyond what its record counts is told apart from the recorded ones", () => {
  const dir = join(scratch, "beyond");
  cpSync(recorded, dir, { recursive: true });
  const e2e = join(dir, "test-profile", "e2e");
  const file = readdirSync(e2e).find((f) => f.includes("a_hook"));
  const record = JSON.parse(readFileSync(join(e2e, file), "utf8"));
  record.programs.git.count -= 2;
  writeFileSync(join(e2e, file), JSON.stringify(record));
  const coverage = rows(
    run("fold", `beyond=${dir}`).stdout,
    "Coverage against the operating system's own record",
  );
  assert.deepEqual(
    coverage.find((c) => c[0] === "test process, beyond the records" && c[2] === "git"),
    ["test process, beyond the records", "e2e-3e9c9dc6bf2c36a6", "git", "2"],
  );
});

test("each program's per-start cost is its cheapest traced invocation's median, beside the runner's minimal start", () => {
  const out = run("fold", `recorded=${recorded}`).stdout;
  const cost = rows(out, "Per-start cost");
  const git = cost.find((c) => c[0] === "git");
  // The cheapest of git's invocations with three or more starts, and host's 30-start median.
  assert.deepEqual(git.slice(2, 5), ["6.9", "13.5", "52"]);
  assert.match(git[1], /^git show-ref @ onevcs::git::bounded/);
  // A program the trace never timed three times takes the runner's figure alone.
  assert.deepEqual(cost.find((c) => c[0] === "onepipeline").slice(1, 5), ["-", "-", "62.1", "6"]);
  assert.match(
    out,
    /^115 records from 10 processes: 0\.0 s building them and 0\.0 s writing them, \d+\.\d µs per record/m,
  );
});

test("folding jobs whose test sets differ names the tests not in every job", () => {
  const extra = join(scratch, "extra");
  cpSync(recorded, extra, { recursive: true });
  const e2e = join(extra, "test-profile", "e2e");
  const file = readdirSync(e2e)[0];
  const record = JSON.parse(readFileSync(join(e2e, file), "utf8"));
  writeFileSync(
    join(e2e, "extra.json"),
    JSON.stringify({ ...record, test: "surface::an_extra_journey" }),
  );
  const out = run("fold", `a=${recorded}`, `b=${extra}`).stdout;
  assert.match(
    out,
    /Test-name sets: a 2 \(\w+\), b 3 \(\w+\); 1 names not in every job: surface::an_extra_journey\./,
  );
});

test("a test name that is not a test path names no file of its own: the record is the runner's untested one", () => {
  const dir = mkdtempSync(join(scratch, "names-"));
  const e2e = join(dir, "e2e-0123456789abcdef");
  copyFileSync(process.execPath, e2e);
  chmodSync(e2e, 0o755);
  const out = join(dir, "os");
  const ran = spawnSync("bash", [runner, e2e, "-e", "0"], {
    encoding: "utf8",
    env: { ...process.env, ONEPIPELINE_OS_TRACE_DIR: out, NEXTEST_TEST_NAME: "../../escape" },
  });
  if (spawnSync("bash", ["-c", "command -v strace"]).status === 0) {
    assert.equal(ran.status, 0, ran.stderr);
    assert.deepEqual(
      readdirSync(out).map((f) => f.replace(/\.\d+\.strace$/, "")),
      ["untested"],
    );
    assert.ok(!existsSync(join(dir, "escape")));
  } else {
    assert.equal(ran.status, 2);
    assert.match(ran.stderr, /strace is not installed/);
  }
});

test("each program's per-start floor is set against the Linux job's, with the minutes the difference costs", () => {
  const win = join(scratch, "windows");
  cpSync(recorded, win, { recursive: true });
  const e2e = join(win, "test-profile", "e2e");
  for (const f of readdirSync(e2e)) {
    const record = JSON.parse(readFileSync(join(e2e, f), "utf8"));
    writeFileSync(join(e2e, f), JSON.stringify({ ...record, os: "windows" }));
  }
  const hostFile = join(win, "test-profile", "launch-host.json");
  const host = JSON.parse(readFileSync(hostFile, "utf8"));
  for (const r of host.spawn.results) if (r.name === "git") r.median_ms = 73.5;
  writeFileSync(hostFile, JSON.stringify(host));
  const out = run("fold", `linux=${recorded}`, `windows=${win}`).stdout;
  const rows_ = rows(out, "Per-start cost against linux");
  // git's floor is 73.5 ms against 13.5 ms: 60 ms over its 52 starts.
  assert.deepEqual(
    rows_.find((c) => c[0] === "windows" && c[1] === "git"),
    ["windows", "git", "73.5", "13.5", "60.0", "52", "0.05", "1.86"],
  );
});

test("trace and waits files of a binary other than e2e that compiles the harness are left out of the tier", () => {
  const dir = join(scratch, "others");
  cpSync(recorded, dir, { recursive: true });
  const launch = join(dir, "test-profile", "launch");
  const file = readdirSync(launch).find(
    (f) => f.startsWith("destination__a_hook") && f.endsWith(".jsonl"),
  );
  cpSync(join(launch, file), join(launch, "a_note_journey_of_the_note_binary.jsonl"));
  writeFileSync(
    join(launch, "a_note_journey_of_the_note_binary.waits.json"),
    JSON.stringify({ test: "a_note_journey_of_the_note_binary", sites: [] }),
  );
  const out = run("fold", `others=${dir}`).stdout;
  assert.match(out, /left out 2 trace and waits files of tests no e2e record names/);
  assert.match(out, /Rows account for 52 traced starts \(trace holds 52\)/);
});

test("an OS-observed image is named as the trace names a program: everything before its last dot", () => {
  const dir = mkdtempSync(join(scratch, "stems-"));
  writeFileSync(
    join(dir, "t.1.strace"),
    [
      "100 clone3({flags=CLONE_VM|CLONE_VFORK, exit_signal=SIGCHLD}, 88) = 101",
      '101 execve("/tmp/op-1/probe.sh", ["/tmp/op-1/probe.sh", "x"], 0x1 /* 3 vars */) = 0',
      "100 clone3({flags=CLONE_VM|CLONE_VFORK, exit_signal=SIGCHLD}, 88) = 102",
      '102 execve("/usr/bin/git", ["git", "status"], 0x1 /* 3 vars */) = 0',
    ].join("\n"),
  );
  const exes = withModule(
    `process.stdout.write(JSON.stringify(m.readOs(${JSON.stringify(dir)}).map((e) => [e.pid, e.ppid, e.exe])))`,
  );
  assert.deepEqual(exes, [
    [101, 100, "probe"],
    [102, 100, "git"],
  ]);
});

test("OS coverage joins reused PIDs by time and program, and never consumes a failed spawn", () => {
  const dir = join(scratch, "reused-pids");
  cpSync(recorded, dir, { recursive: true });
  const launch = join(dir, "test-profile", "launch");
  const source = readdirSync(launch).find((f) => f.endsWith(".jsonl"));
  const events = readFileSync(join(launch, source), "utf8").trim().split("\n").map(JSON.parse);
  const proc = events.find((e) => e.kind === "proc");
  const start = events.find((e) => e.kind === "start");
  const t = Date.parse("2026-11-14T00:00:00Z") * 1e6;
  for (const f of readdirSync(launch)) rmSync(join(launch, f));
  writeFileSync(
    join(launch, source),
    `${[
      { ...proc, pid: 100, exe: "onepipeline", verb: "plan", t },
      { ...start, pid: 100, seq: "onevcs:0", child: 200, t: t + 1e8 },
      { ...start, pid: 100, seq: "onevcs:1", child: null, spawn_ns: 10, t: t + 2e8 },
    ]
      .map((e) => JSON.stringify(e))
      .join("\n")}\n`,
  );
  const os = join(dir, "os");
  rmSync(os, { recursive: true });
  // Artifacts unpack OS records beside launch/ inside test-profile/.
  const artifactOs = join(dir, "test-profile", "os");
  cpSync(join(recorded, "os"), artifactOs, { recursive: true });
  for (const f of readdirSync(artifactOs)) rmSync(join(artifactOs, f));
  const iso = (ns) => new Date(ns / 1e6).toISOString();
  const starts = [
    { pid: 100, ppid: 1, image: "onepipeline.exe", t: iso(t) },
    // The child PID matches, but the program does not.
    { pid: 200, ppid: 100, image: "sh.exe", t: iso(t + 1e8) },
    // A failed git spawn must not absorb this OS-observed git start.
    { pid: 201, ppid: 100, image: "git.exe", t: iso(t + 2e8) },
    // PID 100 is now an uninstrumented shell, not the old engine.
    { pid: 100, ppid: 1, image: "sh.exe", t: iso(t + 10e9) },
    { pid: 202, ppid: 100, image: "git.exe", t: iso(t + 11e9) },
  ];
  writeFileSync(join(artifactOs, "starts.jsonl"), `${starts.map(JSON.stringify).join("\n")}\n`);
  const r = run("fold", `reused=${dir}`);
  assert.equal(r.status, 0, r.stderr);
  assert.match(r.stdout, /5 process starts observed; 0 matched a traced start/);
  const coverage = rows(r.stdout, "Coverage against the operating system's own record");
  assert.ok(
    coverage.some((c) => c[0] === "MISSED by the trace" && c[2] === "sh"),
    r.stdout,
  );
  assert.ok(
    coverage.some((c) => c[0] === "MISSED by the trace" && c[2] === "git"),
    r.stdout,
  );
  assert.ok(
    coverage.some((c) => c[0] === "uninstrumented descendant" && c[1] === "sh" && c[2] === "git"),
    r.stdout,
  );
  assert.match(r.stdout, /Traced starts the OS record has no exec for .*git 2/);
});

test("a fractional test-profile polling count is rejected rather than folded", () => {
  const dir = join(scratch, "fractional-polls");
  cpSync(recorded, dir, { recursive: true });
  const e2e = join(dir, "test-profile", "e2e");
  const file = join(e2e, readdirSync(e2e)[0]);
  const record = JSON.parse(readFileSync(file, "utf8"));
  record.categories.wait.polls = 1.5;
  writeFileSync(file, JSON.stringify(record));
  const r = run("fold", `invalid=${dir}`);
  assert.equal(r.status, 0, r.stderr);
  assert.match(r.stdout, /1 test-profile records not of schema 2/);
  // Recovery: an integral count remains a valid record.
  record.categories.wait.polls = 2;
  writeFileSync(file, JSON.stringify(record));
  assert.match(run("fold", `valid=${dir}`).stdout, /0 test-profile records not of schema 2/);
});

test("reusing every PID in a later invocation preserves both lifetimes and their own git reads", () => {
  const dir = join(scratch, "same-test-pid-reuse");
  cpSync(recorded, dir, { recursive: true });
  const launch = join(dir, "test-profile", "launch");
  for (const f of readdirSync(launch).filter((f) => f.endsWith(".jsonl"))) {
    const path = join(launch, f);
    const events = readFileSync(path, "utf8").trim().split("\n").map(JSON.parse);
    const later = events.map((e) => ({ ...e, ...(e.t == null ? {} : { t: e.t + 100e9 }) }));
    writeFileSync(path, `${[...events, ...later].map(JSON.stringify).join("\n")}\n`);
  }
  const original = run("fold", `original=${recorded}`);
  const repeated = run("fold", `repeated=${dir}`);
  assert.equal(repeated.status, 0, repeated.stderr);
  const section =
    "Engine runs the tests started, by verb (each process's own first-to-last record)";
  const before = rows(original.stdout, section);
  const after = rows(repeated.stdout, section);
  for (const row of before) {
    const twice = after.find((r) => r[0] === row[0]);
    assert.equal(Number(twice[1]), 2 * Number(row[1]), row[0]);
    assert.equal(twice[2], row[2], "PID reuse must preserve the invocation's median lifetime");
  }
  const repeats = (out) => Number(/(\d+) repeats,/.exec(out)[1]);
  assert.equal(repeats(repeated.stdout), 2 * repeats(original.stdout));
});

test("ETW matches a script's shell image and a slow spawn, while rejecting another parent", () => {
  const dir = join(scratch, "etw-images");
  cpSync(recorded, dir, { recursive: true });
  const os = join(dir, "os");
  for (const f of readdirSync(os)) rmSync(join(os, f));
  const launch = join(dir, "test-profile", "launch");
  const file = join(
    launch,
    readdirSync(launch).find((f) => f.endsWith(".jsonl")),
  );
  const events = readFileSync(file, "utf8").trim().split("\n").map(JSON.parse);
  const starts = events.filter((e) => e.kind === "start" && e.child != null).slice(0, 3);
  assert.equal(starts.length, 3);
  starts[1].spawn_ns = 2e9;
  writeFileSync(file, `${events.map(JSON.stringify).join("\n")}\n`);
  const rowsIn = starts.map((s, i) => ({
    pid: s.child,
    ppid: i === 2 ? 999999 : s.pid,
    image: i === 0 ? "cmd.exe" : `${s.program}.exe`,
    t: new Date((s.t + (i === 1 ? 1.5e9 : 0)) / 1e6).toISOString(),
  }));
  writeFileSync(join(os, "starts.jsonl"), `${rowsIn.map(JSON.stringify).join("\n")}\n`);
  const r = run("fold", `etw=${dir}`);
  assert.equal(r.status, 0, r.stderr);
  assert.match(r.stdout, /3 process starts observed; 2 matched a traced start/);
  assert.ok(
    rows(r.stdout, "Coverage against the operating system's own record").some(
      (r) => r[0] === "OS shell image of a traced script" && r[2] === "cmd" && r[3] === "1",
    ),
    r.stdout,
  );
});

test("trace overhead includes each linked copy's cumulative writer total", () => {
  const dir = join(scratch, "writer-copies");
  cpSync(recorded, dir, { recursive: true });
  const launch = join(dir, "test-profile", "launch");
  const file = join(
    launch,
    readdirSync(launch).find((f) => f.endsWith(".jsonl")),
  );
  const events = readFileSync(file, "utf8").trim().split("\n").map(JSON.parse);
  for (const e of events) e.write_ns_before = 0;
  const proc = events.find((e) => e.kind === "proc" && e.exe === "onepipeline");
  const end = events.find((e) => e.kind === "end" && e.pid === proc.pid);
  assert.ok(end);
  end.write_ns_before = 1e9;
  events.push({ ...proc, t: proc.t + 1 }, { ...end, t: end.t + 1, write_ns_before: 2e9 });
  writeFileSync(file, `${events.map(JSON.stringify).join("\n")}\n`);
  const r = run("fold", `writers=${dir}`);
  assert.equal(r.status, 0, r.stderr);
  assert.match(r.stdout, /3.0 s writing them/);
});

test("a child wait without its duration or poll count is rejected", () => {
  const dir = join(scratch, "incomplete-wait");
  cpSync(recorded, dir, { recursive: true });
  const launch = join(dir, "test-profile", "launch");
  const path = join(
    launch,
    readdirSync(launch).find((f) => f.endsWith(".jsonl")),
  );
  const original = readFileSync(path, "utf8");
  const proc = original
    .trim()
    .split("\n")
    .map(JSON.parse)
    .find((e) => e.kind === "proc");
  const wait = {
    v: 1,
    kind: "wait",
    pid: proc.pid,
    t: proc.t + 1,
    site: "x:1",
    function: "child_wait",
  };
  writeFileSync(path, `${original}\n${JSON.stringify(wait)}\n`);
  assert.match(run("fold", `incomplete=${dir}`).stdout, /1 records of no known shape/);
  wait.wait_ns = 10;
  wait.polls = 0;
  writeFileSync(path, `${original}\n${JSON.stringify(wait)}\n`);
  assert.match(run("fold", `complete=${dir}`).stdout, /0 records of no known shape/);
});
