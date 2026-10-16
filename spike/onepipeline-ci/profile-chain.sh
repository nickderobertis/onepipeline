#!/usr/bin/env bash
# The local profile the report reads: each recipe Nx-cold into a fresh cache
# directory, then Nx-warm from the directory that run left, one after another.
#
#   spike/onepipeline-ci/profile-chain.sh <work-dir>
#
# Outputs land in <work-dir>/out/<label>.{json,log,runs/}; each pair's cache is
# removed once its warm run has read it, to keep the host's disk free. Writes
# <work-dir>/chain.exit when done. Do not change the tree while it runs: the
# warm runs are only warm against the exact inputs the cold run hashed.
set -uo pipefail
work=$1
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
mkdir -p "$work/out"
pair() {
  local name=$1
  shift
  local cache="$work/cache-$name"
  rm -rf "$cache"
  df -h /home | tail -1 >"$work/out/$name.disk"
  "$here/local-profile.sh" "$name-cold" "$cache" "$work/out" "$@"
  echo "$name-cold exit $?" >>"$work/chain.status"
  "$here/local-profile.sh" "$name-warm" "$cache" "$work/out" "$@"
  echo "$name-warm exit $?" >>"$work/chain.status"
  du -sh "$cache" >>"$work/out/$name.disk"
  rm -rf "$cache"
}
pair check check
pair check-affected check-affected
pair gate gate
echo 0 >"$work/chain.exit"
