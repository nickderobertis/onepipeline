import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { test } from "node:test";

const script = new URL("../../scripts/engine-startup-cpu.py", import.meta.url).pathname;
const profile = new URL("./golden/engine-startup/cpu.json.gz", import.meta.url).pathname;
const symbols = new URL("./golden/engine-startup/cpu.syms.json.gz", import.meta.url).pathname;

test("sample fold prints the captured main thread CPU and dominant module", () => {
  const run = spawnSync("python", [script, profile, symbols], { encoding: "utf8" });
  assert.equal(run.status, 0, run.stderr);
  assert.match(run.stdout, /sampled CPU 265\.121 ms; lifetime 1765\.573 ms; threads 1/);
  assert.match(run.stdout, /130\.685 ms \(49\.29%; cumulative 49\.29%\) privateterms::normalize/);
  assert.match(run.stdout, /self function:/);
  assert.match(run.stdout, /not disjointly measured here/);
});

test("sample fold refuses a non-profile with a repair and accepts the capture afterwards", () => {
  const bad = spawnSync("python", [script, symbols, symbols], { encoding: "utf8" });
  assert.equal(bad.status, 2);
  assert.match(bad.stderr, /supply a samply profile/);
  assert.equal(spawnSync("python", [script, profile, symbols]).status, 0);
});
