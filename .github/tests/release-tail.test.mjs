import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import {
  chmodSync,
  copyFileSync,
  mkdirSync,
  mkdtempSync,
  readdirSync,
  readFileSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { after, before, test } from "node:test";
import { fileURLToPath } from "node:url";
import { parse, stringify } from "yaml";

// The release tail, as `publish` runs it: the job's own install step and its last step, read
// from release.yml and run as written, with the real onevcs and onebudgetspec they install, the
// real jq and the copied figures script, over the root budgets.yaml. GitHub is the one boundary
// stood in for: `ONEVCS_GH` — onevcs's seam for replaying recorded responses — names a double
// serving the responses recorded, read-only, from this repository's release v0.97.1 and the runs
// that carried it (fixtures/release-tail/). Every figure is held to the one this test computes
// from those recordings' own job timestamps and counts, and every read count to what the double
// itself served.

const root = fileURLToPath(new URL("../../", import.meta.url));
// The onevcs release this repository adopts: the one carrying `onevcs host`'s read-only queries.
const ADOPTED_ONEVCS = "0.57.4";
const REPOSITORY = "nickderobertis/onepipeline";
const RELEASE = "v0.97.1";
// The Release run whose publish job runs the tail.
const RELEASE_RUN = 41192531055;
const TAIL_STEP = "onebudgetspec check budgets.yaml --label release-tail";
const JOB_TOKEN = `\${{ github.token }}`;
const TAIL_IDS = [
  "onepipeline-ci-check-time",
  "onepipeline-cd-job-time",
  "onepipeline-release-cycle-time",
  "onepipeline-ci-job-count",
  "onepipeline-cd-job-count",
  "onepipeline-release-read-requests",
  "onepipeline-pr-read-requests",
];

const budgetsFile = () => parse(readFileSync(join(root, "budgets.yaml"), "utf8"));
const TAIL_BUDGETS = () => budgetsFile().budgets.filter((budget) => TAIL_IDS.includes(budget.id));
const excludedBy = (command) =>
  command.flatMap((arg, n) => (command[n - 1] === "--exclude-workflow" ? [arg] : []));
// The CD and release-read figures leave out the main-push Rust cache warmer, which builds caches
// and delivers nothing to a release.
const WARMER = parse(
  readFileSync(join(root, ".github/workflows/rust-cache-warm.yml"), "utf8"),
).name;
const EXCLUDED = [WARMER];

const releaseWorkflow = () =>
  parse(readFileSync(join(root, ".github/workflows/release.yml"), "utf8"));
// The onebudgetspec release the justfile pins, which `just budgets` installs and the tail runs.
const PINNED_ONEBUDGETSPEC = readFileSync(join(root, "justfile"), "utf8").match(
  /^onebudgetspec-version := "([^"]+)"$/m,
)[1];

// Every way the tail could stop being a report nothing reads, or stop running the onevcs this
// repository adopted.
export function tailProblems(workflow) {
  const problems = [];
  const publish = workflow.jobs?.publish;
  if (!publish) return ["release.yml has no publish job"];
  const steps = publish.steps ?? [];
  const last = steps.at(-1);
  if (last?.run?.trim() !== TAIL_STEP) problems.push(`publish's last step is not \`${TAIL_STEP}\``);
  if (last?.["continue-on-error"] !== undefined)
    problems.push("the release tail carries continue-on-error");
  if (last?.env?.GH_TOKEN !== JOB_TOKEN)
    problems.push("the release tail is not handed the job's token as GH_TOKEN");
  const tailIndex = steps.findIndex((step) =>
    /onebudgetspec check [^\n]*--label release-tail/.test(step.run ?? ""),
  );
  if (tailIndex !== -1 && tailIndex !== steps.length - 1)
    problems.push("the release tail is not publish's last step");
  // Nothing reads the tail's outcome: no step after it, no job after publish, no expression
  // naming the step or the job.
  for (const [id, job] of Object.entries(workflow.jobs)) {
    const needs = typeof job.needs === "string" ? [job.needs] : (job.needs ?? []);
    if (needs.includes("publish"))
      problems.push(`${id} needs publish, so it reads the tail's outcome`);
  }
  const text = JSON.stringify(workflow);
  if (last?.id && new RegExp(`steps\\.${last.id}\\b`).test(text))
    problems.push("a step reads the tail's outcome");
  if (/needs\.publish\b/.test(text)) problems.push("an expression reads publish's result");
  const permissions = publish.permissions ?? {};
  for (const [scope, levels] of Object.entries({
    actions: ["read"],
    // publish also writes the Release's contents, and `write` grants the read the tail needs.
    contents: ["read", "write"],
    "pull-requests": ["read"],
  }))
    if (!levels.includes(permissions[scope]))
      problems.push(`publish does not grant ${scope}: read`);
  const install = steps.slice(0, -1).find((step) => /\bonevcs-cli@/.test(step.run ?? ""));
  if (!install) problems.push("no step of publish before its last installs the onevcs CLI");
  else {
    const version = install.run.match(/\bonevcs-cli@(\S+)/)[1];
    if (version !== ADOPTED_ONEVCS)
      problems.push(`publish installs onevcs-cli@${version}, not the adopted ${ADOPTED_ONEVCS}`);
    const cli = install.run.match(/@onebudgetspec\/cli@(\S+)/)?.[1];
    if (cli !== PINNED_ONEBUDGETSPEC)
      problems.push(`publish installs @onebudgetspec/cli@${cli}, not the justfile's pin`);
  }
  return problems;
}

