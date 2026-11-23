#!/usr/bin/env bash
# The checks one of ci.yml's jobs runs, as one Nx invocation a developer can run
# too: `just ci-lint`, `just ci-test-linux`, `just ci-test-windows`,
# `just ci-test-macos` and `just ci-timed` are this script, and each job's step
# calls its recipe.
#
# One invocation per job rather than one per check, because Nx is what runs a
# job's checks side by side: a pull request waits on its slowest job, and a job
# that ran its tiers and its install proofs one after another would wait on their
# sum. The install proofs are Nx targets for that reason (the crate's
# `install-smoke` and `install-documented`, `onepipeline-packaging`'s
# `wheel-smoke`). `install-smoke` builds into the job's one target directory, so
# the release build it shares is compiled once; `install-documented` builds the
# remote's default branch into a directory of its own (`scripts/install-proof.sh`).
#
# A job keeps an install proof only where it can catch a regression. test-linux
# runs `install-smoke` whenever the crate is affected, since `host-view-time` times
# the binary it installs; test-windows and test-macos run it, and every test job
# `install-documented`, only when the proof's selection project
# (`install/*/project.json`) is affected, which lists what decides that install
# beyond the code. A source change that breaks either install on Windows or macOS
# fails release.yml's per-platform release build and test-install before anything
# is published, and `timed` runs `install-documented` on every push to main that
# affects the crate.
#
# Every job is scoped by `scripts/nx-affected.sh` — a pull request against its
# merge base, a push against the commit before it — and fails closed the way that
# script does: with no derivable base, every project runs. That scope drops the
# packaging proofs for a change that moves only a version or a recipe no package
# runs (`scripts/nx-version-free-selection.mjs`).
#
# `timed` is the push-to-main job's: each selected project's `timed` target times a
# release binary and checks its own budgets, saving the results as
# `target/budget-results/<project>.json`. The documented install runs beside them and
# never during one: every `timed` target is `parallelism: false`. Whatever the targets' outcome, each budget
# those files hold over its threshold, or unmeasured, is then printed as one line
# naming the budget, its figure and its threshold (`scripts/budget-annotations.mjs`),
# a GitHub error annotation in CI, so the commit's red status says which.
#
# Usage:
#   ci-job.sh lint|test-linux|test-windows|test-macos|timed [--plan]
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
  echo "ACTION: run 'scripts/ci-job.sh lint|test-linux|test-windows|test-macos|timed [--plan]'" >&2
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
# platform-independent lints run once, in `lint`. The MSRV check and the
# pinned-wheel channel journeys (`release-compat`) run in `test-linux` rather
# than `lint`: each is a whole-workspace compile (a second toolchain's check; the
# crate's debug build, which `test-linux` makes anyway), and beside the
# visual-docs capture's container build in `lint` they held it past 30 minutes.
# llmlint: ignore-block[external_service_suite_stays_out_of_the_affected_tier] `release-compat` and `smoke`
# in `test-linux` are each their project's own uncached target that no `check` reaches,
# selected on the crate being affected exactly as the `release-compat` and `smoke` jobs were;
# `report-job:test`, selected by `test`, is its project's own target no `check` reaches, reading
# the action's public release and PyPI's public wheel anonymously, selected only when what it
# reads changes and uncached, as its budget needs the released reader it saves beside its telemetry.
case "$job" in
  lint) targets="format-check,lint,syntax-check,deps-check" ;;
  test-linux) targets="test,doc,budgets,test-ci-shape,msrv,smoke,release-compat,install-smoke,install-documented,wheel-smoke" ;;
  test-windows | test-macos) targets="lint,test-quick,test-launcher,install-smoke,install-documented" ;;
  timed) targets="timed,install-documented" ;;
  *) usage "'$job' is not one of ci.yml's jobs" ;;
esac
# llmlint: ignore-end[external_service_suite_stays_out_of_the_affected_tier]

# The install proofs a pull-request job runs only when their selection project is
# affected. `--affects` fails closed, answering `true` with no derivable base, so an
# unanswerable selection runs the proof rather than skipping it.
case "$job" in
  test-linux) narrowed="install-documented" ;;
  test-windows | test-macos) narrowed="install-smoke install-documented" ;;
  *) narrowed="" ;;
esac
for proof in $narrowed; do
  affected="$(bash scripts/nx-affected.sh --affects "onepipeline-$proof-selection")" || {
    echo "ci-job: could not ask whether $job's $proof is selected (nx-affected.sh's reason is above)" >&2
    echo "ACTION: run 'bash scripts/nx-affected.sh --affects onepipeline-$proof-selection' and fix what it reports" >&2
    exit 1
  }
  [ "$affected" = true ] && continue
  targets="$(printf ',%s,' "$targets" | sed "s/,$proof,/,/; s/^,//; s/,\$//")"
done

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

# The results each selected `timed` target owes, cleared first so a file an earlier
# run left cannot stand in for one this run never wrote.
results=()
if [ "$job" = timed ]; then
  # shellcheck disable=SC2086 # the task ids are space-separated words by construction
  for task in $tasks; do
    case "$task" in *:timed) results+=("target/budget-results/${task%%:*}.json") ;; esac
  done
  [ "${#results[@]}" -eq 0 ] || rm -f "${results[@]}"
fi

status=0
if [ "$count" -gt 0 ]; then
  # Streamed, without Nx's per-line prefix: the test legs report a line per test
  # as it finishes, so a job cut short names where it stopped, and
  # `just rerun-failed` reads nextest's status lines off the start of each line.
  ONEPIPELINE_NX_SHOW_OUTPUT=1 nx_run --outputStyle=stream-without-prefixes || status=$?
fi
[ "${#results[@]}" -eq 0 ] || node scripts/budget-annotations.mjs "${results[@]}"
if [ "$status" -ne 0 ]; then
  echo "ci-job: $job failed (Nx exit $status); the failing task's output is above" >&2
  echo "ACTION: fix what that task reports, then re-run 'just ci-$job'" >&2
  exit "$status"
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
