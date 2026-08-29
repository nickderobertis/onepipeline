// The reporter that gives a `workflow_run` failure somewhere to be seen, driven
// as a subprocess the way `published-smoke.yml`'s reporting step drives it.
//
// `gh` is the one collaborator substituted, and it is substituted on the search
// path rather than intercepted inside the script: the real boundary is filing
// issues into this repository, which a check cannot cross without opening a real
// issue on every run. The stub is therefore exercised as the real thing — the
// assertions read the argv the reporter actually invoked.
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
const WROTE_TO = "https://example.invalid/issues/7";

// A `gh` that records the arguments it was given and answers `issue list` from a
// file, so a journey picks the reporter's branch by choosing what the host says
// rather than by telling it which to take. `GH_ERROR` — or `GH_FAIL_SILENT`, for
// the `gh` that fails saying nothing — turns it into one that refuses, and
// `GH_FAIL_LIST` decides whether the listing refuses too or only the write.
const FAKE_GH = `#!/usr/bin/env bash
printf '%s\\n' "$*" >>"$GH_CALLS"
if [ "\${1:-}" = "issue" ] && [ "\${2:-}" = "list" ] && [ -z "\${GH_FAIL_LIST:-}" ]; then
  cat "$GH_EXISTING"
  exit 0
fi
if [ -n "\${GH_ERROR:-}" ] || [ -n "\${GH_FAIL_SILENT:-}" ]; then
  [ -z "\${GH_ERROR:-}" ] || printf '%s\\n' "$GH_ERROR" >&2
  exit 1
fi
echo "${WROTE_TO}"
exit 0
`;