test("publish ends in the release tail, reading-only and read by nothing", () => {
  assert.deepEqual(tailProblems(releaseWorkflow()), []);
});

test("a tail that moves, is read, may fail quietly or installs another onevcs is refused", () => {
  const cases = {
    "is not publish's last step": (w) => w.jobs.publish.steps.push({ run: "echo after" }),
    "needs publish": (w) => {
      w.jobs.after = { needs: ["publish"], "runs-on": "ubuntu-latest", steps: [{ run: "true" }] };
    },
    "a step reads the tail's outcome": (w) => {
      w.jobs.publish.steps.at(-1).id = "tail";
      w.jobs["build-linux"].steps.push({ if: "steps.tail.outcome == 'failure'", run: "true" });
    },
    "reads publish's result": (w) => {
      w.jobs["build-linux"].if = "needs.publish.result == 'failure'";
    },
    "carries continue-on-error": (w) => {
      w.jobs.publish.steps.at(-1)["continue-on-error"] = true;
    },
    "not handed the job's token": (w) => {
      delete w.jobs.publish.steps.at(-1).env;
    },
    "does not grant actions: read": (w) => {
      delete w.jobs.publish.permissions.actions;
    },
    "does not grant contents: read": (w) => {
      delete w.jobs.publish.permissions.contents;
    },
    "does not grant pull-requests: read": (w) => {
      w.jobs.publish.permissions["pull-requests"] = "write";
    },
    "installs the onevcs CLI": (w) => {
      w.jobs.publish.steps.splice(-2, 1);
    },
    "not the adopted": (w) => {
      const install = w.jobs.publish.steps.at(-2);
      install.run = install.run.replace(`onevcs-cli@${ADOPTED_ONEVCS}`, "onevcs-cli@0.56.2");
    },
    "not the justfile's pin": (w) => {
      const install = w.jobs.publish.steps.at(-2);
      install.run = install.run.replace(
        `@onebudgetspec/cli@${PINNED_ONEBUDGETSPEC}`,
        "@onebudgetspec/cli@0.1.3",
      );
    },
  };
  for (const [expected, change] of Object.entries(cases)) {
    const workflow = releaseWorkflow();
    change(workflow);
    const found = tailProblems(workflow);
    assert.ok(
      found.some((problem) => problem.includes(expected)),
      `${expected}: ${JSON.stringify(found)}`,
    );
  }
});

// A recording is exactly what `gh api --include` printed: the status line, the headers, each
// ending in CRLF, a blank line and the body.
function response(text) {
  const split = text.indexOf("\r\n\r\n");
  return {
    head: text.slice(0, split + 4),
    status: Number(text.slice(0, text.indexOf("\n")).split(" ")[1]),
    body: JSON.parse(text.slice(split + 4)),
  };
}
const pathOf = (recording) => recording.sidecar.path;

// What the double serves for a recording: the recording itself, byte for byte, but for the one
// change its sidecar names — the job running the tail is still in progress when the tail reads it.
function served(recording) {
  const change = recording.sidecar.served_in_progress;
  if (!change) return { text: recording.text, ...recording.response };
  const answer = structuredClone(recording.response.body);
  const jobs = answer.jobs.filter((job) => job.name === change.job);
  assert.equal(jobs.length, 1, `${recording.name} records one ${change.job} job`);
  Object.assign(jobs[0], { status: "in_progress", conclusion: null, completed_at: null });
  return {
    ...recording.response,
    text: recording.response.head + JSON.stringify(answer),
    body: answer,
  };
}

