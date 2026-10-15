#!/usr/bin/env node
// Charge the busy review round's telemetry and report it; this command never runs a round.
//
// `scripts/review-round-quota-budget.sh` runs this. The record is the e2e journey's: per
// phase of the round, the remote double's ledger entries (each with the charge the double
// gives it), the `gh` calls the double did not answer (each charged by GitHub's published
// rule), the store reads the scripted `local-md` source recorded and the store tasks the
// write-back's projection records name. The figure is GraphQL points plus REST requests
// across both tokens: the host's `gh` token, and the plan store's board token, on which each
// task written costs 2 points and each read 1.
import { readFileSync } from "node:fs";
import { report } from "@onebudgetspec/sdk";

const record = "target/budget-records/review-round-quota.json";
/** What a board write costs, in GraphQL points: this host's metadata-only projections. */
const WRITE_POINTS = 2;
/** What a board read costs, by GitHub's published rule. */
const READ_POINTS = 1;
/** The typical round, stated beside the figure as context and compared with nothing. */
const TYPICAL =
  "typical round (one pass, 3 comments on one draft, one rework restacking the drafts above it, " +
  "3 replies) about 18 quota units by spike-quota-round-report's measured parts — 8 GraphQL " +
  "points + 4 REST requests on the gh token and about 6 on the board token — context, compared " +
  "with nothing";
/** Which category each double-answered method's call is, as the measure names them. */
const CATEGORY = {
  review_standings: "discovery and readjustment reads",
  change_standings: "discovery and readjustment reads",
  review_comments: "discovery and readjustment reads",
  retarget_change: "retargets and recoveries",
  reopen_change: "retargets and recoveries",
  reply_to_comment: "replies",
};
/** The busy round the budget states, which a record must be the whole of to be charged. */
const WORKLOAD = {
  drafts: 10,
  earlier_comments_per_draft: 30,
  new_comments: 6,
  passes: 2,
  reworks: 2,
  declines: 1,
  landed: 1,
  replies: 6,
};
/** Its steps, in the order the journey runs them. */
const PHASES = ["pass-1", "decline", "rework-b", "rework-h", "readjust", "pass-2"];
let next =
  "run just test-e2e 'test(=review_round::the_busy_review_round_ends_answered_and_true_and_records_what_it_spent)' " +
  "to record the round, then run 'onebudgetspec check budgets.yaml --id review-round-github-quota'";

function fail(message) {
  throw new Error(`unreadable: ${message}`);
}

const count = (value, name) => {
  if (!Number.isSafeInteger(value) || value < 0) fail(`${name} must be a nonnegative integer`);
  return value;
};

const words = (value, name) => {
  if (!Array.isArray(value) || value.some((word) => typeof word !== "string" || word === "")) {
    fail(`${name} must be a list of words`);
  }
  return value;
};

function charged(value, name) {
  if (value === null || typeof value !== "object") fail(`${name} must be a charged call`);
  count(value.graphql_points, `${name}.graphql_points`);
  count(value.rest_requests, `${name}.rest_requests`);
  return value;
}

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
      fail(`workload.${key} must be ${wanted}: the record is not the busy round the budget states`);
    }
  }
  if (Object.keys(held.workload).length !== Object.keys(WORKLOAD).length) {
    fail("workload names more than the busy round states");
  }
  if (
    !Array.isArray(held.phases) ||
    held.phases.map((phase) => phase?.phase).join() !== PHASES.join()
  ) {
    fail(`phases must be the round's steps, in order: ${PHASES.join(", ")}`);
  }
  held.phases.forEach((phase, n) => {
    const at = `phases[${n}]`;
    if (typeof phase?.phase !== "string" || phase.phase === "") fail(`${at}.phase must be named`);
    count(phase.wall_ms, `${at}.wall_ms`);
    for (const list of ["ledger", "gh"]) {
      if (!Array.isArray(phase[list])) fail(`${at}.${list} must be a list`);
      phase[list].forEach((call, m) => {
        charged(call, `${at}.${list}[${m}]`);
      });
    }
    phase.ledger.forEach((call, m) => {
      if (typeof call.method !== "string" || call.method === "") {
        fail(`${at}.ledger[${m}].method must name the double's method`);
      }
    });
    phase.gh.forEach((call, m) => {
      if (typeof call.endpoint !== "string" || call.endpoint === "") {
        fail(`${at}.gh[${m}].endpoint must name the call`);
      }
    });
    words(phase.store_reads, `${at}.store_reads`);
    words(phase.store_writes, `${at}.store_writes`);
    words(phase.dispatches, `${at}.dispatches`);
    if (phase.pass !== undefined) {
      for (const key of ["readjust_ms", "discovery_ms", "replies_ms"]) {
        count(phase.pass[key], `${at}.pass.${key}`);
      }
    }
  });
  const posted = held.phases
    .flatMap((phase) => phase.ledger)
    .filter((call) => call.method === "reply_to_comment").length;
  if (posted !== WORKLOAD.replies) {
    fail(`the ledger records ${posted} reply posts, not the round's ${WORKLOAD.replies}`);
  }
  return held;
}