/**
 * Run the reporter with `gh` stubbed on PATH.
 *
 * `existing` is what `gh issue list` prints — the `number<TAB>title` lines the
 * reporter's own `--jq` program produces — and `env` overrides what the workflow
 * step passes.
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
      stdout: run.stdout,
      said: `exit ${run.status}\n--- stdout ---\n${run.stdout}\n--- stderr ---\n${run.stderr}`,
      calls: readFileSync(calls, "utf8").split("\n").filter(Boolean),
    };
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
}

function called(run, prefix) {
  return run.calls.some((call) => call.startsWith(prefix));
}

describe("the reporter files a workflow_run failure", () => {
  it("opens an issue when there is none open, and says where it went", () => {
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
    assert.match(run.stdout, /opened a new issue/, run.said);
    assert.ok(run.stdout.includes(WROTE_TO), `the log does not say where it filed:\n${run.said}`);
  });

  it("comments on its own open issue rather than opening a second one", () => {
    const run = report({ existing: `41\t${TITLE}\n` });

    assert.equal(run.status, 0, run.said);
    assert.ok(called(run, "issue comment 41 "), `expected a comment on #41:\n${run.said}`);
    assert.ok(
      !called(run, "issue create "),
      `a second issue per failure is the pile this exists to avoid:\n${run.said}`,
    );
    assert.match(run.stdout, /commented on #41/, run.said);
    assert.ok(
      run.stdout.includes(WROTE_TO),
      `the log does not say where it commented:\n${run.said}`,
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

  // A caller error is refused before anything is filed, and says which input it
  // was: the caller is a workflow step nobody is reading at the time.
  for (const missing of ["REPO", "TITLE", "BODY"]) {
    it(`refuses a missing ${missing} by name, without filing an empty issue`, () => {
      const run = report({ env: { [missing]: "" } });

      assert.equal(run.status, 2, run.said);
      assert.ok(!called(run, "issue "), `a refused run must not call gh at all:\n${run.said}`);
      assert.match(run.said, new RegExp(missing), run.said);
      assert.match(run.said, /ACTION:/, run.said);
    });
  }

  it("sends the title to the search as a quoted phrase, so a bare operator is text", () => {
    // `in:`, `-label` and friends are search operators when they arrive bare, so
    // an unquoted title is a query the caller did not write.
    const run = report();

    const listed = run.calls.find((call) => call.startsWith("issue list "));
    assert.ok(listed, `expected an 'issue list' call:\n${run.said}`);
    assert.ok(
      listed.includes(`"${TITLE}" in:title`),
      `the title reaches GitHub's search language unquoted:\n${run.said}`,
    );
  });

  it("refuses a title the quoted search phrase cannot carry", () => {
    const run = report({ env: { TITLE: 'Published "smoke" is failing' } });

    assert.equal(run.status, 2, run.said);
    assert.ok(
      !called(run, "issue "),
      `a title that cannot be searched for is refused first:\n${run.said}`,
    );
    assert.match(run.said, /TITLE/, run.said);
    assert.match(run.said, /Nothing has been filed/, run.said);
  });

  it("refuses a RUN_URL that is not an http(s) URL rather than pasting it into the issue", () => {
    const run = report({ env: { RUN_URL: "javascript:alert(1)" } });

    assert.equal(run.status, 2, run.said);
    assert.ok(!called(run, "issue "), `a link nobody should click is refused first:\n${run.said}`);
    assert.match(run.said, /RUN_URL/, run.said);
    assert.match(run.said, /Nothing has been filed/, run.said);
  });

  it("refuses a REPO that is not an owner/name repository", () => {
    const run = report({ env: { REPO: "https://github.com/owner/repo" } });

    assert.equal(run.status, 2, run.said);
    assert.ok(!called(run, "issue "), `a malformed repository must be refused first:\n${run.said}`);
    assert.match(run.said, /REPO/, run.said);
    assert.match(run.said, /Nothing has been filed/, run.said);
  });

  // Each `gh` failure is answered with what it was doing, what `gh` said, and the
  // next action that particular answer calls for — and, whatever went wrong, with
  // the red run it was reporting, which is the finding that must not be lost.
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
      what: "the listing, saying nothing at all",
      env: { GH_FAIL_SILENT: "1", GH_FAIL_LIST: "1" },
      existing: "",
      expects: [/said nothing/, /ACTION:/],
    },
    {
      what: "the listing, on a repository that did not resolve",
      env: { GH_ERROR: "HTTP 404: Not Found", GH_FAIL_LIST: "1" },
      existing: "",
      expects: [/HTTP 404/, /owner\/repo/, /typo/],
    },
    {
      what: "the listing, on a query GitHub rejected",
      env: { GH_ERROR: "HTTP 422: Validation Failed", GH_FAIL_LIST: "1" },
      existing: "",
      expects: [/HTTP 422/, /TITLE/],
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

      assert.equal(run.status, 1, run.said);
      for (const expected of expects) {
        assert.match(run.said, expected, run.said);
      }
      assert.ok(run.said.includes(RUN_URL), `the finding it was reporting is lost:\n${run.said}`);
    });
  }
});

describe("the workflow the reporter answers for", () => {
  const workflow = readFileSync(WORKFLOW, "utf8");
  const release = readFileSync(join(REPO_ROOT, ".github", "workflows", "release.yml"), "utf8");

  it("asks the registries when a release completes, and on no schedule", () => {
    // `workflow_run` matches on the triggering workflow's `name:`, which lives in
    // release.yml. Read it from there: spelled apart, nothing fails — the smoke
    // is simply triggered by nothing, which is the silence this change ends.
    const releaseName = release.match(/^name: (.+)$/m)?.[1];
    assert.ok(releaseName, "release.yml has no `name:` for a workflow_run to match on");

    assert.match(workflow, /^ {2}workflow_run:$/m, "the smoke is not triggered by a workflow run");
    assert.ok(
      workflow.includes(`workflows: ["${releaseName}"]`),
      `the smoke waits on a workflow named something other than release.yml's "${releaseName}"`,
    );
    assert.match(workflow, /^ {2}workflow_dispatch:$/m, "the manual entry point is gone");
    assert.match(workflow, /^ {6}version:$/m, "the dispatch lost its version input");
    assert.ok(
      !/^\s*(schedule:|- cron:)/m.test(workflow),
      "a cron survived the move, so the sweep this replaced is still running",
    );
  });

  it("binds every input the reporter declares it requires", () => {
    // The workflow step restates that contract in YAML. Read the declaration out
    // of the reporter — one list, in the file that answers for it — so an input
    // added there and nowhere else fails here rather than at the one moment the
    // reporter is needed.
    const declared = readFileSync(REPORTER, "utf8").match(/^for required in (.+); do$/m)?.[1];
    assert.ok(declared, "the reporter no longer declares its required inputs in one place");

    const job = workflow.slice(workflow.indexOf("\n  report:"));
    for (const name of declared.split(/\s+/)) {
      assert.match(job, new RegExp(`^ +${name}:`, "m"), `the reporting job never sets ${name}`);
    }
  });

  it("reports a failure from a checkout, with permission to write the issue", () => {
    const job = workflow.slice(workflow.indexOf("\n  report:"));
    assert.ok(job.includes("if: failure()"), "the reporting job does not run on a failure");
    assert.ok(job.includes("issues: write"), "the reporting job cannot write an issue");
    const checkout = job.indexOf("uses: actions/checkout@v4");
    assert.ok(
      checkout !== -1 && checkout < job.indexOf("report-workflow-failure.sh"),
      "the reporting job runs the script without checking the repository out first, so the file it runs is not in its workspace",
    );
    assert.ok(
      job.includes("run: bash scripts/report-workflow-failure.sh"),
      "the reporting job does not run the reporter this suite proves",
    );
  });
});