const ts = (stamp) => Date.parse(stamp.replace(/\.\d+Z$/, "Z")) / 1000;
const FIXTURES = new URL("./fixtures/release-tail/recordings/", import.meta.url);
const recordings = readdirSync(FIXTURES)
  .filter((name) => name.endsWith(".http"))
  .map((name) => {
    const text = readFileSync(new URL(name, FIXTURES), "utf8");
    return {
      name,
      text,
      response: response(text),
      sidecar: JSON.parse(readFileSync(new URL(`${name}.source.json`, FIXTURES), "utf8")),
    };
  });
const ok = recordings.filter((r) => r.response.status === 200);
const refusal = recordings.find((r) => r.response.status !== 200);
const byPath = new Map(ok.map((r) => [pathOf(r), r]));
const body = (path) => byPath.get(path).response.body;
const publishRecord = ok.find((r) => r.sidecar.served_in_progress);
const publishJob = publishRecord.response.body.jobs.find((job) => job.name === "publish");
const tailRunner = publishJob.runner_name;
// The pull requests the release carries: every commit the comparison lists, by the merged pull
// request the closed page names it the merge commit of.
const compare = ok.find((r) => pathOf(r).includes("/compare/")).response.body;
const pulls = ok.find((r) => pathOf(r).includes("/pulls?")).response.body;
const releaseCommit = compare.commits.at(-1).sha;
const pullRequests = compare.commits.map((commit) => {
  const pull = pulls.find((p) => p.merge_commit_sha === commit.sha);
  assert.ok(pull, `commit ${commit.sha} maps to a merged pull request`);
  return { number: pull.number, head: pull.head.sha, merged: ts(pull.merged_at) };
});
const releaseListing = `repos/${REPOSITORY}/actions/runs?per_page=100&head_sha=${releaseCommit}`;
const atRelease = () => body(releaseListing).workflow_runs;
// The runs the CD figures count: push and release runs at the release commit, but the excluded
// workflow's and a run that concluded skipped, which ran nothing.
const deliveryRuns = () =>
  atRelease().filter(
    (run) =>
      ["push", "release"].includes(run.event) &&
      !EXCLUDED.includes(run.name) &&
      run.conclusion !== "skipped",
  );

// `gh api --include` as onevcs's journeys write it: the response routed for the request asked,
// the request recorded, and a failure the way gh fails on an error status.
const DOUBLE = `#!/bin/sh
dir="$(dirname "$0")/.."
for arg in "$@"; do last="$arg"; done
request="\${last#https://api.github.com/}"
printf '%s\\n' "$request" >> "$dir/served"
n=$(awk -F '\\t' -v r="$request" '$1 == r { print $2; exit }' "$dir/routes.tsv")
if [ -z "$n" ]; then
  echo "replay: no recording for $request" >&2
  exit 1
fi
cat "$dir/responses/$n"
if [ -f "$dir/responses/$n.refusal" ]; then
  cat "$dir/responses/$n.refusal" >&2
  exit 1
fi
`;

const scratch = mkdtempSync(
  join(process.env.ONEPIPELINE_NODE_SCRATCH_DIR || tmpdir(), "release-tail-"),
);
after(() => rmSync(scratch, { recursive: true, force: true }));
let cases = 0;

function replay(serve) {
  const dir = join(scratch, `double-${cases++}`);
  mkdirSync(join(dir, "bin"), { recursive: true });
  mkdirSync(join(dir, "responses"));
  writeFileSync(join(dir, "bin", "gh"), DOUBLE);
  chmodSync(join(dir, "bin", "gh"), 0o755);
  const routes = serve.map((recording, n) => {
    const answer = served(recording);
    writeFileSync(join(dir, "responses", String(n)), answer.text);
    if (answer.status >= 400)
      writeFileSync(
        join(dir, "responses", `${n}.refusal`),
        `gh: ${answer.body.message ?? "error"} (HTTP ${answer.status})\n`,
      );
    return `${pathOf(recording)}\t${n}\n`;
  });
  writeFileSync(join(dir, "routes.tsv"), routes.join(""));
  return {
    program: join(dir, "bin", "gh"),
    served: () => {
      try {
        return readFileSync(join(dir, "served"), "utf8").split("\n").filter(Boolean);
      } catch {
        return [];
      }
    },
  };
}

