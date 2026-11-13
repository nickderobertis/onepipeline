#!/usr/bin/env node
// Fetch, read-only, what `crate-split-replay.mjs --observed` replays: the merged pull
// requests of a window and, for each one's last CI run, every Nx job's log and its
// `nx-cache-entries-<job>` executions record.
//
//   node scripts/crate-split/fetch-evidence.mjs --out DIR [--repo OWNER/NAME]
//        [--prs N] [--floor N] [--graphql-floor N] [--wait] [--profiles RUN_ID]
//
// The population is the N (default 30) most recently merged pull requests whose title
// is not release-plz's `chore: release`, and every release pull request merged since
// the earliest of them. Everything is written under DIR and nothing already there is
// fetched again, so a run stopped by the rate floor resumes where it stopped:
//
//   DIR/population.json            the pull requests, newest first
//   DIR/rollup/<pr>.json           the last commit's check suites (GraphQL)
//   DIR/artifacts/<run>.json       the run's artifact listing
//   DIR/logs/<job id>.log          each Nx job's log
//   DIR/executions/<job id>.json   that job's executions record, from the artifact of
//                                  the attempt the job's check run is
//   DIR/profiles/<run>/<job>/      with --profiles, that run's test-profile records
//
// Pull requests, their check suites and job ids are read over GraphQL; logs, listings
// and artifacts are REST core requests. Before each read the allowance it spends is
// read (`gh api rate_limit`, which spends neither): below --floor (default 2500) core or
// --graphql-floor (default 2000) GraphQL points remaining, it stops, says what is
// unread, and exits 2 — or with --wait sleeps until that allowance resets and goes on.
// The allowance is the host login's, shared with every other session on it. It writes
// nothing to GitHub, and every answer it saves is checked first (`validate.mjs`).
//
// Exit 0: everything is fetched; one line says how many reads it made. 2: a floor
// stopped it; what is unread and the `next:` action are on stderr, and a re-run resumes.
// 1: an argument, a read or an answer was refused, with its `next:` action on stderr.
import { execFileSync } from "node:child_process";
import { existsSync, mkdirSync, readFileSync, renameSync, rmSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { NX_JOBS } from "./observed.mjs";
import * as check from "./validate.mjs";

const ROLLUP = `query($owner:String!,$name:String!,$n:Int!){repository(owner:$owner,name:$name){pullRequest(number:$n){number headRefOid commits(last:1){nodes{commit{oid checkSuites(first:20){nodes{workflowRun{databaseId runNumber event workflow{name} createdAt updatedAt} conclusion checkRuns(first:20){nodes{name databaseId conclusion startedAt completedAt}}}}}}}}}}`;
const USAGE =
  "node scripts/crate-split/fetch-evidence.mjs --out DIR [--repo OWNER/NAME] [--prs N] [--floor N] [--graphql-floor N] [--wait] [--profiles RUN_ID]";

class Floor extends Error {}

function gh(args, binary = false) {
  try {
    return execFileSync("gh", args, {
      encoding: binary ? null : "utf8",
      maxBuffer: 1024 * 1024 * 1024,
      stdio: ["ignore", "pipe", "pipe"],
    });
  } catch (error) {
    const said = String(error.stderr ?? error.message)
      .trim()
      .split("\n")[0];
    throw new check.Refusal(
      `gh ${args.filter((a) => !a.startsWith("query=")).join(" ")} failed: ${said}`,
      "check `gh auth status` and the network, then re-run; what was fetched is kept",
    );
  }
}

const json = (text, what, next) => {
  try {
    return JSON.parse(text);
  } catch (error) {
    throw new check.Refusal(`${what} is not JSON: ${error.message}`, next);
  }
};

function parseArgs(argv) {
  const options = {
    repo: "nickderobertis/onepipeline",
    prs: "30",
    floor: "2500",
    "graphql-floor": "2000",
    ...check.parseOptions(argv, ["out", "repo", "prs", "floor", "graphql-floor", "profiles"], {
      flags: ["wait"],
      usage: USAGE,
    }),
  };
  check.need(options.out, "--out names no directory", USAGE);
  check.repository(options.repo);
  for (const name of ["prs", "floor", "graphql-floor"])
    check.wholeNumber(options[name], `--${name}`, USAGE);
  check.need(
    Number(options.prs) > 0,
    "--prs 0 asks for no pull request",
    "give --prs a count of at least 1",
  );
  if (options.profiles !== undefined) check.runId(options.profiles, "--profiles");
  return options;
}

/** The population: N merged non-release pull requests and the releases since. */
export function population(merged, count) {
  const sorted = [...check.mergedList(merged)].sort((a, b) => b.mergedAt.localeCompare(a.mergedAt));
  const release = (pr) => /^chore: release/.test(pr.title);
  const nonRelease = sorted.filter((pr) => !release(pr)).slice(0, count);
  const from = nonRelease.at(-1)?.mergedAt;
  return {
    window_from: from,
    non_release: nonRelease,
    release: sorted.filter((pr) => release(pr) && pr.mergedAt >= from),
  };
}

/**
 * The CI run of a pull request's last commit, and its Nx jobs' check runs: the newest
 * `CI` suite, the run whose result the merge rested on, or null when there is none.
 */
export function ciRun(rollup, pr) {
  const commit = check.rollup(rollup, pr);
  const suites = commit.checkSuites.nodes.filter((s) => s.workflowRun?.workflow?.name === "CI");
  if (suites.length === 0) return null;
  const suite = suites
    .sort((a, b) => a.workflowRun.createdAt.localeCompare(b.workflowRun.createdAt))
    .at(-1);
  return {
    run: suite.workflowRun.databaseId,
    head: commit.oid,
    conclusion: suite.conclusion,
    jobs: suite.checkRuns.nodes.map((r) => ({
      name: r.name,
      id: r.databaseId,
      conclusion: r.conclusion,
      started_at: r.startedAt,
      completed_at: r.completedAt,
    })),
  };
}

/**
 * The artifact a job's own attempt uploaded: a re-run attempt uploads one of the same
 * name, so it is the newest one created while that check run was running (or in the
 * ten minutes its upload step may run after it).
 */
export function artifactOf(artifacts, name, job) {
  const from = Date.parse(job.started_at);
  const to = Date.parse(job.completed_at) + 10 * 60 * 1000;
  const candidates = artifacts.filter(
    (a) => a.name === name && Date.parse(a.created_at) >= from && Date.parse(a.created_at) <= to,
  );
  return candidates.sort((a, b) => b.created_at.localeCompare(a.created_at))[0] ?? null;
}

function main(argv) {
  const options = parseArgs(argv);
  const out = options.out;
  const floors = { core: Number(options.floor), graphql: Number(options["graphql-floor"]) };
  const spent = { core: 0, graphql: 0 };
  // Read the allowance a read spends before making it; under its floor, pause until it
  // resets (--wait) or stop.
  const guard = (resource = "core") => {
    for (;;) {
      const { remaining, reset } = check.rateLimit(
        json(
          gh(["api", "rate_limit", "--jq", `.resources.${resource} | {remaining, reset}`]),
          "the rate limit",
          "run `gh api rate_limit`",
        ),
        resource,
      );
      if (remaining >= floors[resource]) break;
      const why = `${remaining} ${resource} remain, below the floor of ${floors[resource]}`;
      if (!options.wait) throw new Floor(why);
      const ms = Math.max(5, reset * 1000 - Date.now() + 30000);
      console.error(
        `fetch-evidence: ${why}; pausing until ${new Date(Date.now() + ms).toISOString()}`,
      );
      Atomics.wait(new Int32Array(new SharedArrayBuffer(4)), 0, 0, ms);
    }
    spent[resource]++;
  };
  // A file appears whole or not at all, so an interrupted run leaves nothing half-read.
  const write = (path, body) => {
    mkdirSync(join(path, ".."), { recursive: true });
    writeFileSync(`${path}.partial`, body);
    renameSync(`${path}.partial`, path);
  };
  const [owner, name] = options.repo.split("/");
  const popPath = join(out, "population.json");
  if (!existsSync(popPath)) {
    guard("graphql");
    const merged = json(
      gh([
        "pr",
        "list",
        "--repo",
        options.repo,
        "--state",
        "merged",
        "--limit",
        "100",
        "--json",
        "number,title,mergedAt,headRefName,headRefOid,mergeCommit",
      ]),
      "the merged pull request list",
      "re-run the same command",
    );
    write(popPath, `${JSON.stringify(population(merged, Number(options.prs)), null, 1)}\n`);
  }
  const pop = check.population(
    json(readFileSync(popPath, "utf8"), popPath, "remove it and re-run"),
  );
  const prs = [...pop.non_release, ...pop.release];
  const unread = [];
  try {
    for (const pr of prs) {
      const rollupPath = join(out, "rollup", `${pr.number}.json`);
      if (!existsSync(rollupPath)) {
        guard("graphql");
        const body = gh([
          "api",
          "graphql",
          "-F",
          `owner=${owner}`,
          "-F",
          `name=${name}`,
          "-F",
          `n=${pr.number}`,
          "-f",
          `query=${ROLLUP}`,
        ]);
        check.rollup(
          json(body, `the check suites of #${pr.number}`, "re-run the same command"),
          pr.number,
        );
        write(rollupPath, body);
      }
      const run = ciRun(
        json(readFileSync(rollupPath, "utf8"), rollupPath, "remove it and re-run"),
        pr.number,
      );
      if (!run) continue;
      const listPath = join(out, "artifacts", `${run.run}.json`);
      if (!existsSync(listPath)) {
        guard();
        const body = gh([
          "api",
          `repos/${options.repo}/actions/runs/${run.run}/artifacts?per_page=100`,
        ]);
        check.artifacts(
          json(body, `run ${run.run}'s artifacts`, "re-run the same command"),
          run.run,
        );
        write(listPath, body);
      }
      const artifacts = check.artifacts(
        json(readFileSync(listPath, "utf8"), listPath, "remove it and re-run"),
        run.run,
      );
      for (const job of run.jobs.filter((j) => NX_JOBS.includes(j.name))) {
        const logPath = join(out, "logs", `${job.id}.log`);
        if (!existsSync(logPath)) {
          guard();
          write(
            logPath,
            gh(
              [
                "api",
                "--allow-escape-sequences",
                `repos/${options.repo}/actions/jobs/${job.id}/logs`,
              ],
              true,
            ),
          );
        }
        const execPath = join(out, "executions", `${job.id}.json`);
        if (existsSync(execPath)) continue;
        const artifact = artifactOf(artifacts, `nx-cache-entries-${job.name}`, job);
        if (!artifact || artifact.expired) {
          write(execPath, `${JSON.stringify({ absent: artifact ? "expired" : "none" })}\n`);
          continue;
        }
        guard();
        const zip = join(out, "executions", `${job.id}.zip`);
        write(zip, gh(["api", `repos/${options.repo}/actions/artifacts/${artifact.id}/zip`], true));
        let body;
        try {
          body = execFileSync("unzip", ["-p", zip, "executions.json"], {
            encoding: "utf8",
            stdio: ["ignore", "pipe", "ignore"],
          });
          check.executions(
            json(body, `job ${job.id}'s executions.json`, "remove the zip and re-run"),
            `job ${job.id}'s executions.json`,
          );
        } catch (error) {
          if (error instanceof check.Refusal) throw error;
          body = `${JSON.stringify({ absent: "no executions.json in the artifact" })}\n`;
        }
        rmSync(zip);
        write(execPath, body);
      }
    }
    if (options.profiles) fetchProfiles(options, out, guard, write);
  } catch (error) {
    if (!(error instanceof Floor)) throw error;
    unread.push(error.message);
  }
  if (unread.length) {
    console.error(
      `fetch-evidence: stopped after ${spent.core} core and ${spent.graphql} GraphQL reads: ${unread[0]}`,
    );
    console.error(
      "next: re-run the same command once the allowance resets, or add --wait; it resumes",
    );
    process.exitCode = 2;
    return;
  }
  console.log(
    `fetch-evidence: ${prs.length} pull requests (${pop.non_release.length} non-release, ${pop.release.length} release) under ${out}, ${spent.core} core and ${spent.graphql} GraphQL reads`,
  );
}

/** The newest test-profile records of each test job of one run, unpacked once. */
function fetchProfiles(options, out, guard, write) {
  const listPath = join(out, "artifacts", `${options.profiles}.json`);
  if (!existsSync(listPath)) {
    guard();
    const body = gh([
      "api",
      `repos/${options.repo}/actions/runs/${options.profiles}/artifacts?per_page=100`,
    ]);
    check.artifacts(
      json(body, `run ${options.profiles}'s artifacts`, "re-run the same command"),
      options.profiles,
    );
    write(listPath, body);
  }
  const artifacts = check.artifacts(
    json(readFileSync(listPath, "utf8"), listPath, "remove it and re-run"),
    options.profiles,
  );
  for (const job of NX_JOBS.filter((j) => j.startsWith("test-"))) {
    const dir = join(out, "profiles", options.profiles, job);
    if (existsSync(dir)) continue;
    const artifact = artifacts
      .filter((a) => a.name === `test-profile-${job}` && !a.expired)
      .sort((a, b) => b.created_at.localeCompare(a.created_at))[0];
    if (!artifact) continue;
    guard();
    const zip = `${dir}.zip`;
    write(zip, gh(["api", `repos/${options.repo}/actions/artifacts/${artifact.id}/zip`], true));
    // Unpacked beside the directory and moved into place, so an interrupted unpack is
    // read again rather than taken as a whole one.
    const partial = `${dir}.partial`;
    rmSync(partial, { recursive: true, force: true });
    mkdirSync(partial, { recursive: true });
    try {
      execFileSync("unzip", ["-q", "-o", zip, "-d", partial], {
        stdio: ["ignore", "ignore", "pipe"],
      });
    } catch (error) {
      throw new check.Refusal(
        `the test-profile-${job} artifact did not unpack: ${String(error.stderr).trim()}`,
        "remove the zip beside it and re-run",
      );
    }
    renameSync(partial, dir);
    rmSync(zip);
  }
}

if (import.meta.url === `file://${process.argv[1]}`) {
  try {
    main(process.argv.slice(2));
  } catch (error) {
    console.error(`fetch-evidence: ${error.message.split("\n")[0]}`);
    if (error instanceof Floor) {
      // A floor met before the population was read: nothing is fetched yet.
      console.error("next: re-run the same command once the allowance resets, or add --wait");
      process.exitCode = 2;
    } else {
      console.error(`next: ${error instanceof check.Refusal ? error.next : USAGE}`);
      process.exitCode = 1;
    }
  }
}
