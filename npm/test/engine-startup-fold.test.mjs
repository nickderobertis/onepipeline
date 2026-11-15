import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { test } from "node:test";

const script = new URL("../../scripts/engine-startup.mjs", import.meta.url);
const fixture = new URL("./golden/engine-startup/", import.meta.url);

test("fold prints measured start and adopt means from a recorded real journey", () => {
  const run = spawnSync(
    process.execPath,
    [script.pathname, "fold", `recorded=${fixture.pathname}`],
    { encoding: "utf8" },
  );
  assert.equal(run.status, 0, run.stderr);
  assert.match(run.stdout, /recorded \(linux\)/);
  assert.match(run.stdout, /e2e minutes: not measured/);
  assert.match(run.stdout, /start: 50\.479360 ms mean; 1 completed \/ 1 observed/);
  assert.match(run.stdout, /adopt: 62\.583040 ms mean; 1 completed \/ 1 observed/);
  assert.match(run.stdout, /completed launch-trace lifetime; excludes process creation/);
});

test("bad input is refused with a repair, then the recorded directory succeeds", () => {
  const bad = spawnSync(
    process.execPath,
    [script.pathname, "fold", "bad=/absent-engine-startup-fixture"],
    { encoding: "utf8" },
  );
  assert.equal(bad.status, 2);
  assert.match(bad.stderr, /unzip/);
  const good = spawnSync(
    process.execPath,
    [script.pathname, "fold", `recorded=${fixture.pathname}`],
    { encoding: "utf8" },
  );
  assert.equal(good.status, 0, good.stderr);
});

test("fold groups hosted sides and marks a zero difference undecided", () => {
  const job = new URL("./golden/launch-inventory/", import.meta.url).pathname;
  const run = spawnSync(
    process.execPath,
    [script.pathname, "fold", `before/1=${job}`, `contributors/1=${job}`],
    { encoding: "utf8" },
  );
  assert.equal(run.status, 0, run.stderr);
  assert.match(
    run.stdout,
    /before\/linux: e2e mean 1\.000000 min; range 1\.000000\.\.1\.000000; n=1/,
  );
  assert.match(run.stdout, /before-minus-after 0\.000000 min; undecided/);
  assert.match(
    run.stdout,
    /typical PR estimate \(27\/30\) 0\.000000 min \(undecided, not a saving\)/,
  );
  assert.match(run.stdout, /baseline test profiles missing \[\]/);
});

