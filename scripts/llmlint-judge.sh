#!/usr/bin/env bash
# Body of the cached Nx `onepipeline:lint-llm-diff` target: judge this branch's
# diff against one resolved base commit. Run it through `just lint-llm-diff
# <base>`, which resolves the base ref to the commit this reads and keys the cache
# on.
#
# llmlint runs, and its exit status is this task's exit status — so Nx caches a
# clean run and restores the verdict it recorded verbatim, while a run with findings
# and a run that never reached a verdict both stay uncached and are judged again.
#
# The base arrives as `LLMLINT_DIFF_BASE_SHA` rather than as an argument because Nx
# hashes declared environment variables but not target arguments: keying and
# judging on the same value is what stops a clean verdict computed against one base
# from being replayed for another. Exits 2 when what it was handed is not a base it
# can judge, 3 when this checkout or host cannot support a run, and otherwise
# llmlint's own status, which is this task's status and so the cache's verdict.
#
# A clean run says one line — the verdict, and where the run behind it is readable
# in full — and records that same line as this target's declared output, which is
# what Nx stores and restores for a replay. A run with findings says everything
# llmlint said, since nobody replays it and the operator has to clear it.
#
# Every run that ends non-zero also *records* what it said, under
# `.lint-llm-diff/report`. Saying it is not enough on its own: this task's streams
# reach `scripts/llmlint-diff.sh` only through Nx, which has forwarded its wrapper
# and nothing else on a loaded host, and a judged tier that fails with no reason
# attached is one a developer can only act on by paying for the run again by hand.
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
  # What to do about it differs, and what llmlint left behind says which: nothing
  # at all is a judge that said nothing to act on, 1 is a verdict against the diff,
  # and anything above it is a judge that never reached one.
  if [ ! -s "$report" ]; then
    advice="lint-llm-diff: llmlint exited $status without reporting anything; run 'llmlint --diff --diff-base $base_sha -v' by hand to see what it did, then retry"
  elif ((status == 1)); then
    advice="lint-llm-diff: clear the findings above with 'just lint-llm-diff <base>' alone, then run the gate once to confirm"
  else
    advice="lint-llm-diff: llmlint exited $status without judging this diff; run 'llmlint doctor', or 'just setup-llmlint' to reinstall the toolchain, then retry"
  fi
  # Recorded as well as said, for the reason the refusal below is: what this says
  # reaches `scripts/llmlint-diff.sh` only through Nx's pipes, and on a loaded host
  # has arrived there as nothing at all — which leaves the judged tier red with no
  # reason attached, and a developer rerunning this script by hand outside Nx to
  # read an error message the paid run already produced. Every non-zero report is
  # recorded, findings and toolchain failure and silence alike, because which of
  # the three happened is exactly what the missing diagnostic would have said.
  #
  # Not a declared output, so Nx neither stores nor restores it: Nx caches
  # successful tasks only, and this record exists only for runs that failed. It is
  # this run's — written before this process exits, and cleared by the driver
  # before the next.
  if mkdir -p "$root/.lint-llm-diff" &&
    { cat "$report" && printf '%s\n' "$advice"; } >"$root/.lint-llm-diff/report"; then
    cat "$root/.lint-llm-diff/report" >&2
  else
    # The report is still said, since it is what the operator acts on, and the
    # status stays llmlint's: a checkout that cannot hold the record has not
    # changed what the judge ruled about this diff, and reporting a findings exit
    # as a broken host would send the operator to repair the wrong thing.
    cat "$report" >&2
    printf '%s\n' "$advice" >&2
    echo "lint-llm-diff: could not record that report in .lint-llm-diff/report, so a caller Nx forwards nothing to will see no reason for this failure; free disk space and retry" >&2
  fi
  exit "$status"
fi

# A clean run is what Nx stores and replays, so it is one line: the verdict and
# where the run behind it can be read in full. A run that reached no verdict is not
# a clean run whatever llmlint's status said, and must not be stored as one.
verdict="$(grep -m1 -E '^[0-9]+ rules: ' "$report")" || {
  refusal="lint-llm-diff: llmlint exited cleanly without reporting a verdict for this diff; run 'llmlint --diff --diff-base $base_sha -v' to see what it did, then retry"
  # Recorded as well as said, for the reason the verdict below is: what this says
  # reaches `scripts/llmlint-diff.sh` only through Nx's pipes, and a refusal that
  # arrived there as nothing at all would read as a judge that never spoke. The
  # record is not a declared output, so Nx neither stores nor restores it: it is
  # this run's, written before this process exits, and the driver clears it before
  # the next. A record that could not be written is the checkout failing this run,
  # not the judge reaching no verdict, and exits as the verdict's own write does
  # below — the refusal is still said, since it is what the operator retries for.
  if ! mkdir -p "$root/.lint-llm-diff" || ! printf '%s\n' "$refusal" >"$root/.lint-llm-diff/refusal"; then
    echo "$refusal" >&2
    echo "lint-llm-diff: could not record that refusal in .lint-llm-diff/refusal; free disk space and retry" >&2
    exit 3
  fi
  echo "$refusal" >&2
  exit 2
}
pointer="$(sed -n 's/.*\(llmlint history [A-Za-z0-9_-]*\).*/\1/p' "$report" | tail -1)"
line="lint-llm-diff: ${verdict}${pointer:+ (full report: $pointer)}"
# The line is recorded as well as printed, because what this task prints reaches
# `scripts/llmlint-diff.sh` only through Nx's pipes, and Nx decides for itself when
# that output is finished. The record is complete by the time this task has exited:
# the write below returns before this process can exit, Nx stores the target's
# declared outputs only after that exit, and it restores them before it reports a
# hit. So the record is the verdict, and the printed line is for whoever is reading.
if ! mkdir -p "$root/.lint-llm-diff" || ! printf '%s\n' "$line" >"$root/.lint-llm-diff/verdict"; then
  echo "lint-llm-diff: could not record the verdict in .lint-llm-diff/verdict; free disk space and retry" >&2
  exit 3
fi
printf '%s\n' "$line"
