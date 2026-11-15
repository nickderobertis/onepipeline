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