function tally(calls, key) {
  const by = new Map();
  for (const call of calls) {
    const name = key(call);
    const held = by.get(name) ?? { calls: 0, graphql: 0, rest: 0 };
    held.calls += 1;
    held.graphql += call.graphql_points;
    held.rest += call.rest_requests;
    by.set(name, held);
  }
  return by;
}

function counted(list) {
  const by = new Map();
  for (const word of list) by.set(word, (by.get(word) ?? 0) + 1);
  return [...by].map(([word, n]) => `${word} ${n}`).join(", ") || "none";
}

const spent = (held) => `${held.graphql} GraphQL points + ${held.rest} REST requests`;

function measure(held) {
  const calls = held.phases.flatMap((phase) => [...phase.ledger, ...phase.gh]);
  const gh = tally(calls, () => "gh");
  const host = gh.get("gh") ?? { calls: 0, graphql: 0, rest: 0 };
  const reads = held.phases.flatMap((phase) => phase.store_reads);
  const writes = held.phases.flatMap((phase) => phase.store_writes);
  const board = writes.length * WRITE_POINTS + reads.length * READ_POINTS;
  const value = host.graphql + host.rest + board;
  const byPhase = held.phases.map((phase) => {
    const own = tally([...phase.ledger, ...phase.gh], () => "all").get("all") ?? {
      graphql: 0,
      rest: 0,
    };
    const boardOwn = phase.store_writes.length * WRITE_POINTS + phase.store_reads.length;
    const pass = phase.pass
      ? ` (readjustment ${phase.pass.readjust_ms} ms, discovery ${phase.pass.discovery_ms} ms, replies ${phase.pass.replies_ms} ms)`
      : "";
    return (
      `${phase.phase}: gh token ${spent(own)}, board token ${boardOwn} points ` +
      `(${phase.store_writes.length} written, ${phase.store_reads.length} read); ${phase.wall_ms} ms${pass}`
    );
  });
  const categories = tally(calls, (call) =>
    call.method
      ? (CATEGORY[call.method] ?? `other (${call.method})`)
      : "publications and other host calls",
  );
  const endpoints = tally(calls, (call) => call.method ?? call.endpoint);
  const detail = [
    `${value} quota units for the round of ${held.workload?.drafts ?? "?"} drafts`,
    `gh token: ${spent(host)} over ${host.calls} calls`,
    `board token: ${writes.length} store writes x ${WRITE_POINTS} + ${reads.length} store reads x ${READ_POINTS} = ${board} points`,
    `by phase: ${byPhase.join("; ")}`,
    `by kind: ${[...categories].map(([name, held]) => `${name} ${spent(held)}`).join("; ")}; ` +
      `store writes by kind: ${counted(writes)}; store reads: ${counted(reads)}`,
    `by endpoint: ${[...endpoints].map(([name, held]) => `${name} x${held.calls} = ${spent(held)}`).join("; ")}`,
    `agent dispatches by role (context): ${counted(held.phases.flatMap((phase) => phase.dispatches))}`,
    TYPICAL,
  ].join(". ");
  return { value, detail };
}

function main() {
  try {
    const held = readRecord();
    next =
      "ensure ONEBUDGETSPEC_RESULT names a writable file, then rerun " +
      "'onebudgetspec check budgets.yaml --id review-round-github-quota'";
    const { value, detail } = measure(held);
    if (!report(value, detail)) {
      next =
        "invoke this command through 'onebudgetspec check budgets.yaml --id review-round-github-quota'";
      throw new Error("ONEBUDGETSPEC_RESULT is missing; invoke through onebudgetspec");
    }
  } catch (error) {
    console.error(`${record}: ${error.message}\nnext: ${next}`);
    process.exitCode = 1;
  }
}

main();
