#!/usr/bin/env bash
# The local profile the report reads: each recipe Nx-cold (the clone's .nx
# removed), then Nx-warm from what that run left, one after another.
#
#   spike/onepipeline-ci/profile-chain.sh <work-dir> <standalone-clone>
#
# Outputs land in <work-dir>/out/<label>.{json,log,runs/,terminal/}. Writes
# <work-dir>/chain.exit when done. Do not change the clone while it runs: the
# warm runs are only warm against the exact inputs the cold run hashed.
set -uo pipefail
work=$1 clone=$2
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
mkdir -p "$work/out"
pair() {
  local name=$1
  shift
  df -h /home | tail -1 >"$work/out/$name.disk"
  "$here/local-profile.sh" "$name-cold" "$clone" "$work/out" cold "$@"
  echo "$name-cold exit $?" >>"$work/chain.status"
  "$here/local-profile.sh" "$name-warm" "$clone" "$work/out" warm "$@"
  echo "$name-warm exit $?" >>"$work/chain.status"
  du -sh "$clone/.nx/cache" >>"$work/out/$name.disk"
}
# `gate` runs `check` as its dependency recipe, so each gate run's log also
# times a `check` of the same Nx state, up to its `check: ok` line.
pair gate gate
pair check-affected check-affected
echo 0 >"$work/chain.exit"
