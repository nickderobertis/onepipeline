#!/usr/bin/env bash
# Fingerprint the llmlint judge configuration for Nx's cache key.
#
# Declared as the `lint-llm-diff` target's `runtime` input, so a recorded verdict
# is invalidated by anything that changes what the judge would ask — including the
# two things no tracked file records: the *installed* llmlint version, and the
# resolved content of a plugin pinned in `llmlint.yml` but fetched from outside
# this repository. `llmlint config` prints the effective merged config — this
# repository's `llmlint.yml` plus every plugin's resolved rules — so one hash
# covers all of them.
#
# `just lint-llm-diff` runs this itself before handing the tier to Nx, and refuses
# when it fails. That is not belt and braces: Nx scores a runtime input that exits
# non-zero as *no contribution* rather than as an error, so a fingerprint that
# cannot be produced would silently shrink the key to the tree and the base commit
# and replay a verdict the judge configuration has since moved on from.
#
# Two host details are kept out, so the digest describes the judge configuration
# rather than the machine it was resolved on. The checkout path is folded to a
# placeholder, so two checkouts of this repository agree. `LLMLINT_ONEHARNESS_BIN`
# is cleared for the config call, so what is hashed is the harness binding this
# checkout declares — `llmlint.yml` pins none, so it renders as `null` — rather
# than whichever wrapper the calling environment happens to export. That value
# names where the harness binary lives, not what the judge is asked, and reading
# it would give one judged diff a different key per dispatch, which is the split
# verdict this cache exists to end. The judged run still honours it: a host that
# has to reach its harness through a wrapper keeps doing so.
#
# Run it by hand to see the current judge fingerprint — the answer to "why did the
# cache miss when nothing in the tree changed?". It exits 2 when the toolchain
# cannot answer what the judge would ask, and 1 when this checkout cannot be read
# at all; the two say which of the toolchain or the checkout to repair.
set -euo pipefail

# llmlint: ignore-block[changed_behavior_has_e2e] Reaching this needs a checkout
# whose own directory cannot be entered while this script is still readable inside
# it, which no journey can arrange without root: every other path through this file
# is driven in npm/test/llmlint-cache.test.mjs.
CDPATH='' cd -- "$(dirname -- "$0")/.." || {
  echo "llmlint fingerprint: could not enter the repository from this script; reinstall the checkout and retry" >&2
  exit 1
}
# llmlint: ignore-end[changed_behavior_has_e2e]
root=$PWD
# shellcheck source=scripts/llmlint-runtime-env.sh
. "$root/scripts/llmlint-runtime-env.sh" || {
  echo "llmlint fingerprint: could not load the shared runtime environment; restore scripts/llmlint-runtime-env.sh and retry" >&2
  exit 1
}
llmlint_runtime_env

# Both answers are external input, and both are cache-key material: an empty one
# would hash to a fingerprint that says nothing about the judge configuration and
# replay a verdict it has moved on from, so emptiness is refused rather than hashed.
version="$(llmlint --version)" || {
  echo "llmlint fingerprint: 'llmlint --version' failed; run 'just setup-llmlint' and retry" >&2
  exit 2
}
[ -n "${version//[[:space:]]/}" ] || {
  echo "llmlint fingerprint: 'llmlint --version' answered nothing; reinstall the toolchain with 'just setup-llmlint' and retry" >&2
  exit 2
}
config="$(env -u LLMLINT_ONEHARNESS_BIN llmlint config)" || {
  echo "llmlint fingerprint: 'llmlint config' failed; repair llmlint.yml or its plugin pins and retry" >&2
  exit 2
}
[ -n "${config//[[:space:]]/}" ] || {
  echo "llmlint fingerprint: 'llmlint config' answered nothing; repair llmlint.yml or its plugin pins and retry" >&2
  exit 2
}
config="${config//"$root"/\{root\}}"

# `sha256sum` on Linux, `shasum` where coreutils is not the default — the tier is
# reachable from `just gate` on every platform a contributor develops on.
if command -v sha256sum >/dev/null 2>&1; then
  digest="$(printf '%s\n%s\n' "$version" "$config" | sha256sum)"
elif command -v shasum >/dev/null 2>&1; then
  digest="$(printf '%s\n%s\n' "$version" "$config" | shasum -a 256)"
else
  echo "llmlint fingerprint: no sha256 tool found; install coreutils (sha256sum) or perl (shasum) and retry" >&2
  exit 2
fi
printf '%s\n' "${digest%% *}"
