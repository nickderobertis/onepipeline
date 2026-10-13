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

if ! rm -rf scratch; then
  echo "maintain: cannot remove scratch in $(pwd); check that nothing holds the slot's worktree read-only" >&2
  exit 1
fi

if [ -f grow.bytes ]; then
  if ! head -c "$(cat grow.bytes)" /dev/urandom >grown.bin; then
    echo "maintain: cannot write $(cat grow.bytes) bytes into grown.bin in $(pwd); check the free space under ONEVCS_HOME" >&2
    exit 1
  fi
fi

exit "${1:-0}"