// publish's own install step, run as written, puts the tools it installs on GITHUB_PATH.
const toolPath = [];
before(() => {
  const steps = releaseWorkflow().jobs.publish.steps;
  const install = steps.slice(0, -1).find((step) => /\bonevcs-cli@/.test(step.run ?? ""));
  const runnerTemp = join(scratch, "install-runner-temp");
  mkdirSync(runnerTemp);
  const githubPath = join(scratch, "github-path");
  writeFileSync(githubPath, "");
  const ran = spawnSync("bash", ["--noprofile", "--norc", "-eo", "pipefail", "-c", install.run], {
    cwd: root,
    env: { ...process.env, RUNNER_TEMP: runnerTemp, GITHUB_PATH: githubPath },
    encoding: "utf8",
  });
  assert.equal(ran.status, 0, `publish's install step failed: ${ran.stdout}${ran.stderr}`);
  toolPath.push(...readFileSync(githubPath, "utf8").split("\n").filter(Boolean));
  const env = { PATH: [...toolPath, process.env.PATH].join(":") };
  const onevcs = spawnSync("onevcs", ["--version"], { env, encoding: "utf8" });
  assert.equal(onevcs.stdout.trim(), `onevcs ${ADOPTED_ONEVCS}`);
  const checker = spawnSync("onebudgetspec", ["--version"], { env, encoding: "utf8" });
  assert.equal(checker.stdout.trim(), `onebudgetspec ${PINNED_ONEBUDGETSPEC}`);
});

// A checkout as publish's is: the root budgets.yaml (or a test copy of it) and the figures copy.
function checkout(budgets = readFileSync(join(root, "budgets.yaml"), "utf8")) {
  const dir = join(scratch, `checkout-${cases++}`);
  mkdirSync(join(dir, "scripts"), { recursive: true });
  writeFileSync(join(dir, "budgets.yaml"), budgets);
  copyFileSync(
    join(root, "scripts/actions-budget-figures.sh"),
    join(dir, "scripts/actions-budget-figures.sh"),
  );
  return dir;
}

// The last step of publish, run as written in a runner's environment for the release event and
// nothing of this process's own (no GITHUB_* of the CI running these tests leaks in).
function tail({ double, dir = checkout(), runnerTemp }) {
  const step = releaseWorkflow().jobs.publish.steps.at(-1);
  const event = join(scratch, `event-${cases++}.json`);
  writeFileSync(event, JSON.stringify({ action: "published", release: { tag_name: RELEASE } }));
  const home = join(scratch, `home-${cases++}`);
  mkdirSync(home);
  if (!runnerTemp) {
    runnerTemp = join(scratch, `runner-temp-${cases++}`);
    mkdirSync(runnerTemp);
  }
  const started = Math.floor(Date.now() / 1000);
  const ran = spawnSync("bash", ["--noprofile", "--norc", "-eo", "pipefail", "-c", step.run], {
    cwd: dir,
    env: {
      PATH: [...toolPath, process.env.PATH].join(":"),
      HOME: home,
      ONEVCS_HOME: join(home, "onevcs"),
      ONEVCS_GH: double.program,
      GH_TOKEN: step.env.GH_TOKEN === JOB_TOKEN ? "replay-token" : "",
      GITHUB_REPOSITORY: REPOSITORY,
      GITHUB_EVENT_PATH: event,
      GITHUB_RUN_ID: String(RELEASE_RUN),
      RUNNER_NAME: tailRunner,
      RUNNER_TEMP: runnerTemp,
    },
    encoding: "utf8",
    timeout: 600_000,
  });
  const ended = Math.ceil(Date.now() / 1000);
  const output = ran.stdout + ran.stderr;
  const results = Object.fromEntries(
    [
      ...output.matchAll(
        /^budget (\S+): (?:actual (\d+) \S+, budget (\d+) \S+, .* — (within|over)|(error) — .*?);/gm,
      ),
    ].map((m) => [
      m[1],
      { value: m[2] && Number(m[2]), threshold: m[3] && Number(m[3]), verdict: m[4] ?? m[5] },
    ]),
  );
  return { status: ran.status, output, results, started, ended, runnerTemp };
}

