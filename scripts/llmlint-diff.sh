#!/usr/bin/env bash
# The judged tier, memoized: judge this branch's diff against one base commit, or
# replay the verdict already recorded for that tree, that base, and that judge
# configuration. `just lint-llm-diff <base> [nx args]` is how it is invoked.
#
# The judge is non-deterministic across the gap between what it judges — every file
# in the base-to-head diff, because llmlint has no increment mode — and what
# changed, so with no memo every gate run over one diff is an independent roll, and
# rolls of one branch have named a different rule each time. The cached Nx
# `onepipeline:lint-llm-diff` target caches the judge run itself; there is no
# verdict record to write, restore, or race on.
#
# The base ref is resolved to a commit here, before Nx hashes it, so a rebased or
# advanced base misses rather than replaying a verdict computed against a different
# comparison, and it is reported with the verdict because "green" means green
# *against that commit*.
#
# Only a clean run is cached, because Nx caches successful tasks only: findings
# (llmlint exit 1) and a toolchain that never reached a verdict (exit >= 2) are
# both judged again next invocation. `--skip-nx-cache` forces one fresh judgement
# and is deliberately per-invocation; an ambient `NX_SKIP_NX_CACHE` /
# `NX_DISABLE_NX_CACHE` is reported and ignored, because it would re-roll a
# non-deterministic judge from every unrelated command. Every other Nx target
# still honours it.
#
# Exits 0 when the judge certified this diff and 1 when it did not, which is what
# a gate acts on; the report above the exit is what a person acts on. 2 is a
# question this tier could not be asked — a base or an option it cannot use, or a
# judge configuration it could not fingerprint. 3 is this checkout or host being
# unable to support a run at all, which says nothing about the diff.
#
# The judge's report is this tier's product — Nx replays this task's terminal output
# in place of a verdict record — so it is handed through whole. What this script
# adds to it is one line saying whether the verdict was judged or replayed, and the
# refusals below, which each name the one thing to fix.
set -euo pipefail

# Every caller runs this from the repository root: `just` from the justfile's own
# directory, Nx from the workspace root, and `scripts/llmlint-diff.sh` from the root
# it checked for itself. So the root is required rather than climbed to — a run from
# anywhere else would answer about a different tree than the one being judged.
[ -f nx.json ] || {
  echo "lint-llm-diff: run this from the repository root, which is where the Nx workspace it hands this tier to is; 'just lint-llm-diff <base>' does that for you" >&2
  exit 3
}
root=$PWD

# The base arrives from a command line, so its shape is checked before it reaches
# git or Nx: a ref is what a ref may look like. Everything after it is passed to Nx
# as separate arguments rather than as text.
(($# >= 1)) || {
  echo "lint-llm-diff: pass the base to judge against, e.g. 'just lint-llm-diff origin/main'" >&2
  exit 2
}
base_ref=$1
shift
[[ "$base_ref" =~ ^[A-Za-z0-9][A-Za-z0-9._/~^-]*$ ]] || {
  echo "lint-llm-diff: '$base_ref' is not a usable git ref; pass a branch, tag, or commit" >&2
  exit 2
}
# The tier's own levers, rather than whatever the caller wanted Nx to do: an
# unrecognized flag here would change how a non-deterministic judge is run without
# this saying so, and `--skip-nx-cache` is the one documented way to re-judge.
for argument in "$@"; do
  case "$argument" in
  --skip-nx-cache) ;;
  *)
    echo "lint-llm-diff: '$argument' is not one of this tier's options; pass --skip-nx-cache to force one fresh judgement" >&2
    exit 2
    ;;
  esac
done

