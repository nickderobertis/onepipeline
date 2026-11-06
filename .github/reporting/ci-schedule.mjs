#!/usr/bin/env node
// Whether a workflow's `budgets` job starts for one event and its predecessors' outcomes, by the
// interpreter `.github/ci-shape/` and the Mac budgets use over the job's real `needs` and `if:`.
//   node ci-schedule.mjs WORKFLOW_FILE SCENARIO_JSON   prints {"triggered", "selected", "needs"}
import { readFileSync } from "node:fs";
import { parse } from "yaml";
import { runGraph, triggered } from "../workflow-graph.mjs";

const [file, scenarioJson] = process.argv.slice(2);
if (!file || !scenarioJson) {
  console.error("usage: ci-schedule.mjs WORKFLOW_FILE SCENARIO_JSON");
  process.exit(2);
}
const workflow = parse(readFileSync(file, "utf8"));
const scenario = JSON.parse(scenarioJson);
const OUTCOMES = ["success", "failure", "skipped", "cancelled"];
const isObject = (value) => value !== null && typeof value === "object" && !Array.isArray(value);
const refusal =
  (!["pull_request", "push"].includes(scenario?.eventName) && "eventName") ||
  (typeof scenario.ref !== "string" && "ref") ||
  (typeof scenario.head !== "string" && "head") ||
  (!isObject(scenario.event) && "event") ||
  (!isObject(scenario.results) && "results") ||
  (Object.entries(scenario.results).some(
    ([job, outcome]) => !Object.hasOwn(workflow.jobs ?? {}, job) || !OUTCOMES.includes(outcome),
  ) &&
    "results: each names a job of the workflow and one of " + OUTCOMES.join(", "));
if (refusal) {
  console.error(`ci-schedule: the scenario's ${refusal} is invalid`);
  process.exit(2);
}
const row = runGraph(workflow, scenario).find((candidate) => candidate.job === "budgets");
if (!row) {
  console.error(`${file} has no budgets job`);
  process.exit(1);
}
console.log(
  JSON.stringify({
    triggered: triggered(workflow, scenario),
    selected: row.selected,
    needs: row.needs,
  }),
);