// A counted job is completed, not skipped and taken by a runner — GitHub records a job no runner
// took with a null or an empty runner_name — or the job running the tail, still in progress,
// timed to the tail's moment.
const counted = (now) => (job) => {
  if (job.status === "completed" && job.conclusion !== "skipped" && job.runner_name)
    return {
      ...job,
      secs: Math.max(ts(job.completed_at) - ts(job.started_at), 0),
      end: ts(job.completed_at),
    };
  if (job.run_id === RELEASE_RUN && job.status === "in_progress" && job.runner_name === tailRunner)
    return { ...job, secs: Math.max(now - ts(job.started_at), 0), end: now };
  return null;
};
const jobsPath = (runId) =>
  `repos/${REPOSITORY}/actions/runs/${runId}/jobs?per_page=100&filter=latest`;
const jobsOf = (runId) => served(byPath.get(jobsPath(runId))).body.jobs;
const runsAt = (listing, servedPaths) =>
  body(listing).workflow_runs.filter((run) => servedPaths.includes(jobsPath(run.id)));
const prListing = (pr) =>
  `repos/${REPOSITORY}/actions/runs?per_page=100&head_sha=${pr.head}&event=pull_request`;
const releaseOwn = [
  `repos/${REPOSITORY}/releases?per_page=100`,
  `repos/${REPOSITORY}/compare/`,
  `repos/${REPOSITORY}/pulls?`,
  releaseListing,
];

// Each figure as this test computes it from the recorded jobs' own timestamps and counts, and
// each read figure from the requests the double served.
function expected(servedPaths, now) {
  const prs = pullRequests.map((pr) => {
    const runs = runsAt(prListing(pr), servedPaths);
    const jobs = runs
      .flatMap((run) => jobsOf(run.id))
      .map(counted(now))
      .filter(Boolean);
    const requests = servedPaths.filter(
      (path) =>
        path === prListing(pr) ||
        runs.some((run) => path.startsWith(`repos/${REPOSITORY}/actions/runs/${run.id}/`)),
    );
    return { ...pr, runs, jobs, requests };
  });
  const cdRuns = deliveryRuns();
  const cdJobs = cdRuns
    .flatMap((run) => jobsOf(run.id))
    .map(counted(now))
    .filter(Boolean);
  const own = servedPaths.filter(
    (path) =>
      releaseOwn.some((prefix) => path === prefix || path.startsWith(prefix)) ||
      cdRuns.some((run) => path.startsWith(`repos/${REPOSITORY}/actions/runs/${run.id}/`)),
  );
  return {
    "onepipeline-ci-check-time": Math.max(...prs.flatMap((pr) => pr.jobs.map((job) => job.secs))),
    "onepipeline-cd-job-time": Math.max(...cdJobs.map((job) => job.secs)),
    "onepipeline-release-cycle-time": Math.max(
      ...prs.map(
        (pr) =>
          Math.max(...pr.jobs.map((job) => job.end)) -
          Math.min(...pr.runs.map((run) => ts(run.created_at))) +
          (now - pr.merged),
      ),
    ),
    "onepipeline-ci-job-count": Math.max(...prs.map((pr) => pr.jobs.length)),
    "onepipeline-cd-job-count": cdJobs.length,
    "onepipeline-release-read-requests": own.length,
    "onepipeline-pr-read-requests": Math.max(...prs.map((pr) => pr.requests.length)),
  };
}
// A figure timed to the tail's own moment lies between the one computed at the step's start and
// the one at its end; every other figure is exact.
function assertFigures(run, servedPaths) {
  const low = expected(servedPaths, run.started);
  const high = expected(servedPaths, run.ended);
  for (const id of TAIL_IDS) {
    const value = run.results[id]?.value;
    assert.ok(
      value >= Math.min(low[id], high[id]) && value <= Math.max(low[id], high[id]),
      `${id} reported ${value}, computed ${low[id]}..${high[id]}`,
    );
  }
  return low;
}

let recorded;
test(`over release ${RELEASE} the tail reports all seven budgets with their breakdowns`, () => {
  const double = replay(ok);
  const run = tail({ double });
  const servedPaths = double.served();
  recorded = { run, served: servedPaths };
  const budgets = budgetsFile().budgets;
  assert.deepEqual(Object.keys(run.results).sort(), [...TAIL_IDS].sort(), run.output);
  const figures = assertFigures(run, servedPaths);
  // The step's exit is onebudgetspec's: 1 when any reading is over, 0 when all are within.
  const over = TAIL_IDS.filter((id) => run.results[id].verdict === "over");
  for (const id of TAIL_IDS) {
    const threshold = budgets.find((budget) => budget.id === id).threshold;
    assert.equal(run.results[id].threshold, threshold, id);
    assert.equal(run.results[id].verdict, figures[id] > threshold ? "over" : "within", id);
  }
  assert.equal(run.status, over.length ? 1 : 0, run.output);
  for (const pr of pullRequests)
    assert.match(run.output, new RegExp(`#${pr.number}: checks \\d+s \\+ merge to release \\d+s`));
  assert.match(run.output, /Release \/ publish: \d+s/);
  assert.match(run.output, /longest per runner OS:/);
  assert.match(run.output, /per workflow:/);
  assert.match(run.output, /release-changes --tag v0\.97\.1: 3 requests/);
});

