// The reporter that gives a `workflow_run` failure somewhere to be seen.
//
// `published-smoke.yml` runs when `release.yml` completes, so it has no pull
// request to redden and nobody waiting on it: a red run announces itself only if
// something files it. `scripts/report-workflow-failure.sh` is that something, and
// the one time it matters is the one time nobody is watching it work — it runs
// exactly when the thing it reports on is already broken.
//
// So both of its branches are driven here as a subprocess, the way the workflow
// step drives it: no open issue (it must CREATE one) and its own issue already
// open (it must COMMENT, never open a second, or a bad week at a registry becomes
// a pile of issues nobody reads). `gh` is the one collaborator substituted, and it
// is substituted on the search path rather than intercepted inside the script —
// the real boundary is filing issues into this repository, which a check cannot
// cross without opening a real issue every run. The stub is exercised as the real
// thing: the assertions read the argv the reporter actually invoked.
//
// The last journeys here cover the failure side, because a reporter that dies
// quietly takes the finding down with it: a refused input, and a `gh` that fails
// on each of the three calls the reporter makes.
import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { chmodSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { delimiter, dirname, join, resolve } from "node:path";
import { describe, it } from "node:test";
import { fileURLToPath } from "node:url";

const REPO_ROOT = resolve(dirname(fileURLToPath(import.meta.url)), "..", "..");
const REPORTER = join(REPO_ROOT, "scripts", "report-workflow-failure.sh");
const WORKFLOW = join(REPO_ROOT, ".github", "workflows", "published-smoke.yml");

// The title every journey files under, so one can plant an issue matching it
// exactly and one that only looks like it.
const TITLE = "Published smoke is failing";
const RUN_URL = "https://example.invalid/run/1";

// A `gh` that records the arguments it was given and answers `issue list` from a
// file, so a journey picks which branch the reporter should take. `GH_ERROR`
// turns it into a `gh` that fails, and `GH_FAIL_LIST` decides whether the listing
// fails too or only the write that follows it.
const FAKE_GH = `#!/usr/bin/env bash
printf '%s\\n' "$*" >>"$GH_CALLS"
if [ "\${1:-}" = "issue" ] && [ "\${2:-}" = "list" ] && [ -z "\${GH_FAIL_LIST:-}" ]; then
  cat "$GH_EXISTING"
  [ -z "\${GH_ERROR:-}" ] || exit 0
  exit 0
fi
if [ -n "\${GH_ERROR:-}" ]; then
  printf '%s\\n' "$GH_ERROR" >&2
  exit 1
fi
echo "https://example.invalid/issues/7"
exit 0
`;

/**
 * Run the reporter the way the workflow step runs it, with `gh` stubbed on PATH.
 *
 * `existing` is what `gh issue list` prints — the `number<TAB>title` lines the
 * reporter's `--jq` program produces — so a journey chooses the branch by
 * choosing what the host answers, not by telling the reporter which to take.
 */
function report({ existing = "", env = {} } = {}) {
  const dir = mkdtempSync(join(tmpdir(), "onepipeline-report-"));
  try {
    const bin = join(dir, "bin");
    mkdirSync(bin);
    writeFileSync(join(bin, "gh"), FAKE_GH);
    chmodSync(join(bin, "gh"), 0o755);
    const calls = join(dir, "calls");
    writeFileSync(calls, "");
    const listing = join(dir, "existing");
    writeFileSync(listing, existing);

    const run = spawnSync("bash", [REPORTER], {
      cwd: REPO_ROOT,
      encoding: "utf8",
      env: {
        ...process.env,
        PATH: `${bin}${delimiter}${process.env.PATH}`,
        GH_CALLS: calls,
        GH_EXISTING: listing,
        REPO: "owner/repo",
        TITLE,
        BODY: "the smoke failed",
        RUN_URL,
        ...env,
      },
    });
    return {
      status: run.status,
      said: `exit ${run.status}\n--- stdout ---\n${run.stdout}\n--- stderr ---\n${run.stderr}`,
      calls: readFileSync(calls, "utf8").split("\n").filter(Boolean),
    };
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
}

/** Whether the reporter made a `gh` call starting with these words. */
function called(run, prefix) {
  return run.calls.some((call) => call.startsWith(prefix));
}

describe("the reporter files a workflow_run failure", () => {
  it("opens an issue when there is none open, and puts the run behind it", () => {
    const run = report();

    assert.equal(run.status, 0, run.said);
    assert.ok(called(run, "issue create "), `expected an 'issue create' call:\n${run.said}`);
    assert.ok(
      !called(run, "issue comment "),
      `nothing was open, so there was nothing to comment on:\n${run.said}`,
    );
    assert.ok(
      run.calls.some((call) => call.includes(RUN_URL)),
      `the red run's URL has to reach the issue body, or the finding is unreachable from it:\n${run.said}`,
    );
  });

  it("comments on its own open issue rather than opening a second one", () => {
    const run = report({ existing: `41\t${TITLE}\n` });

    assert.equal(run.status, 0, run.said);
    assert.ok(called(run, "issue comment 41 "), `expected a comment on #41:\n${run.said}`);
    assert.ok(
      !called(run, "issue create "),
      `a second issue per failure is the pile this exists to avoid:\n${run.said}`,
    );
  });

  it("does not comment a smoke failure onto an issue that merely resembles its own", () => {
    // `--search … in:title` is fuzzy, so it answers with near misses. Commenting
    // onto somebody else's thread is worse than opening a second issue.
    const run = report({ existing: `41\t${TITLE} (macOS)\n` });

    assert.equal(run.status, 0, run.said);
    assert.ok(called(run, "issue create "), `a near miss is not this issue:\n${run.said}`);
    assert.ok(!called(run, "issue comment "), run.said);
  });

  it("refuses an issue id that is not a number rather than addressing a comment at it", () => {
    const run = report({ existing: `not-a-number\t${TITLE}\n` });

    assert.notEqual(run.status, 0, run.said);
    assert.ok(!called(run, "issue comment "), run.said);
  });

  it("refuses a missing input by name, without filing an empty issue", () => {
    const run = report({ env: { TITLE: "" } });

    assert.equal(run.status, 2, run.said);
    assert.ok(!called(run, "issue "), `a refused run must not call gh at all:\n${run.said}`);
    assert.match(run.said, /TITLE/, run.said);
    assert.match(run.said, /ACTION:/, run.said);
  });

  // Each `gh` failure is answered with what it was doing, what `gh` said, and the
  // next action that answer calls for — and, whatever went wrong, with the red run
  // the reporter was reporting, which is the finding that must not be lost.
  const ghFailures = [
    {
      what: "the listing, with no credential",
      env: {
        GH_ERROR: "gh: To get started with GitHub CLI, please run: gh auth login",
        GH_FAIL_LIST: "1",
      },
      existing: "",
      expects: [/looking for an open issue/, /gh auth login/, /GH_TOKEN/],
    },
    {
      what: "the create, with no permission",
      env: { GH_ERROR: "HTTP 403: Resource not accessible by integration" },
      existing: "",
      expects: [/opening an issue/, /issues: write/],
    },
    {
      what: "the comment, with a server error",
      env: { GH_ERROR: "HTTP 500: Server Error" },
      existing: `41\t${TITLE}\n`,
      expects: [/commenting on #41/, /ACTION:/],
    },
  ];

  for (const { what, env, existing, expects } of ghFailures) {
    it(`does not report a filed issue when gh fails on ${what}`, () => {
      const run = report({ existing, env });

      assert.notEqual(run.status, 0, run.said);
      for (const expected of expects) {
        assert.match(run.said, expected, run.said);
      }
      assert.match(run.said, new RegExp(RUN_URL.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")), run.said);
    });
  }
});

describe("the workflow the reporter answers for", () => {
  const workflow = readFileSync(WORKFLOW, "utf8");

  it("asks the registries when a release completes, and on no schedule", () => {
    assert.match(workflow, /^ {2}workflow_run:$/m, "the smoke is not triggered by a workflow run");
    assert.match(workflow, /^ {4}workflows: \["Release"\]$/m, "it does not name release.yml");
    assert.match(workflow, /^ {2}workflow_dispatch:$/m, "the manual entry point is gone");
    assert.match(workflow, /^ {6}version:$/m, "the dispatch lost its version input");
    assert.ok(
      !/^\s*(schedule:|- cron:)/m.test(workflow),
      "a cron survived the move, so the sweep this replaced is still running",
    );
  });

  it("reports a failure from a checkout, with permission to write the issue", () => {
    const report = workflow.slice(workflow.indexOf("\n  report:"));
    assert.ok(report.includes("if: failure()"), "the reporting job does not run on a failure");
    assert.ok(report.includes("issues: write"), "the reporting job cannot write an issue");
    const checkout = report.indexOf("uses: actions/checkout@v4");
    assert.ok(
      checkout !== -1 && checkout < report.indexOf("report-workflow-failure.sh"),
      "the reporting job runs the script without checking the repository out first, so the file it runs is not in its workspace",
    );
    assert.ok(
      report.includes("run: bash scripts/report-workflow-failure.sh"),
      "the reporting job does not run the reporter this suite proves",
    );
  });
});
