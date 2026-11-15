// Measurement only: exercise the comparison command with the shared recorded trace.
import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { cpSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { test } from "node:test";

const root = resolve(import.meta.dirname, "../..");
const recorded = join(root, "npm/test/golden/launch-inventory");
const script = join(root, "scripts/git-reads-compare.mjs");
const names = [
  "destination::a_hook_git_would_skip_and_a_hook_that_cannot_start_are_told_apart",
  "destination::a_rules_check_this_build_cannot_read_a_policy_off_leaves_the_node_unchecked",
];
const run = (...args) => spawnSync(process.execPath, [script, ...args], { encoding: "utf8" });

function withSides(body) {
  const scratch = mkdtempSync(
    join(process.env.ONEPIPELINE_NODE_SCRATCH_DIR ?? tmpdir(), "compare-test-"),
  );
  try {
    const side = (label, seconds = [60], status = "PASS") => {
      for (const [i, duration] of seconds.entries()) {
        const dir = join(scratch, label, `attempt-${i}`, "test-linux");
        mkdirSync(dir, { recursive: true });
        cpSync(recorded, dir, { recursive: true });
        const log = readFileSync(join(dir, "job.log"), "utf8")
          .replace(
            /(^.*Summary \[ {2}60\.000s\].*$)/m,
            `${names.map((n) => `2026-11-14T09:00:59Z ${status} [ 1.000s] onepipeline::e2e ${n}`).join("\n")}\n$1`,
          )
          .replace("Summary [  60.000s]", `Summary [  ${duration.toFixed(3)}s]`);
        writeFileSync(join(dir, "job.log"), log);
      }
      return `${label}=${join(scratch, label)}`;
    };
    body(side);
  } finally {
    rmSync(scratch, { recursive: true, force: true });
  }
}

test("the command folds recorded launches, measured means, verbs and passed-name evidence", () => {
  withSides((side) => {
    const result = run("compare", side("before"), side("after"));
    assert.equal(result.status, 0, result.stderr);
    assert.match(result.stdout, /\| linux \| 1\/1 \| 1\.000 \| 1\.000 \| 0\.000 \| undecided/);
    assert.match(result.stdout, /\| rev-parse --git-path \| .* \| 9 \| 9 \|/);
    assert.match(result.stdout, /\| onepipeline plan \| 24 \| 24 \|/);
    assert.match(result.stdout, /all 2 passed/);
    for (const name of names) assert.ok(result.stdout.includes(name), name);
    assert.match(result.stdout, /workflow blob/);
  });
});

test("a missing or failed outcome is never counted as passed just because its profile exists", () => {
  withSides((side) => {
    const result = run("compare", side("before"), side("after", [60], "FAIL"));
    assert.equal(result.status, 0, result.stderr);
    assert.match(result.stdout, /2 not passed:/);
    const unknown = run("compare", side("before"), side("unknown", [60], "SLOW"));
    assert.equal(unknown.status, 0, unknown.stderr);
    assert.match(unknown.stdout, /2 not passed:/);
  });
});

test("a mean difference smaller than either side's spread stays undecided", () => {
  withSides((side) => {
    const result = run("compare", side("before", [6000, 6600]), side("after", [6540, 7140]));
    assert.equal(result.status, 0, result.stderr);
    assert.match(result.stdout, /\| -9\.000 \| undecided/);
  });
});

test("bad usage and absent artifact directories give an actionable refusal", () => {
  assert.equal(run("compare").status, 2);
  const result = run("compare", "before=/does-not-exist", "after=/does-not-exist");
  assert.equal(result.status, 1);
  assert.match(result.stderr, /does not exist\nnext:/);
});
