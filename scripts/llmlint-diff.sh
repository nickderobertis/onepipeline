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
# Exits 0 on a clean verdict and 1 when the judge ruled against this diff, which
# are llmlint's own meanings. 2 is what it could not be asked — a base or an option
# it cannot use, or a judge toolchain that never reached a verdict. 3 is this
# checkout or host being unable to support a run at all; nothing about the diff.
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
  exit 3
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

#: The line the judge states its own exit status on, which this reads and keeps out
#: of what it replays. `scripts/llmlint-judge.sh` writes the same string.
readonly JUDGE_STATUS_MARKER="lint-llm-diff: judge exit status"

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
# Nx collapses every failed task to 1, which would say the same thing about a diff
# the judge ruled against and a diff it never reached. The judge states its own
# status on the line below, which travels *with* the report rather than beside it,
# so a replayed verdict carries its status as well as its text.
judged="$(sed -n "s/^${JUDGE_STATUS_MARKER} \([0-9][0-9]*\)$/\1/p" "$captured/err" | tail -1)"
[ -z "$judged" ] || status=$judged
cat "$captured/out"
grep -v "^${JUDGE_STATUS_MARKER} " "$captured/err" >&2 || true

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
  grep -qE '^Nx read the output from the cache instead of running the command|^> nx run onepipeline:lint-llm-diff +\[existing outputs match the cache'; then
  echo "lint-llm-diff: replayed the recorded verdict for base $base_sha (Nx cache hit)" >&2
else
  echo "lint-llm-diff: judged this diff against base $base_sha (Nx cache miss)" >&2
fi
exit "$status"