test("fold prices nonzero means against both observed spreads and reports missing profiles", async () => {
  const { cpSync, mkdtempSync, readFileSync, readdirSync, rmSync, unlinkSync, writeFileSync } =
    await import("node:fs");
  const { join } = await import("node:path");
  const { tmpdir } = await import("node:os");
  const root = mkdtempSync(
    join(process.env.ONEPIPELINE_NODE_SCRATCH_DIR ?? tmpdir(), "startup-fold-"),
  );
  try {
    const original = new URL("./golden/launch-inventory/", import.meta.url).pathname;
    const jobs = [];
    for (const [label, seconds] of [
      ["before/1", 60],
      ["before/2", 120],
      ["inside/1", 30],
      ["inside/2", 60],
      ["outside/1", 10],
      ["outside/2", 20],
    ]) {
      const dir = join(root, label);
      cpSync(original, dir, { recursive: true });
      const log = join(dir, "job.log");
      writeFileSync(
        log,
        readFileSync(log, "utf8").replace(
          /Summary \[\s*60\.000s\] 2 tests run/,
          `Summary [${seconds.toFixed(3)}s] 2 tests run`,
        ),
      );
      jobs.push(`${label}=${dir}`);
    }
    const missing = readdirSync(join(root, "outside/2/test-profile/e2e"))[0];
    unlinkSync(join(root, "outside/2/test-profile/e2e", missing));
    const run = spawnSync(process.execPath, [script.pathname, "fold", ...jobs], {
      encoding: "utf8",
    });
    assert.equal(run.status, 0, run.stderr);
    assert.match(
      run.stdout,
      /before\/linux: e2e mean 1\.500000 min; range 1\.000000\.\.2\.000000; n=2/,
    );
    assert.match(run.stdout, /inside\/linux: before-minus-after 0\.750000 min; undecided/);
    assert.match(
      run.stdout,
      /outside\/linux: before-minus-after 1\.250000 min; outside observed spreads; typical PR estimate \(27\/30\) 1\.125000 min/,
    );
    assert.match(run.stdout, /outside\/2: baseline test profiles missing \["destination::/);
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test("profiling command refuses an unrelated executable before attempting a capture", () => {
  const runner = new URL("../../scripts/engine-startup-profile.sh", import.meta.url).pathname;
  const run = spawnSync(
    "bash",
    [runner, process.execPath, process.env.ONEPIPELINE_NODE_SCRATCH_DIR ?? "."],
    { encoding: "utf8" },
  );
  assert.equal(run.status, 2, run.stderr);
  assert.match(run.stderr, /expected the compiled e2e-HASH binary/);
});

test("unfinished launches stay in observed counts and never become zero-duration samples", async () => {
  const { cpSync, mkdtempSync, readFileSync, readdirSync, rmSync, writeFileSync } = await import(
    "node:fs"
  );
  const { join } = await import("node:path");
  const { tmpdir } = await import("node:os");
  const root = mkdtempSync(
    join(process.env.ONEPIPELINE_NODE_SCRATCH_DIR ?? tmpdir(), "startup-incomplete-"),
  );
  try {
    cpSync(fixture.pathname, root, { recursive: true });
    const dir = join(root, "test-profile/launch");
    const file = join(
      dir,
      readdirSync(dir).find((name) => name.endsWith(".jsonl")),
    );
    const records = readFileSync(file, "utf8").trim().split("\n").map(JSON.parse);
    const start = records.find((r) => r.kind === "proc" && r.verb === "start");
    const partial = { ...start, pid: start.pid + 10000000 };
    writeFileSync(file, `${[...records, partial].map(JSON.stringify).join("\n")}\n`);
    const mixed = spawnSync(process.execPath, [script.pathname, "fold", `mixed=${root}`], {
      encoding: "utf8",
    });
    assert.equal(mixed.status, 0, mixed.stderr);
    assert.match(mixed.stdout, /start: 50\.479360 ms mean; 1 completed \/ 2 observed/);
    writeFileSync(
      file,
      `${records
        .filter((r) => !(r.kind === "end" && r.pid === start.pid))
        .map(JSON.stringify)
        .join("\n")}\n`,
    );
    const none = spawnSync(process.execPath, [script.pathname, "fold", `none=${root}`], {
      encoding: "utf8",
    });
    assert.equal(none.status, 0, none.stderr);
    assert.match(none.stdout, /start: not measured; 0 completed \/ 1 observed/);
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test("local phase accounting reports disjoint trace intervals and leaves CPU and filesystem uncertainty explicit", () => {
  const run = spawnSync(
    process.execPath,
    [script.pathname, "phases", `recorded=${fixture.pathname}`],
    { encoding: "utf8" },
  );
  assert.equal(run.status, 0, run.stderr);
  assert.match(run.stdout, /start pid .* trace window 50\.479 ms/);
  assert.match(run.stdout, /adopt pid .* trace window 62\.583 ms/);
  assert.match(run.stdout, /filesystem latency.*not separately measured/);
  assert.match(run.stdout, /sampled engine CPU overlaps/);
  for (const line of run.stdout.split("\n").filter((line) => line.startsWith("partition:"))) {
    const values = [...line.matchAll(/([\d.]+) ms/g)].map((m) => Number(m[1]));
    assert.equal(values.length, 4, line);
    assert.ok(Math.abs(values[0] + values[1] + values[2] - values[3]) <= 0.002, line);
  }
});

test("unknown commands and malformed operands are refused before a valid recorded fold recovers", () => {
  for (const args of [
    ["fold"],
    ["unknown", "x=a"],
    ["fold", "broken"],
    ["phases", "broken"],
    ["fold", "=a"],
    ["fold", "x="],
  ]) {
    const run = spawnSync(process.execPath, [script.pathname, ...args], { encoding: "utf8" });
    assert.equal(run.status, 2, run.stdout);
    assert.match(run.stderr, /use:|expected LABEL/);
  }
  assert.equal(
    spawnSync(process.execPath, [script.pathname, "fold", `recorded=${fixture.pathname}`]).status,
    0,
  );
});
