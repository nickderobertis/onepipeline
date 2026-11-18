// The fold's own rules over hand-made records, where the real run cannot arrange them on demand:
// how CPU is attributed when samples miss a process's end, the critical path's chain, the load
// line between samples, and the report shape REPORT_SCHEMA declares.
import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import {
  mkdirSync,
  mkdtempSync,
  readFileSync,
  rmSync,
  statSync,
  utimesSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";
import { loadOver, longestPath, REPORT_SCHEMA, taskCpu, validateReport } from "./fold.mjs";

const foldScript = join(dirname(fileURLToPath(import.meta.url)), "fold.mjs");

const task = (target, invocation = "i1") => ({ project: "p", target, hash: target, invocation });
const proc = (key, parent, owner, ticks, extra = {}) => ({
  key,
  pid: Number(key.split(":")[0]),
  parent,
  comm: "x",
  task: owner,
  first_tick: 0,
  last_tick: 5,
  self_ticks: ticks,
  children_ticks: 0,
  ...extra,
});

test("a task whose exit its parent saw is charged what the parent reaped, beyond its samples", () => {
  const cpu = taskCpu({
    processes: [proc("1:0", null, null, 50, { last_tick: 9 }), proc("2:0", "1:0", task("a"), 100)],
    reaps: [{ reaper: "1:0", tick: 6, ticks: 180, children: ["2:0"] }],
  });
  assert.deepEqual(cpu.get("i1|p:a"), { ticks: 180, basis: "reaped", delegated_ticks: 0 });
});

test("trees reaped in one interval split the jump by what each was seen to use", () => {
  const cpu = taskCpu({
    processes: [
      proc("1:0", null, null, 0, { last_tick: 9 }),
      proc("2:0", "1:0", task("a"), 100),
      proc("3:0", "1:0", task("b"), 300),
    ],
    reaps: [{ reaper: "1:0", tick: 6, ticks: 800, children: ["2:0", "3:0"] }],
  });
  assert.equal(cpu.get("i1|p:a").ticks, 200);
  assert.equal(cpu.get("i1|p:b").ticks, 600);
});

test("unseen exits fall back to samples, counting children still running at the last one", () => {
  const cpu = taskCpu({
    processes: [
      proc("1:0", null, null, 0, { last_tick: 9 }),
      proc("2:0", "1:0", task("a"), 10, { children_ticks: 5, last_tick: 3 }),
      proc("4:0", "2:0", null, 40, { last_tick: 4 }),
      proc("5:0", "2:0", null, 99, { last_tick: 2 }),
    ],
    reaps: [],
  });
  // 5:0 had exited by 2:0's last sample, so it is already in 2:0's reaped 5; 4:0 had not.
  assert.deepEqual(cpu.get("i1|p:a"), { ticks: 55, basis: "sampled", delegated_ticks: 0 });
});

test("a nested Nx run inside a task stays that task's work, whatever its own tasks are named", () => {
  const cpu = taskCpu({
    processes: [
      proc("1:0", null, null, 0),
      proc("2:0", "1:0", task("outer"), 10),
      proc("3:0", "2:0", task("inner", "nested"), 20),
    ],
    reaps: [],
  });
  assert.deepEqual([...cpu.keys()], ["i1|p:outer"]);
  assert.equal(cpu.get("i1|p:outer").ticks, 30);
});

test("a daemon's work for a task is its delegated CPU, apart from its own trees'", () => {
  const cpu = taskCpu({
    processes: [
      proc("1:0", null, null, 0, { last_tick: 9 }),
      proc("2:0", "1:0", task("a"), 30),
      // A compiler sccache's server started with the task's environment: no parent in the run.
      proc("7:0", null, task("a"), 400),
    ],
    reaps: [],
  });
  assert.deepEqual(cpu.get("i1|p:a"), { ticks: 30, basis: "sampled", delegated_ticks: 400 });
});

test("trees seen to use nothing split what their parent reaped evenly", () => {
  const cpu = taskCpu({
    processes: [
      proc("1:0", null, null, 0, { last_tick: 9 }),
      proc("2:0", "1:0", task("a"), 0),
      proc("3:0", "1:0", task("b"), 0),
    ],
    reaps: [{ reaper: "1:0", tick: 6, ticks: 10, children: ["2:0", "3:0"] }],
  });
  assert.equal(cpu.get("i1|p:a").ticks, 5);
  assert.equal(cpu.get("i1|p:b").ticks, 5);
});

test("processes whose parents loop, or one named twice, are refused rather than walked", () => {
  assert.throws(
    () =>
      taskCpu({
        processes: [proc("1:0", "2:0", null, 0), proc("2:0", "1:0", task("a"), 0)],
        reaps: [],
      }),
    /parents loop through/,
  );
  assert.throws(
    () =>
      taskCpu({ processes: [proc("1:0", null, null, 0), proc("1:0", null, null, 0)], reaps: [] }),
    /names a process twice/,
  );
});

test("the critical path follows the longest chain, through a replay that took no time", () => {
  const durations = new Map([
    ["build", 0],
    ["fast", 1],
    ["slow", 5],
    ["check", 0],
  ]);
  const path = longestPath(durations, {
    fast: ["build"],
    slow: ["build"],
    check: ["fast", "slow"],
  });
  assert.deepEqual(path, ["build", "slow", "check"]);
  assert.deepEqual(longestPath(new Map(), {}), []);
  assert.throws(
    () => longestPath(durations, { build: ["check"], check: ["slow"], slow: ["build"] }),
    /the task graph loops through/,
  );
});

test("load is the samples' mean inside an interval, and their line between samples", () => {
  const loads = [
    [0, 1],
    [1000, 3],
    [2000, 5],
  ];
  assert.equal(loadOver(loads, 0, 1000), 2);
  assert.equal(loadOver(loads, 1200, 1400), 3.6);
  assert.equal(loadOver(loads, 5000, 6000), 5);
  assert.equal(loadOver(loads, -500, -100), 1);
  assert.equal(loadOver([], 0, 1), 0);
});

test("REPORT_SCHEMA refuses a missing field, an unknown one and a wrongly typed one", () => {
  assert.equal(REPORT_SCHEMA.properties.schema_version.enum[0], 1);
  const errors = validateReport({
    schema_version: 1,
    recipe: 7,
    surprise: true,
  });
  assert.ok(errors.includes("$.recipe: expected string, got 7"), errors.join("\n"));
  assert.ok(errors.includes("$: unexpected surprise"));
  assert.ok(errors.includes("$: missing phases"));
  assert.deepEqual(
    validateReport([{ load: -1 }], {
      type: "array",
      items: { type: "object", properties: { load: { type: "number", minimum: 0 } } },
    }),
    ["$[0].load: -1 is below 0"],
  );
});

/** A run directory as a run leaves it, built by hand: one phase, one Nx task, two cargo builds. */
function handMadeRun(t) {
  const root = mkdtempSync(join(process.env.ONEPIPELINE_NODE_SCRATCH_DIR ?? tmpdir(), "fold-"));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const dir = join(root, "target", "gate-profile", "x");
  const put = (path, value) => {
    mkdirSync(dirname(path), { recursive: true });
    writeFileSync(path, typeof value === "string" ? value : `${JSON.stringify(value)}\n`);
  };
  const base = Math.floor(Date.now() / 1000) - 100;
  const cpu = { cpu_ms_before: [0, 0], cpu_ms_after: [500, 100] };
  put(join(dir, "meta.json"), { recipe: "r", root, commit: null, dirty: false, run: "r1" });
  put(join(dir, "run.json"), { recipe: "r", start: base, end: base + 10, ...cpu, exit: 0 });
  put(join(dir, "processes.json"), {
    clock_ticks_per_second: 100,
    interval_ms: 100,
    nproc: 4,
    loads: [
      [base * 1000, 1.5],
      [(base + 10) * 1000, 2.5],
    ],
    processes: [],
    reaps: [],
  });
  const phase = {
    id: "1-1",
    parent: "",
    recipe: "r",
    start: base,
    end: base + 10,
    ...cpu,
    exit: 0,
  };
  put(join(dir, "phases.jsonl"), `${JSON.stringify(phase)}\n`);
  put(join(dir, "nx", "i1.invocation.json"), { id: "i1", phase: "1-1", args: ["run-many"] });
  // An Nx run that failed before it wrote its task profile.
  put(join(dir, "nx", "i2.invocation.json"), { id: "i2", phase: "1-1", args: ["run", "x:y"] });
  put(join(dir, "nx", "i1.profile.json"), [
    { ph: "M", name: "thread_name" },
    { ph: "X", name: "p:x", ts: (base + 1) * 1e6, dur: 2e6, args: { status: "success" } },
  ]);
  const cargo = (start, end, target_dir) => ({
    start,
    end,
    ...cpu,
    exit: 0,
    argv: ["build"],
    cwd: root,
    target_dir,
    llvm_cov_target_dir: "",
    invocation: "i1",
    phase: "1-1",
    task: "p:x",
  });
  const elsewhere = join(root, "out", "of", "the", "way");
  put(
    join(dir, "cargo.jsonl"),
    `${JSON.stringify(cargo(base + 2, base + 5, elsewhere))}\n${JSON.stringify(cargo(base + 6, base + 7, ""))}\n`,
  );
  const page = (path, built, written) => {
    const start = new Date(built * 1000).toISOString().replace("Z", "123456Z");
    put(
      path,
      `<td>Build start:</td><td>${start}</td>\nconst UNIT_DATA = [\n  {"name": "u", "version": "1.0.0", "mode": "todo", "target": " (check)", "start": 0.1, "duration": 0.5}\n];\n`,
    );
    utimesSync(path, written, written);
  };
  // In the first build's own target directory, which no search of the root reaches.
  page(join(elsewhere, "cargo-timings", "cargo-timing-a.html"), base + 2.5, base + 4.9);
  // Written during the run by no recorded build, and one from before the run.
  page(join(root, "sub", "target", "cargo-timings", "cargo-timing-b.html"), base + 8.5, base + 8.9);
  page(join(root, "sub", "target", "cargo-timings", "cargo-timing-c.html"), base - 50, base - 49);
  return dir;
}

const fold = (dir) => spawnSync(process.execPath, [foldScript, dir], { encoding: "utf8" });

test("refolding a run writes its reports, saying what the records did not have", (t) => {
  const dir = handMadeRun(t);
  const result = fold(dir);
  assert.equal(result.status, 0, result.stderr);
  const report = JSON.parse(readFileSync(join(dir, "report.json"), "utf8"));
  assert.deepEqual(validateReport(report), []);
  assert.ok(readFileSync(join(dir, "report.md"), "utf8").includes("## Notes"));
  assert.deepEqual(
    report.cargo.map((c) => [c.timings, c.units.map((u) => u.unit)]),
    [
      [["out/of/the/way/cargo-timings/cargo-timing-a.html"], ["u v1.0.0 (check)"]],
      [[], []],
    ],
  );
  assert.equal(report.critical_path.complete, false);
  assert.deepEqual(
    report.critical_path.tasks.map((task) => task.task),
    ["p:x"],
  );
  assert.equal(report.tasks[0].cpu_basis, "none");
  assert.equal(report.notes.length, 4, report.notes.join("\n"));
  assert.match(report.notes.join("\n"), /No task graph for invocation i1/);
  assert.match(report.notes.join("\n"), /Nx wrote no task profile for invocation i2 \(run x:y\)/);
  assert.deepEqual(
    report.invocations.map((i) => [i.id, i.task_graph]),
    [
      ["i1", false],
      ["i2", false],
    ],
  );
  assert.match(report.notes.join("\n"), /cargo-timing-b\.html was written during the run by no/);
  assert.match(report.notes.join("\n"), /`cargo build` left no timings page/);
});

test("refolding refuses a timings page whose units are not as cargo writes them", (t) => {
  const dir = handMadeRun(t);
  const meta = JSON.parse(readFileSync(join(dir, "meta.json"), "utf8"));
  const page = join(meta.root, "out/of/the/way/cargo-timings/cargo-timing-a.html");
  const html = readFileSync(page, "utf8");
  const { mtime } = statSync(page);
  writeFileSync(page, html.replace('"duration": 0.5', '"duration": -1'));
  utimesSync(page, mtime, mtime);
  const result = fold(dir);
  assert.equal(result.status, 3);
  assert.match(result.stderr, /unit data is not as recorded: \$\[0\]\.duration: -1 is below 0/);
});

test("refolding refuses a missing or malformed record with exit 3, naming it", (t) => {
  const dir = handMadeRun(t);
  writeFileSync(join(dir, "phases.jsonl"), `${JSON.stringify({ id: 1, recipe: "r" })}\n`);
  const malformed = fold(dir);
  assert.equal(malformed.status, 3);
  assert.match(malformed.stderr, /phases\.jsonl line 1 .* is not as recorded: \$: missing parent/);
  rmSync(join(dir, "run.json"));
  const missing = fold(dir);
  assert.equal(missing.status, 3);
  assert.match(missing.stderr, /cannot read the whole run's record/);
  const usage = spawnSync(process.execPath, [foldScript], { encoding: "utf8" });
  assert.equal(usage.status, 2);
  assert.match(usage.stderr, /usage: fold\.mjs <run dir>/);
});

test("refolding refuses records that are not JSON, run backwards or loop, naming each", async (t) => {
  const cases = [
    ["meta.json", () => "{", /the run's meta\.json .* is not JSON/],
    ["phases.jsonl", () => "{not json\n", /phases\.jsonl line 1 .* is not JSON/],
    [
      "run.json",
      (base) =>
        JSON.stringify({
          recipe: "r",
          start: base,
          end: base - 1,
          cpu_ms_before: [0, 0],
          cpu_ms_after: [1, 1],
          exit: 0,
        }),
      /ends at .*, before it starts/,
    ],
    [
      "phases.jsonl",
      (base) =>
        ["a", "b"]
          .map((id) =>
            JSON.stringify({
              id,
              parent: id === "a" ? "b" : "a",
              recipe: id,
              start: base,
              end: base,
              cpu_ms_before: [0, 0],
              cpu_ms_after: [0, 0],
              exit: 0,
            }),
          )
          .join("\n"),
      /phases\.jsonl's parents loop through/,
    ],
  ];
  for (const [file, content, pattern] of cases)
    await t.test(`${file}: ${pattern}`, (t) => {
      const dir = handMadeRun(t);
      const base = JSON.parse(readFileSync(join(dir, "processes.json"), "utf8")).loads[0][0] / 1000;
      writeFileSync(join(dir, file), content(base));
      const result = fold(dir);
      assert.equal(result.status, 3);
      assert.match(result.stderr, pattern);
    });
});

test("refolding refuses a timings page that is not one cargo wrote", async (t) => {
  const cases = [
    [(html) => html.replace("Build start:", "Started:"), /has no build start or unit data/],
    [(html) => html.replace(/T(\d\d):/, "T99:"), /is not a date/],
    [(html) => html.replace('"name": "u"', '"name": u'), /unit data is not JSON/],
  ];
  for (const [rewrite, pattern] of cases)
    await t.test(String(pattern), (t) => {
      const dir = handMadeRun(t);
      const meta = JSON.parse(readFileSync(join(dir, "meta.json"), "utf8"));
      const page = join(meta.root, "out/of/the/way/cargo-timings/cargo-timing-a.html");
      const { mtime } = statSync(page);
      writeFileSync(page, rewrite(readFileSync(page, "utf8")));
      utimesSync(page, mtime, mtime);
      const result = fold(dir);
      assert.equal(result.status, 3, result.stderr);
      assert.match(result.stderr, pattern);
    });
});
