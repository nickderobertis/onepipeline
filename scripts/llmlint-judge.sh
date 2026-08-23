#!/usr/bin/env bash
# Body of the cached Nx `onepipeline:lint-llm-diff` target: judge this branch's
# diff against one resolved base commit. Run it through `just lint-llm-diff
# <base>`, which resolves the base ref to the commit this reads and keys the cache
# on.
#
# Nothing here records or replays a verdict. llmlint runs, its report is this
# task's terminal output, and its exit status is this task's exit status — so Nx
# caches a clean run and replays that report verbatim, while a run with findings
# and a run that never reached a verdict both stay uncached and are judged again.
#
# The base arrives as `LLMLINT_DIFF_BASE_SHA` rather than as an argument because Nx
# hashes declared environment variables but not target arguments: keying and
# judging on the same value is what stops a clean verdict computed against one base
# from being replayed for another. Exits 2 when what it was handed is not a base it
# can judge, 3 when this checkout cannot be read, and otherwise llmlint's own
# status.
#
# llmlint: ignore-file[tool_output_is_signal] llmlint's own report is this tier's
# product — Nx replays this task's terminal output in place of a verdict record —
# so it is handed through untouched rather than reduced to a line.
set -euo pipefail

#: Prefix of the status line this states on the way out, on every path. Nx collapses
#: a failed task to 1, so without it findings, a toolchain that never reached a
#: verdict, and a checkout that could not run one all reach the caller as the same
#: answer. `scripts/llmlint-diff.sh` matches the same string and keeps it out of
#: what it replays, so the two must agree.
readonly JUDGE_STATUS_MARKER="lint-llm-diff: judge exit status"
trap 'printf "%s %s\n" "$JUDGE_STATUS_MARKER" "$?" >&2' EXIT

# llmlint: ignore-block[changed_behavior_has_e2e] Reaching this needs a checkout
# whose own directory cannot be entered while this script is still readable inside
# it, which no journey can arrange without root: every other path through this file
# is driven in npm/test/llmlint-cache.test.mjs.
CDPATH='' cd -- "$(dirname -- "$0")/.." || {
  echo "lint-llm-diff: could not enter the repository from this script; reinstall the checkout and retry" >&2
  exit 3
}
# llmlint: ignore-end[changed_behavior_has_e2e]
root=$PWD
# shellcheck source=scripts/llmlint-runtime-env.sh
. "$root/scripts/llmlint-runtime-env.sh" || {
  echo "lint-llm-diff: could not load the shared runtime environment; restore scripts/llmlint-runtime-env.sh and retry" >&2
  exit 3
}
base_sha="${LLMLINT_DIFF_BASE_SHA:-}"
[[ "$base_sha" =~ ^[0-9a-f]{40,64}$ ]] || {
  echo "lint-llm-diff: LLMLINT_DIFF_BASE_SHA must be a resolved commit id; run 'just lint-llm-diff <base>' instead of this target directly" >&2
  exit 2
}
git -C "$root" rev-parse --verify --quiet "${base_sha}^{commit}" >/dev/null || {
  echo "lint-llm-diff: base commit '$base_sha' is missing from this checkout; fetch it and retry" >&2
  exit 2
}

llmlint_runtime_env
exec_status=0
llmlint --diff --diff-base "$base_sha" || exec_status=$?
exit "$exec_status"