test("the CD and release-read budgets name the cache warmer's workflow, and only it, with --exclude-workflow", () => {
  const excluding = {
    "onepipeline-cd-job-time": EXCLUDED,
    "onepipeline-cd-job-count": EXCLUDED,
    "onepipeline-release-read-requests": EXCLUDED,
  };
  assert.deepEqual(
    TAIL_BUDGETS()
      .map((budget) => budget.id)
      .sort(),
    [...TAIL_IDS].sort(),
  );
  for (const budget of TAIL_BUDGETS())
    assert.deepEqual(excludedBy(budget.command), excluding[budget.id] ?? [], budget.id);
  assert.equal(WARMER, "Rust cache warm");
  // v0.97.1 predates the warmer, so its release commit carries no run the exclusion leaves out,
  // and every push or release run there is counted.
  assert.deepEqual(
    atRelease().filter((run) => run.name === WARMER),
    [],
  );
});

test("each onevcs read is made once across the seven figures", () => {
  const { served: servedPaths } = recorded;
  assert.equal(
    new Set(servedPaths).size,
    servedPaths.length,
    `a read was made twice: ${servedPaths.join("\n")}`,
  );
  assert.deepEqual(
    [...servedPaths].sort(),
    ok.map(pathOf).sort(),
    "every recording served, and nothing else",
  );
});

test("the read figures are the requests the double served", () => {
  const { run, served: servedPaths } = recorded;
  const figures = expected(servedPaths, run.started);
  assert.equal(
    run.results["onepipeline-release-read-requests"].value,
    figures["onepipeline-release-read-requests"],
  );
  assert.equal(
    run.results["onepipeline-pr-read-requests"].value,
    figures["onepipeline-pr-read-requests"],
  );
  // Every request falls to the release or to one pull request, so a read neither figure
  // counts cannot hide between them.
  assert.equal(
    figures["onepipeline-release-read-requests"] +
      pullRequests
        .map(
          (pr) =>
            servedPaths.filter(
              (path) =>
                path === prListing(pr) ||
                runsAt(prListing(pr), servedPaths).some((r) => path.includes(`/runs/${r.id}/`)),
            ).length,
        )
        .reduce((a, b) => a + b, 0),
    servedPaths.length,
  );
});

test("the selection is the release's: the pull requests' heads and the release commit's CD runs", () => {
  const { served: servedPaths } = recorded;
  for (const pr of pullRequests)
    assert.ok(servedPaths.includes(prListing(pr)), `#${pr.number}'s head was read`);
  for (const run of deliveryRuns())
    assert.ok(
      runsAt(releaseListing, servedPaths).some((r) => r.id === run.id),
      run.name,
    );
});

test("a threshold below a recorded figure turns the step's exit non-zero naming that budget", () => {
  const source = budgetsFile();
  const runnerTemp = recorded.run.runnerTemp;
  const copy = (lowered) => {
    const document = structuredClone(source);
    for (const budget of document.budgets)
      if (TAIL_IDS.includes(budget.id))
        budget.threshold = budget.id in lowered ? lowered[budget.id] : 10 ** 9;
    return stringify(document);
  };
  const within = tail({ double: replay(ok), dir: checkout(copy({})), runnerTemp });
  assert.equal(within.status, 0, within.output);
  const figures = expected(recorded.served, recorded.run.started);
  for (const id of TAIL_IDS) {
    const run = tail({
      double: replay(ok),
      dir: checkout(copy({ [id]: figures[id] - 1 })),
      runnerTemp,
    });
    assert.equal(run.status, 1, `${id}: ${run.output}`);
    assert.equal(run.results[id].verdict, "over", `${id}: ${run.output}`);
    for (const other of TAIL_IDS.filter((o) => o !== id))
      assert.equal(run.results[other].verdict, "within", other);
  }
});