# Nx scores a runtime input that exits non-zero as *no contribution* rather than as
# an error, so a fingerprint it cannot produce would silently shrink the key to the
# tree and the base. Refuse here instead, while a stale verdict can still be kept
# from replaying. This resolves llmlint the way the judged target does, so it is
# also where a missing toolchain is named.
bash scripts/llmlint-fingerprint.sh >/dev/null || {
  echo "lint-llm-diff: refusing to judge without the judge-configuration fingerprint named above; the cache key would drop it and replay a verdict that configuration has moved on from" >&2
  exit 2
}

base_sha="$(git rev-parse --verify --quiet "${base_ref}^{commit}")" || {
  echo "lint-llm-diff: '$base_ref' does not resolve to a commit; fetch it or pass an existing base" >&2
  exit 2
}

if [ -n "${NX_SKIP_NX_CACHE:-}${NX_DISABLE_NX_CACHE:-}" ]; then
  echo "lint-llm-diff: ignoring the ambient global Nx cache skip; force a fresh judgement of this tier alone with 'just lint-llm-diff $base_ref --skip-nx-cache'" >&2
fi
unset NX_SKIP_NX_CACHE NX_DISABLE_NX_CACHE

# The report is captured to read the provenance off it, and each half is replayed
# to the stream it came from: a run with findings has to leave its diagnostics on
# stderr, not folded into the verdict on stdout.
captured="$(mktemp -d)" || {
  echo "lint-llm-diff: could not open temporary storage for the judge report; free disk space and retry" >&2
  exit 3
}
trap 'rm -rf "$captured"' EXIT

status=0
LLMLINT_DIFF_BASE_SHA="$base_sha" ONEPIPELINE_NX_SHOW_OUTPUT=1 \
  bash scripts/nx.sh run onepipeline:lint-llm-diff "$@" \
  >"$captured/out" 2>"$captured/err" || status=$?
# Colour is not cosmetic to what is read below: Nx wraps its own lines, and the
# words inside them, in escapes whenever it thinks the terminal takes colour —
# which includes every run nested inside another Nx task, and an unstripped match
# reports each replay as a fresh judgement. What is read is a stripped copy; what
# is replayed keeps whatever the judge chose to say.
escape="$(printf '\033')"
plain="$captured/plain"
sed "s/${escape}\[[0-9;]*[a-zA-Z]//g" "$captured/out" "$captured/err" >"$plain"
# Provenance is Nx's own cache reporting: the annotation on the task line, or the
# summary line it prints only when it replayed a task instead of running it. Both
# are read, because only the first is safe at any size — Nx replays a hit as one
# burst, so a large replay can arrive truncated and its summary never does.
if grep -qE '^Nx read the output from the cache instead of running the command|^> nx run onepipeline:lint-llm-diff +\[existing outputs match the cache' "$plain"; then
  provenance="replayed the recorded verdict for base $base_sha (Nx cache hit)"
else
  provenance="judged this diff against base $base_sha (Nx cache miss)"
fi

if ((status != 0)); then
  # A failure is the whole report, on the streams it arrived on: the operator has
  # to act on it, and it is never cached, so it never has to survive a replay.
  cat "$captured/out"
  cat "$captured/err" >&2
  echo "lint-llm-diff: $provenance" >&2
  exit "$status"
fi

# A pass is one line, like every other recipe here: Nx's orchestration chatter is
# not this tier's answer. The answer is the verdict line the task itself produced,
# which is what Nx stored and what it replays — so a fresh run and a replayed one
# say the same thing, which is the whole claim this cache makes.
#
# A task that succeeded without producing one has not certified anything, whatever
# its status said, so it is refused rather than reported as a pass.
verdict="$(grep -m1 '^lint-llm-diff: ' "$plain")" || {
  echo "lint-llm-diff: the judged run reported no verdict for base $base_sha; rerun with --skip-nx-cache, and if it stays empty run 'bash scripts/llmlint-judge.sh' with LLMLINT_DIFF_BASE_SHA set to see what it did" >&2
  exit 2
}
echo "lint-llm-diff: $provenance — ${verdict#lint-llm-diff: }" >&2
exit "$status"
