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
# can judge, 3 when this checkout or host cannot support a run, and otherwise
# llmlint's own status, which is this task's status and so the cache's verdict.
#
# llmlint's own report is this tier's product — Nx replays this task's terminal
# output in place of a verdict record — so it is handed through untouched rather
# than reduced to a line. What this script adds is its own refusals, which name the
# one thing to fix.
set -euo pipefail

# Every caller runs this from the repository root: `just` from the justfile's own
# directory, Nx from the workspace root, and `scripts/llmlint-diff.sh` from the root
# it checked for itself. So the root is required rather than climbed to — a run from
# anywhere else would answer about a different tree than the one being judged.
[ -f llmlint.yml ] || {
  echo "lint-llm-diff: run this from the repository root, which is where the judge configuration it lints under is; 'just lint-llm-diff <base>' does that for you" >&2
  exit 3
}
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

llmlint_runtime_env || exit 3

exec llmlint --diff --diff-base "$base_sha"
