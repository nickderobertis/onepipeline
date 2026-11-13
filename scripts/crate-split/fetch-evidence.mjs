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
// nothing to GitHub. It exits 0 once everything is fetched, and 1
// with the reason on stderr when a read fails.
import { execFileSync } from "node:child_process";
import { existsSync, mkdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { join } from "node:path";

const JOBS = ["lint", "test-linux", "test-windows", "test-macos"];
const ROLLUP = `query($owner:String!,$name:String!,$n:Int!){repository(owner:$owner,name:$name){pullRequest(number:$n){number headRefOid commits(last:1){nodes{commit{oid checkSuites(first:20){nodes{workflowRun{databaseId runNumber event workflow{name} createdAt updatedAt} conclusion checkRuns(first:20){nodes{name databaseId conclusion startedAt completedAt}}}}}}}}}}`;

class Floor extends Error {}

function gh(args, binary = false) {
  return execFileSync("gh", args, {
    encoding: binary ? null : "utf8",
    maxBuffer: 1024 * 1024 * 1024,
    stdio: ["ignore", "pipe", "pipe"],
  });
}

function parseArgs(argv) {
  const options = {
    repo: "nickderobertis/onepipeline",
    prs: "30",
    floor: "2500",
    "graphql-floor": "2000",
  };
  for (let i = 0; i < argv.length; i += 2) {
    const name = argv[i]?.replace(/^--/, "");
    if (name === "wait") {
      options.wait = true;
      i--;
      continue;
    }
    if (
      !["out", "repo", "prs", "floor", "graphql-floor", "profiles"].includes(name) ||
      argv[i + 1] === undefined
    ) {
      throw new Error(`${argv[i]} is not an option this fetch reads`);
    }
    options[name] = argv[i + 1];
  }
  if (!options.out) throw new Error("--out names no directory");
  if (!/^[\w.-]+\/[\w.-]+$/.test(options.repo)) throw new Error("--repo is not OWNER/NAME");
  for (const name of ["prs", "floor", "graphql-floor"]) {
    if (!/^\d+$/.test(options[name])) throw new Error(`--${name} is not a whole number`);
  }
  return options;
}

/** The population: N merged non-release pull requests and the releases since. */
export function population(merged, count) {
  const sorted = [...merged].sort((a, b) => b.mergedAt.localeCompare(a.mergedAt));
  const release = (pr) => /^chore: release/.test(pr.title);
  const nonRelease = sorted.filter((pr) => !release(pr)).slice(0, count);
  const from = nonRelease.at(-1)?.mergedAt;
  return {
    window_from: from,
    non_release: nonRelease,
    release: sorted.filter((pr) => release(pr) && pr.mergedAt >= from),
  };
}

/** The CI run of a pull request's last commit, and its Nx jobs' check runs. */
export function ciRun(rollup) {
  const commit = rollup.data.repository.pullRequest.commits.nodes[0].commit;
  const suites = commit.checkSuites.nodes.filter((s) => s.workflowRun?.workflow?.name === "CI");
  if (suites.length === 0) return null;
  // The last CI suite of the commit is the run whose result the merge rested on.
  const suite = suites.sort((a, b) =>
    a.workflowRun.createdAt.localeCompare(b.workflowRun.createdAt),
  )[suites.length - 1];
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
 * name, so it is the one created while that check run was running.
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
      const { remaining, reset } = JSON.parse(
        gh(["api", "rate_limit", "--jq", `.resources.${resource} | {remaining, reset}`]),
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
  const write = (path, body) => {
    mkdirSync(join(path, ".."), { recursive: true });
    writeFileSync(path, body);
  };
  const [owner, name] = options.repo.split("/");
  const popPath = join(out, "population.json");
  if (!existsSync(popPath)) {
    guard("graphql");
    const merged = JSON.parse(
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
    );
    write(popPath, `${JSON.stringify(population(merged, Number(options.prs)), null, 1)}\n`);
  }
  const pop = JSON.parse(readFileSync(popPath, "utf8"));
  const prs = [...pop.non_release, ...pop.release];
  const unread = [];
  try {
    for (const pr of prs) {
      const rollupPath = join(out, "rollup", `${pr.number}.json`);
      if (!existsSync(rollupPath)) {
        guard("graphql");
        write(
          rollupPath,
          gh([
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
          ]),
        );
      }
      const run = ciRun(JSON.parse(readFileSync(rollupPath, "utf8")));
      if (!run) continue;
      const listPath = join(out, "artifacts", `${run.run}.json`);
      if (!existsSync(listPath)) {
        guard();
        write(
          listPath,
          gh(["api", `repos/${options.repo}/actions/runs/${run.run}/artifacts?per_page=100`]),
        );
      }
      const artifacts = JSON.parse(readFileSync(listPath, "utf8")).artifacts;
      for (const job of run.jobs.filter((j) => JOBS.includes(j.name))) {
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
        if (!existsSync(execPath)) {
          const artifact = artifactOf(artifacts, `nx-cache-entries-${job.name}`, job);
          if (!artifact || artifact.expired) {
            write(execPath, `${JSON.stringify({ absent: artifact ? "expired" : "none" })}\n`);
            continue;
          }
          guard();
          const zip = join(out, "executions", `${job.id}.zip`);
          write(
            zip,
            gh(["api", `repos/${options.repo}/actions/artifacts/${artifact.id}/zip`], true),
          );
          let body;
          try {
            body = execFileSync("unzip", ["-p", zip, "executions.json"], { encoding: "utf8" });
          } catch {
            body = `${JSON.stringify({ absent: "no executions.json in the artifact" })}\n`;
          }
          rmSync(zip);
          write(execPath, body);
        }
      }
    }
    if (options.profiles) {
      const listPath = join(out, "artifacts", `${options.profiles}.json`);
      if (!existsSync(listPath)) {
        guard();
        write(
          listPath,
          gh([
            "api",
            `repos/${options.repo}/actions/runs/${options.profiles}/artifacts?per_page=100`,
          ]),
        );
      }
      const artifacts = JSON.parse(readFileSync(listPath, "utf8")).artifacts;
      for (const job of ["test-linux", "test-windows", "test-macos"]) {
        const dir = join(out, "profiles", options.profiles, job);
        if (existsSync(dir)) continue;
        const artifact = artifacts
          .filter((a) => a.name === `test-profile-${job}` && !a.expired)
          .sort((a, b) => b.created_at.localeCompare(a.created_at))[0];
        if (!artifact) continue;
        guard();
        const zip = `${dir}.zip`;
        write(zip, gh(["api", `repos/${options.repo}/actions/artifacts/${artifact.id}/zip`], true));
        mkdirSync(dir, { recursive: true });
        execFileSync("unzip", ["-q", "-o", zip, "-d", dir]);
        rmSync(zip);
      }
    }
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

if (import.meta.url === `file://${process.argv[1]}`) {
  try {
    main(process.argv.slice(2));
  } catch (error) {
    console.error(`fetch-evidence: ${error.message.split("\n")[0]}`);
    if (error instanceof Floor) {
      // A floor met before the population was read: nothing is fetched yet.
      console.error("next: re-run the same command once the allowance resets, or add --wait");
      process.exitCode = 2;
    } else process.exitCode = 1;
  }
}
