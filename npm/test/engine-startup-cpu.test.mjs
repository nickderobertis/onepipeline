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

test("sample fold validates corrupt tables and preserves multithread and unresolved accounting", async () => {
  const { mkdtempSync, readFileSync, rmSync, writeFileSync } = await import("node:fs");
  const { gunzipSync } = await import("node:zlib");
  const { join } = await import("node:path");
  const { tmpdir } = await import("node:os");
  const root = mkdtempSync(
    join(process.env.ONEPIPELINE_NODE_SCRATCH_DIR ?? tmpdir(), "startup-cpu-"),
  );
  const saved = JSON.parse(gunzipSync(readFileSync(profile)));
  const path = join(root, "profile.json");
  const run = (change) => {
    const copy = structuredClone(saved);
    change(copy);
    writeFileSync(path, JSON.stringify(copy));
    return spawnSync("python", [script, path, symbols], { encoding: "utf8" });
  };
  try {
    const doubled = run((p) => p.threads.push(structuredClone(p.threads[0])));
    assert.equal(doubled.status, 0, doubled.stderr);
    assert.match(doubled.stdout, /sampled CPU 530\.242 ms; lifetime 1765\.573 ms; threads 2/);
    const unresolved = run((p) => {
      delete p.threads[0].processShutdownTime;
      p.threads[0].resourceTable.lib.fill(null);
      p.threads[0].samples.stack.fill(null);
    });
    assert.equal(unresolved.status, 0, unresolved.stderr);
    assert.match(unresolved.stdout, /lifetime not measured/);
    assert.match(unresolved.stdout, /unresolved \(no stack\)/);
    for (const [change, message] of [
      [
        (p) => {
          p.threads[0].samples.length += 1;
        },
        /sample columns/,
      ],
      [
        (p) => {
          p.threads[0].samples.stack.pop();
        },
        /sample columns/,
      ],
      [
        (p) => {
          p.threads[0].processStartupTime = -1;
        },
        /finite nonnegative/,
      ],
      [
        (p) => {
          p.threads[0].processShutdownTime = 0;
        },
        /precedes/,
      ],
      [
        (p) => {
          p.meta.preprocessedProfileVersion = 48;
        },
        /pinned profiler/,
      ],
      [
        (p) => {
          p.meta.sampleUnits.threadCPUDelta = "ms";
        },
        /microseconds/,
      ],
      [
        (p) => {
          p.threads = [];
        },
        /no onepipeline process/,
      ],
      [
        (p) => {
          p.threads[0].samples.stack.fill(9999999);
        },
        /invalid table index/,
      ],
      [
        (p) => {
          p.threads[0].samples.stack.fill(0);
          p.threads[0].stackTable.prefix[0] = 0;
        },
        /cycle/,
      ],
      [
        (p) => {
          p.threads[0].samples.threadCPUDelta.fill(-1);
        },
        /finite nonnegative/,
      ],
    ]) {
      const bad = run(change);
      assert.equal(bad.status, 2, bad.stdout);
      assert.match(bad.stderr, message);
    }
    assert.equal(run(() => {}).status, 0);
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});