test("an error status from GitHub fails the step and reports no figure", () => {
  const serve = [refusal, ...ok.filter((r) => pathOf(r) !== pathOf(refusal))];
  const run = tail({ double: replay(serve) });
  assert.notEqual(run.status, 0, run.output);
  assert.equal(run.status, 3, run.output);
  for (const id of TAIL_IDS)
    assert.equal(run.results[id]?.verdict, "error", `${id}: ${run.output}`);
  assert.match(run.output, /Bad credentials/);
});

test("every recording names its API path and where it was fetched from", () => {
  assert.ok(refusal, "an error status is recorded");
  for (const recording of recordings) {
    const { path, repository, fetched_from, recorded: how } = recording.sidecar;
    assert.ok(path.startsWith(`repos/${REPOSITORY}/`), recording.name);
    assert.equal(repository, REPOSITORY, recording.name);
    assert.equal(fetched_from.release, RELEASE, recording.name);
    assert.ok(
      how.read_only && how.with.startsWith(`gh api --include --method GET '${path}'`),
      recording.name,
    );
    assert.match(recording.response.head, /^HTTP\/2\.0 \d{3} /, recording.name);
    assert.ok(Object.keys(recording.response.body).length, recording.name);
  }
  assert.deepEqual(
    recordings.filter((r) => r.sidecar.served_in_progress).map((r) => r.name),
    [publishRecord.name],
    "one recording carries the one change",
  );
  assert.equal(publishRecord.sidecar.fetched_from.run, RELEASE_RUN);
});

