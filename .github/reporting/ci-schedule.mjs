#!/usr/bin/env node
// Whether a workflow's `budgets` job starts for one event and its predecessors' outcomes, by the
// interpreter `.github/ci-shape/` and the Mac budgets use over the job's real `needs` and `if:`.
//   node ci-schedule.mjs WORKFLOW_FILE SCENARIO_JSON   prints {"triggered", "selected", "needs"}
// Exit 0 answered; 1 the workflow has no `budgets` job; 2 unreadable or invalid input.
import { readFileSync } from "node:fs";
import { parse } from "yaml";
import { runGraph, triggered } from "../workflow-graph.mjs";

const [file, scenarioJson] = process.argv.slice(2);
if (!file || !scenarioJson) {
  console.error("usage: ci-schedule.mjs WORKFLOW_FILE SCENARIO_JSON");
  process.exit(2);
}
const isObject = (value) => value !== null && typeof value === "object" && !Array.isArray(value);
function refuse(reason) {
  console.error(`ci-schedule: ${reason}`);
  process.exit(2);
}
let workflow;
let scenario;
try {
  workflow = parse(readFileSync(file, "utf8"));
  scenario = JSON.parse(scenarioJson);
} catch (error) {
  refuse(`cannot read the workflow or the scenario: ${error.message}`);
}
if (!isObject(workflow) || !isObject(workflow.jobs)) refuse(`${file} is not a workflow with jobs`);
const OUTCOMES = ["success", "failure", "skipped", "cancelled"];
const pullRequest = scenario?.event?.pull_request;
const eventShaped =
  scenario?.eventName === "push"
    ? typeof scenario.event?.head_commit?.message === "string"
    : typeof pullRequest?.head?.ref === "string" &&
      typeof pullRequest?.head?.repo?.full_name === "string" &&
      typeof pullRequest?.base?.ref === "string";
const refusal =
  (!["pull_request", "push"].includes(scenario?.eventName) && "eventName") ||
  (typeof scenario.ref !== "string" && "ref") ||
  (typeof scenario.head !== "string" && "head") ||
  (!eventShaped && "event") ||
  (!isObject(scenario.results) && "results") ||
  (Object.entries(scenario.results).some(
    ([job, outcome]) => !Object.hasOwn(workflow.jobs, job) || !OUTCOMES.includes(outcome),
  ) &&
    `results: each names a job of the workflow and one of ${OUTCOMES.join(", ")}`);
if (refusal) refuse(`the scenario's ${refusal} is invalid`);
let started;
let rows;
let row;
try {
  started = triggered(workflow, scenario);
  rows = runGraph(workflow, scenario);
  row = rows.find((candidate) => candidate.job === "budgets");
} catch (error) {
  refuse(`cannot interpret ${file}: ${error.message}`);
}
if (!row) {
  console.error(`${file} has no budgets job`);
  process.exit(1);
}
const jobs = Object.fromEntries(rows.map((candidate) => [candidate.job, candidate.selected]));
console.log(JSON.stringify({ triggered: started, selected: row.selected, needs: row.needs, jobs }));
