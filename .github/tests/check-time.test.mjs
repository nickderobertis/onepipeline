// `onepipeline-ci-check-time` reports, after a release, the longest job of the pull requests
// it carried; `timeout-minutes` is what fails that job at pull-request time. Its threshold is
// set from this change's own measured hosted durations, so a test job whose timeout fell below
// it would be cut off before the budget it reports to could ever be reached.
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { test } from "node:test";
import { fileURLToPath } from "node:url";
import { parse } from "yaml";

const root = fileURLToPath(new URL("../../", import.meta.url));
const read = (path) => parse(readFileSync(join(root, path), "utf8"));

export function checkTimeProblems(budgets, ci) {
  const budget = budgets.budgets.find((entry) => entry.id === "onepipeline-ci-check-time");
  if (!budget) return ["budgets.yaml has no onepipeline-ci-check-time"];
  const problems = [];
  if (budget.unit !== "seconds") problems.push(`its unit is ${budget.unit}, not seconds`);
  if (ci.jobs?.lint?.["timeout-minutes"] !== 30) problems.push("lint is not bounded at 30 minutes");
  for (const id of ["test-linux", "test-windows", "test-macos"]) {
    const minutes = ci.jobs?.[id]?.["timeout-minutes"];
    if (!(minutes * 60 >= budget.threshold)) {
      problems.push(
        `${id}'s timeout of ${minutes} minutes is below the ${budget.threshold} s threshold`,
      );
    }
  }
  return problems;
}

test("lint is bounded at 30 minutes and each test job's timeout reaches onepipeline-ci-check-time's threshold", () => {
  assert.deepEqual(checkTimeProblems(read("budgets.yaml"), read(".github/workflows/ci.yml")), []);
});

test("a test job timed out below the threshold, or lint off 30 minutes, is named", () => {
  const budgets = read("budgets.yaml");
  const ci = read(".github/workflows/ci.yml");
  const threshold = budgets.budgets.find((b) => b.id === "onepipeline-ci-check-time").threshold;
  ci.jobs["test-macos"]["timeout-minutes"] = Math.floor(threshold / 60) - 1;
  ci.jobs.lint["timeout-minutes"] = 45;
  assert.deepEqual(checkTimeProblems(budgets, ci), [
    "lint is not bounded at 30 minutes",
    `test-macos's timeout of ${Math.floor(threshold / 60) - 1} minutes is below the ${threshold} s threshold`,
  ]);
});
