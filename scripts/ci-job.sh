#!/usr/bin/env bash
# The checks one of ci.yml's four jobs runs, as one Nx invocation a developer can
# run too: `just ci-lint`, `just ci-test-linux`, `just ci-test-windows` and
# `just ci-test-macos` are this script, and each job's step calls its recipe.
#
# One invocation per job rather than one per check, because Nx is what runs a
# job's checks side by side: a pull request waits on its slowest job, and a job
# that ran its tiers and its install proofs one after another would wait on their
# sum. The install proofs are Nx targets of the crate for that reason
# (`install-smoke`, `install-documented`, `wheel-smoke`), and they build into the
# job's one target directory, so the release build they share is compiled once.
#
# Every job is scoped by `scripts/nx-affected.sh` — a pull request against its
# merge base, a push against the commit before it — and fails closed the way that
# script does: with no derivable base, every project runs.
#
# Usage:
#   ci-job.sh lint|test-linux|test-windows|test-macos [--plan]
#
# `--plan` prints the tasks the job would run and runs none of them: the
# selection, readable on a host that cannot run the job's platform. Either way the
# first line on stdout names the tasks (`ci-job: <job> runs N task(s): ...`), so a
# log says what was selected before anything ran.
#
# Exit 0: every selected task passed (or none was selected). Non-zero: a task
# failed — Nx's own status — or the call was refused (2).
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT" || {
  echo "ci-job: cannot enter the repository root $ROOT" >&2
  echo "ACTION: run this from a checkout whose directories are readable" >&2
  exit 1
}

usage() {
  echo "ci-job: $1" >&2
  echo "ACTION: run 'scripts/ci-job.sh lint|test-linux|test-windows|test-macos [--plan]'" >&2
  exit 2
}

job="${1:-}"
[ -n "$job" ] || usage "name the job to run"
shift
plan=""
while [ "$#" -gt 0 ]; do
  case "$1" in
    --plan) plan="yes"; shift ;;
    *) usage "unknown argument '$1'" ;;
  esac
done

# What each job owes, as Nx targets, and the one copy of it: the workflow names
# the recipe, never these. Between them `lint` and `test-linux` hold every target
# a project's `check` reaches, and `lint` and each of the other two every target
# `just check-cross` runs (tests/ci_jobs holds both); formatting and the
# platform-independent lints run once, in `lint`.
# llmlint: ignore-block[external_service_suite_stays_out_of_the_affected_tier] `release-compat` in `lint` and `smoke`
# in `test-linux` are where the plan that made the four jobs places them; each stays its
# project's own uncached target that no `check` reaches, selected on the crate being affected
# exactly as the `release-compat` and `smoke` jobs were.
case "$job" in
  lint) targets="format-check,lint,syntax-check,msrv,deps-check,release-compat" ;;
  test-linux) targets="test,doc,budgets,test-ci-shape,smoke,install-smoke,install-documented,wheel-smoke" ;;
  test-windows | test-macos) targets="lint,test-quick,test-launcher,install-smoke,install-documented" ;;
  *) usage "'$job' is not one of ci.yml's jobs" ;;
esac
# llmlint: ignore-end[external_service_suite_stays_out_of_the_affected_tier]

nx_run() {
  bash scripts/nx-affected.sh -t "$targets" "$@"
}
graph="$(ONEPIPELINE_NX_SHOW_OUTPUT=1 nx_run --graph=stdout)" || {
  printf '%s\n' "$graph" >&2
  echo "ci-job: Nx could not plan $job's tasks" >&2
  echo "ACTION: fix the Nx error above; 'just nx show projects' lists what it can read" >&2
  exit 1
}
tasks="$(printf '%s' "$graph" | node scripts/ci-job-tasks.mjs)" || {
  printf '%s\n' "$graph" >&2
  echo "ci-job: could not read the task graph Nx planned for $job" >&2
  echo "ACTION: run 'just nx run-many -t $targets --graph=stdout' and fix what it reports" >&2
  exit 1
}
count=0
# shellcheck disable=SC2086 # the task ids are space-separated words by construction
[ -z "$tasks" ] || count="$(printf '%s\n' $tasks | wc -l | tr -d ' ')"
echo "ci-job: $job runs $count task(s): ${tasks:-none}"
[ -z "$plan" ] || exit 0

if [ "$count" -gt 0 ]; then
  # Streamed, without Nx's per-line prefix: the test legs report a line per test
  # as it finishes, so a job cut short names where it stopped, and
  # `just rerun-failed` reads nextest's status lines off the start of each line.
  ONEPIPELINE_NX_SHOW_OUTPUT=1 nx_run --outputStyle=stream-without-prefixes || {
    status=$?
    echo "ci-job: $job failed (Nx exit $status); the failing task's output is above" >&2
    echo "ACTION: fix what that task reports, then re-run 'just ci-$job'" >&2
    exit "$status"
  }
fi

# A committed lockfile the bootstrap or a tier had to change is a stale generated
# file; fail rather than let it drift. Asked whether or not a task ran, because
# the job's bootstrap installs from both lockfiles before this runs.
if [ "$job" = "test-linux" ]; then
  git diff --exit-code -- Cargo.lock package-lock.json || {
    echo "ci-job: a committed lockfile differs from what this checkout resolves (diff above)" >&2
    echo "ACTION: run 'just bootstrap', commit the updated lockfile, and push" >&2
    exit 1
  }
fi