// The copy is dero-skills' create-repo source, edited only there. create-repo's
// check_repo_baseline.py compares it with the source as it then stands; this refuses the one
// drift a copy can make on its own — an edit in place — by its own header, whose `sha256` line
// is the digest of every byte after it.
test("the figures script is the unedited create-repo copy its header names", () => {
  const text = readFileSync(join(root, "scripts/actions-budget-figures.sh"), "utf8");
  const lines = text.split("\n");
  assert.equal(
    lines[2],
    "# source: github.com/nickderobertis/dero-skills/skills/bootstrap/create-repo/assets/budgets/actions-budget-figures.sh",
  );
  assert.equal(lines[3], "# version: 10");
  const digest = lines[4].match(/^# sha256: ([0-9a-f]{64})$/)?.[1];
  const rest = lines.slice(5).join("\n");
  assert.equal(
    createHash("sha256").update(rest).digest("hex"),
    digest,
    "the copy was edited in place",
  );
  assert.equal(digest, "13ac7d79aaa3902dc7cfe87449283a05ba397fbc0612701e9b788ed59daf438d");
});

// The reconciliation script, against a stand-in for gh that answers each path with its
// recording, or with what GitHub would answer later: rewritten headers and timestamps alone,
// a changed body, or no answer at all.
test("the reconciliation names a recording GitHub no longer answers, and ignores what GitHub rewrites", () => {
  const dir = join(scratch, `reconcile-${cases++}`);
  mkdirSync(join(dir, "bin"), { recursive: true });
  const answer = join(dir, "answer.mjs");
  writeFileSync(
    answer,
    `import { readdirSync, readFileSync } from "node:fs";
// GitHub as it answers now: the committed recordings.
const dir = ${JSON.stringify(fileURLToPath(FIXTURES))};
const path = process.argv.at(-1);
// GitHub answers the releases path 401 under an invalid token, as the reconciliation asks it.
const invalid = (process.env.GH_TOKEN ?? "").startsWith("ghp_invalid");
const name = readdirSync(dir).find((n) => {
  if (!n.endsWith(".http.source.json")) return false;
  const sidecar = JSON.parse(readFileSync(dir + "/" + n, "utf8"));
  return sidecar.path === path && sidecar.recorded.with.endsWith("under an invalid token") === invalid;
});
if (!name || process.env.ANSWER === "none") process.exit(1);
let text = readFileSync(dir + "/" + name.replace(/\\.source\\.json$/, ""), "utf8");
if (process.env.ANSWER === "rewritten")
  text = text
    .replace(/^Date: .*$/m, "Date: Sun, 01 Nov 2026 00:00:00 GMT")
    .replace(/^X-Ratelimit-Remaining: .*$/im, "X-Ratelimit-Remaining: 1")
    .replace(/"updated_at":"[^"]*"/g, '"updated_at":"2026-11-01T00:00:00Z"');
// One recording drifting in a way the reconciliation must see: its body, its status, a header
// it keeps, or a field changing from a list to an object.
if (name.startsWith("runs-at-pr-1278-head")) {
  if (process.env.ANSWER === "changed") text = text.replace('"total_count":', '"total_count":1');
  if (process.env.ANSWER === "status") text = text.replace(/^HTTP\\/2\\.0 200 OK/, "HTTP/2.0 404 Not Found");
  if (process.env.ANSWER === "header") text = text.replace(/^Content-Type: .*$/im, "Content-Type: text/plain");
  if (process.env.ANSWER === "container") {
    const split = text.indexOf("\\r\\n\\r\\n") + 4;
    const body = JSON.parse(text.slice(split));
    body.workflow_runs = {};
    text = text.slice(0, split) + JSON.stringify(body);
  }
}
if (process.env.ANSWER === "no-separator") text = text.replaceAll("\\r\\n", "\\n");
if (process.env.ANSWER === "no-status") text = text.replace(/^HTTP\\/[^\\n]*/, "garbage");
process.stdout.write(text);
`,
  );
  writeFileSync(join(dir, "bin", "gh"), `#!/bin/sh\nexec "${process.execPath}" "${answer}" "$@"\n`);
  chmodSync(join(dir, "bin", "gh"), 0o755);
  const reconcile = (ANSWER) =>
    spawnSync(
      process.execPath,
      [fileURLToPath(new URL("./fixtures/release-tail/reconcile.mjs", import.meta.url))],
      {
        env: { ...process.env, PATH: `${join(dir, "bin")}:${process.env.PATH}`, ANSWER },
        encoding: "utf8",
        maxBuffer: 64 * 1024 * 1024,
      },
    );
  const same = reconcile("same");
  assert.equal(same.status, 0, same.stderr);
  const rewritten = reconcile("rewritten");
  assert.equal(rewritten.status, 0, rewritten.stderr);
  const changed = reconcile("changed");
  assert.equal(changed.status, 1, changed.stderr);
  assert.match(
    changed.stderr,
    /^runs-at-pr-1278-head\.http: GitHub now answers otherwise at body\.total_count$/m,
  );
  assert.match(changed.stderr, /1 recording\(s\) no longer match GitHub/);
  for (const [kind, at] of [
    ["status", "status 200 is now 404"],
    ["header", "header content-type"],
    ["container", "body.workflow_runs"],
  ]) {
    const drifted = reconcile(kind);
    assert.equal(drifted.status, 1, `${kind}: ${drifted.stderr}`);
    assert.match(
      drifted.stderr,
      new RegExp(`^runs-at-pr-1278-head\\.http: GitHub now answers otherwise at .*${at}`, "m"),
      kind,
    );
  }
  // A sidecar naming anything but a REST path of this repository is refused before gh is asked,
  // in a copy of the script and its recordings so the committed ones are never touched.
  const copy = join(dir, "copy");
  mkdirSync(join(copy, "recordings"), { recursive: true });
  copyFileSync(
    fileURLToPath(new URL("./fixtures/release-tail/reconcile.mjs", import.meta.url)),
    join(copy, "reconcile.mjs"),
  );
  for (const name of readdirSync(FIXTURES))
    copyFileSync(new URL(name, FIXTURES), join(copy, "recordings", name));
  const sidecar = join(copy, "recordings", "runs-at-release-commit.http.source.json");
  for (const path of ["repos/someone/else/actions/runs", "--hostname=example.com", ""]) {
    writeFileSync(sidecar, JSON.stringify({ ...JSON.parse(readFileSync(sidecar, "utf8")), path }));
    const refused = spawnSync(process.execPath, [join(copy, "reconcile.mjs")], {
      env: { ...process.env, PATH: `${join(dir, "bin")}:${process.env.PATH}`, ANSWER: "same" },
      encoding: "utf8",
      maxBuffer: 64 * 1024 * 1024,
    });
    assert.equal(refused.status, 2, `${path}: ${refused.stderr}`);
    assert.match(refused.stderr, /not a REST path of this repository/, path);
  }
  for (const [kind, says] of [
    ["none", /holds no response/],
    ["no-separator", /holds no response/],
    ["no-status", /starts with no HTTP status line: garbage/],
  ]) {
    const refused = reconcile(kind);
    assert.equal(refused.status, 2, `${kind}: ${refused.stderr}`);
    assert.match(refused.stderr, says, kind);
  }
});
