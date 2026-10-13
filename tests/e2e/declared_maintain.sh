#!/bin/sh
# A repository's own maintenance command for the declared-maintenance journeys in
# `maintenance.rs`: committed into the identity's repository as `tools/maintain.sh`
# and named by that repository's `onevcs.toml`, so it reaches each slot's worktree
# the way a real repository's script does, and `onevcs pool maintain` runs it there
# with no shell.
#
# It removes `scratch/`, the ordinary files a journey wrote into the worktree. Where
# the worktree holds `grow.bytes`, it then writes that many bytes into `grown.bin`,
# so a journey can see a slot whose maintenance grew it. Given an argument, it exits
# with that status after its work, so a journey can see a declared command that
# failed recorded as one.
set -u

if [ "$#" -gt 1 ]; then
  echo "maintain: takes at most one argument, the status to exit with" >&2
  exit 64
fi

status=${1:-0}
case "$status" in
  '' | *[!0-9]* | ????*)
    echo "maintain: the status to exit with is '$status', which is not a whole number from 0 to 255; declare one such as 3, or no argument for a maintenance that succeeds" >&2
    exit 64
    ;;
esac
if [ "$status" -gt 255 ]; then
  echo "maintain: the status to exit with is '$status', which is above 255 and would wrap; declare one from 0 to 255, such as 3" >&2
  exit 64
fi

if ! rm -rf scratch; then
  echo "maintain: cannot remove scratch in $(pwd); check that nothing holds the slot's worktree read-only" >&2
  exit 1
fi

if [ -f grow.bytes ]; then
  grow=$(cat grow.bytes)
  case "$grow" in
    '' | *[!0-9]*)
      echo "maintain: grow.bytes in $(pwd) holds '$grow', which is not a whole number of bytes; write digits only, such as 41943040" >&2
      exit 64
      ;;
  esac
  if ! head -c "$grow" /dev/urandom >grown.bin; then
    echo "maintain: cannot write $grow bytes into grown.bin in $(pwd); check the free space under ONEVCS_HOME" >&2
    exit 1
  fi
fi

if [ "$status" -ne 0 ]; then
  echo "maintain: exiting $status because the declaration passes $status as this script's argument, asking for a maintenance that fails after its work; declare no argument for one that succeeds" >&2
fi
exit "$status"
