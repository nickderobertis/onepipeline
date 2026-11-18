// The fold's own rules over hand-made records, where the real run cannot arrange them on demand:
// how CPU is attributed when samples miss a process's end, the critical path's chain, the load
// line between samples, and the report shape REPORT_SCHEMA declares.
import assert from "node:assert/strict";
import test from "node:test";
import { loadOver, longestPath, REPORT_SCHEMA, taskCpu, validateReport } from "./fold.mjs";

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
