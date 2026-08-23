#!/usr/bin/env bash
# Body of the cached Nx `onepipeline:lint-llm-diff` target: judge this branch's
# diff against one resolved base commit. Run it through `just lint-llm-diff
# <base>`, which resolves the base ref to the commit this reads and keys the cache
# on.
#
# Nothing here records or replays a verdict. llmlint runs, what it concluded is this
# task's terminal output, and its exit status is this task's exit status — so Nx
# caches a clean run and replays its verdict verbatim, while a run with findings and
# a run that never reached a verdict both stay uncached and are judged again.
#
# The base arrives as `LLMLINT_DIFF_BASE_SHA` rather than as an argument because Nx
# hashes declared environment variables but not target arguments: keying and
# judging on the same value is what stops a clean verdict computed against one base
# from being replayed for another. Exits 2 when what it was handed is not a base it
# can judge, 3 when this checkout or host cannot support a run, and otherwise
# llmlint's own status, which is this task's status and so the cache's verdict.
#
# A clean run says one line — the verdict, and where the run behind it is readable
# in full — because that line is what Nx stores and replays in place of a verdict
# record. A run with findings says everything llmlint said, since nobody replays it
# and the operator has to clear it.
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

report="$(mktemp)" || {
  echo "lint-llm-diff: could not open temporary storage for the judge's report; free disk space and retry" >&2
  exit 3
}
trap 'rm -f "$report"' EXIT

status=0
llmlint --diff --diff-base "$base_sha" >"$report" 2>&1 || status=$?
if ((status != 0)); then
  # Never cached, so never replayed: a failure can afford every byte, and the
  # operator who has to clear it is the one who needs the judge's report verbatim.
  # What to do about it differs, and llmlint's own status says which: 1 is a
  # verdict against the diff, anything above it is a judge that never reached one.
  cat "$report" >&2
  if ((status == 1)); then
    echo "lint-llm-diff: clear the findings above with 'just lint-llm-diff <base>' alone, then run the gate once to confirm" >&2
  else
    echo "lint-llm-diff: llmlint exited $status without judging this diff; run 'llmlint doctor', or 'just setup-llmlint' to reinstall the toolchain, then retry" >&2
  fi
  exit "$status"
fi

# A clean run is what Nx stores and replays, so it is one line: the verdict and
# where the run behind it can be read in full. A run that reached no verdict is not
# a clean run whatever llmlint's status said, and must not be stored as one.
verdict="$(grep -m1 -E '^[0-9]+ rules: ' "$report")" || {
  echo "lint-llm-diff: llmlint exited cleanly without reporting a verdict for this diff; run 'llmlint --diff --diff-base $base_sha -v' to see what it did, then retry" >&2
  exit 2
}
pointer="$(sed -n 's/.*\(llmlint history [A-Za-z0-9_-]*\).*/\1/p' "$report" | tail -1)"
printf 'lint-llm-diff: %s%s\n' "$verdict" "${pointer:+ (full report: $pointer)}"
