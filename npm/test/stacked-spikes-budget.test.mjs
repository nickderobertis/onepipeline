// The stacked-spikes stage budget, measured the way the gate measures it: the
// committed `budgets.yaml` registration run by the real `onebudgetspec check`,
// from the `@onebudgetspec/cli` the pinned SDK installs, over a record written
// into a scratch workspace. Nothing launches an onepipeline run, and the real
// `target/budget-records` file is never touched, so this cannot race the e2e
// journey that writes it.

import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { copyFileSync, mkdirSync, mkdtempSync, rmSync, symlinkSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { afterEach, beforeEach, describe, it } from "node:test";
import { fileURLToPath } from "node:url";

const root = join(dirname(fileURLToPath(import.meta.url)), "..", "..");
const binary = join(root, "node_modules", ".bin", "onebudgetspec");
const record = "target/budget-records/stacked-spikes-stage.json";

let workspace;

// The registration is the committed one, and `scripts` links back to the tree so
// the command's import of the SDK resolves from this repository's lock.
beforeEach(() => {
  workspace = mkdtempSync(join(tmpdir(), "stacked-spikes-budget-"));
  copyFileSync(join(root, "budgets.yaml"), join(workspace, "budgets.yaml"));
  symlinkSync(join(root, "scripts"), join(workspace, "scripts"), "dir");
  mkdirSync(join(workspace, dirname(record)), { recursive: true });
});

afterEach(() => rmSync(workspace, { recursive: true, force: true }));

function check() {
  const output = spawnSync(binary, ["check", "--json", join(workspace, "budgets.yaml")], {
    encoding: "utf8",
  });
  const results = JSON.parse(output.stdout).results;
  assert.equal(results.length, 1, output.stdout);
  assert.equal(results[0].id, "stacked-spikes-stage-time");
  return results[0];
}

describe("the stacked-spikes stage budget", () => {
  it("reports the recorded wall clock in seconds, naming the run", () => {
    writeFileSync(
      join(workspace, record),
      JSON.stringify({ run_id: "stacked-stage", wall_ms: 11187 }),
    );
    const result = check();
    assert.equal(result.actual, 11.187);
    assert.equal(result.verdict, "within");
    assert.match(result.detail, /^run stacked-stage: run journal's first event to its last;/);
  });

  it("errors naming the record when it is missing", () => {
    const result = check();
    assert.equal(result.verdict, "error");
    assert.equal(result.actual, null);
    assert.match(result.error, new RegExp(`${record}: missing \\| next: run just test-e2e`));
  });

  for (const [what, body, reason] of [
    ["not JSON", "not json", "is not valid JSON"],
    [
      "JSON of the wrong shape",
      '{"wall_ms":1.5,"run_id":"r"}',
      "wall_ms must be a nonnegative safe integer",
    ],
    ["JSON without a run", '{"wall_ms":1000,"run_id":" "}', "run_id must be a nonempty string"],
  ]) {
    it(`errors naming the record as unreadable when it is ${what}`, () => {
      writeFileSync(join(workspace, record), body);
      const result = check();
      assert.equal(result.verdict, "error");
      assert.ok(result.error.includes(`${record}: unreadable: `), result.error);
      assert.ok(result.error.includes(reason), result.error);
    });
  }

  it("errors naming the record as unreadable when it cannot be read as a file", () => {
    mkdirSync(join(workspace, record));
    const result = check();
    assert.equal(result.verdict, "error");
    assert.match(result.error, new RegExp(`${record}: unreadable: EISDIR`));
  });

  it("refuses, naming the variable, when run outside a check", () => {
    writeFileSync(join(workspace, record), JSON.stringify({ run_id: "r", wall_ms: 1000 }));
    const env = { ...process.env };
    delete env.ONEBUDGETSPEC_RESULT;
    const output = spawnSync("node", ["scripts/stacked-spikes-budget.mjs"], {
      cwd: workspace,
      encoding: "utf8",
      env,
    });
    assert.equal(output.status, 1);
    assert.match(output.stderr, /ONEBUDGETSPEC_RESULT is missing; invoke through onebudgetspec/);
  });
});
