#!/usr/bin/env node
// Charge the check-visibility workload's telemetry and report it; this command runs no workload.
//
// `scripts/check-visibility-quota-budget.sh` runs this. The record is the e2e journey's: every
// `follow_checks` read the engine made (answered and charged by onevcs-testing's remote double)
// and every `release_holdup` query (answered by recorded GitHub responses, charged their own
// `rateLimit.cost`), each with the GraphQL points and REST requests it cost. The figure is their
// sum on the host's gh token.
import { readFileSync } from "node:fs";
import { report } from "@onebudgetspec/sdk";

const record = "target/budget-records/check-visibility-quota.json";
/** The workload the budget states, which a record must be the whole of to be charged. */
const WORKLOAD = {
  changes: 12,
  repositories: 3,
  followed_changes: 6,
  running_checks_per_followed_change: 3,
  running_minutes: 20,
  held_nodes: 2,
  held_hours: 3,
  scale: 1000,
};
/** Its two kinds of read, in the order the record holds them. */
const PHASES = ["follow", "holdup"];
let next =
  "run just test-e2e 'test(=check_visibility::the_check_visibility_workload_reads_at_its_cadence_and_records_what_it_spent)' " +
  "to record the workload, then run 'onebudgetspec check budgets.yaml --id check-visibility-github-quota'";

function fail(message) {
  throw new Error(`unreadable: ${message}`);
}

const count = (value, name) => {
  if (!Number.isSafeInteger(value) || value < 0) fail(`${name} must be a nonnegative integer`);
  return value;
};

function readRecord() {
  let text;
  try {
    text = readFileSync(record, "utf8");
  } catch (error) {
    throw new Error(error.code === "ENOENT" ? "missing" : `unreadable: ${error.message}`);
  }
  let held;
  try {
    held = JSON.parse(text);
  } catch (error) {
    fail(error.message);
  }
  if (held === null || typeof held !== "object" || held.schema_version !== 1) {
    fail("schema_version must be 1");
  }
  for (const [key, wanted] of Object.entries(WORKLOAD)) {
    if (held.workload?.[key] !== wanted) {
      fail(`workload.${key} must be ${wanted}: the record is not the workload the budget states`);
    }
  }
  if (Object.keys(held.workload).length !== Object.keys(WORKLOAD).length) {
    fail("workload names more than the budget states");
  }
  if (
    !Array.isArray(held.phases) ||
    held.phases.map((phase) => phase?.phase).join() !== PHASES.join()
  ) {
    fail(`phases must be ${PHASES.join(", ")}, in order`);
  }
  held.phases.forEach((phase, n) => {
    if (!Array.isArray(phase.calls)) fail(`phases[${n}].calls must be a list`);
    phase.calls.forEach((call, m) => {
      const at = `phases[${n}].calls[${m}]`;
      if (call === null || typeof call !== "object") fail(`${at} must be a call`);
      count(call.at_ms, `${at}.at_ms`);
      count(call.graphql_points, `${at}.graphql_points`);
      count(call.rest_requests, `${at}.rest_requests`);
    });
  });
  return held;
}

function measure(held) {
  const spent = (calls) =>
    calls.reduce((sum, call) => sum + call.graphql_points + call.rest_requests, 0);
  const points = (calls) => calls.reduce((sum, call) => sum + call.graphql_points, 0);
  const [follow, holdup] = held.phases.map((phase) => phase.calls);
  const value = spent(follow) + spent(holdup);
  const listings = holdup.filter((call) => call.query === "listing").length;
  const detail = [
    `${value} quota units for one run of 12 change requests across 3 repositories at scale 1/1000`,
    `follow reads: ${follow.length} calls, ${points(follow)} GraphQL points, ` +
      `${spent(follow) - points(follow)} REST requests`,
    `holdup reads: ${listings} reads in ${holdup.length} queries, ${points(holdup)} GraphQL ` +
      `points, ${spent(holdup) - points(holdup)} REST requests`,
  ].join(". ");
  return { value, detail };
}

function main() {
  try {
    const held = readRecord();
    next =
      "ensure ONEBUDGETSPEC_RESULT names a writable file, then rerun " +
      "'onebudgetspec check budgets.yaml --id check-visibility-github-quota'";
    const { value, detail } = measure(held);
    if (!report(value, detail)) {
      next =
        "invoke this command through 'onebudgetspec check budgets.yaml --id check-visibility-github-quota'";
      throw new Error("ONEBUDGETSPEC_RESULT is missing; invoke through onebudgetspec");
    }
  } catch (error) {
    console.error(`${record}: ${error.message}\nnext: ${next}`);
    process.exitCode = 1;
  }
}

main();
