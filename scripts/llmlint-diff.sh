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
# Exits 0 on a clean verdict, 1 when the judge ruled against this diff, and 2 when
# it was asked for something it cannot judge — including a judge toolchain that
# never reached a verdict, which is llmlint's own meaning for that code.
#
# llmlint: ignore-file[tool_output_is_signal] The judge's report is this tier's
# product — Nx replays this task's terminal output in place of a verdict record —
# so it is handed through whole, with one line of provenance added rather than a
# summary substituted for it.
set -euo pipefail

# llmlint: ignore-block[changed_behavior_has_e2e] Reaching this needs a checkout
# whose own directory cannot be entered while this script is still readable inside
# it, which no journey can arrange without root: every other path through this file
# is driven in npm/test/llmlint-cache.test.mjs.
CDPATH='' cd -- "$(dirname -- "$0")/.." || {
  echo "lint-llm-diff: could not enter the repository from this script; reinstall the checkout and retry" >&2
  exit 1
}
# llmlint: ignore-end[changed_behavior_has_e2e]
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
  --skip-nx-cache | --verbose) ;;
  *)
    echo "lint-llm-diff: '$argument' is not one of this tier's options; pass --skip-nx-cache to force one fresh judgement, or --verbose for Nx's own detail" >&2
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
  exit 1
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
  exit 1
}
trap 'rm -rf "$captured"' EXIT

status=0
LLMLINT_DIFF_BASE_SHA="$base_sha" ONEPIPELINE_NX_SHOW_OUTPUT=1 \
  LLMLINT_JUDGE_STATUS_FILE="$captured/judge-status" \
  bash scripts/nx.sh run onepipeline:lint-llm-diff "$@" \
  >"$captured/out" 2>"$captured/err" || status=$?
# Nx collapses every failed task to 1, which would say the same thing about a diff
# the judge ruled on and a diff it never reached. The judge records its own status
# beside the report when it runs, so the two stay distinguishable; a replayed run
# records nothing, and there is nothing to distinguish.
if ((status != 0)) && judged="$(cat "$captured/judge-status" 2>/dev/null)" &&
  [[ "$judged" =~ ^[0-9]+$ ]] && ((judged != 0)); then
  status=$judged
fi
cat "$captured/out"
cat "$captured/err" >&2

# Provenance is Nx's own cache reporting: the annotation on the task line, or the
# summary line it prints only when it replayed a task instead of running it. Both
# are read because only the first is safe at any size — Nx replays a hit as one
# burst, so a large replay can arrive truncated and its summary never does. Colour
# is stripped first: Nx wraps those lines, and the words inside them, in escapes
# whenever it thinks the terminal takes colour — which includes every run nested
# inside another Nx task — and an unstripped match reports each replay as a fresh
# judgement.
escape="$(printf '\033')"
if sed "s/${escape}\[[0-9;]*[a-zA-Z]//g" "$captured/out" "$captured/err" |
  grep -qE '^Nx read the output from the cache instead of running the command|^> nx run onepipeline:lint-llm-diff +\[(local cache|remote cache|existing outputs match the cache)'; then
  echo "lint-llm-diff: replayed the recorded verdict for base $base_sha (Nx cache hit)" >&2
else
  echo "lint-llm-diff: judged this diff against base $base_sha (Nx cache miss)" >&2
fi
exit "$status"
