#!/bin/sh
# The host's `maintain` command for the journeys in `maintenance.rs`: what a
# `workspaces.yml` names under `maintain.command`, run by `onevcs pool maintain`
# in an idle slot's worktree with no shell.
#
# It leaves one line in `maintained.log` in the directory it is started in —
# the slot's worktree — which is the marker a journey reads to say the slot was
# maintained, and how many times. Given a path as its one argument it waits for
# that file to exist before writing, for up to 300 seconds, so a journey can hold
# a sweep open long enough to look at it from outside; a hold nobody releases
# ends as a maintenance that failed rather than one that never ended, which the
# sibling's own `timeout` would otherwise decide. `ONEPIPELINE_E2E_MAINTAIN_EXIT`
# names the status it exits with after writing, `0` unset, so a journey can see
# a command that failed recorded as one.
set -u

if [ "$#" -gt 1 ]; then
  echo "maintain: takes at most one argument, the path to wait for" >&2
  exit 64
fi

if [ "$#" -eq 1 ]; then
  deadline=$(( $(date +%s) + 300 ))
  until [ -f "$1" ]; do
    if [ "$(date +%s)" -ge "$deadline" ]; then
      echo "maintain: nothing wrote $1 within 300 seconds; the journey holding this sweep releases it by writing that path, so a hold this long is a journey that ended without releasing it — let the sibling record the failure and read the journey's own panic" >&2
      exit 1
    fi
    sleep 0.05
  done
fi

if ! printf 'maintained in %s at %s\n' "$(pwd)" "$(date +%s)" >>maintained.log; then
  echo "maintain: cannot write maintained.log in $(pwd); this runs in the slot's worktree, which onevcs cut under ONEVCS_HOME, so check that state root is on a writable mount and that nothing holds the worktree read-only" >&2
  exit 1
fi
echo "maintain: wrote maintained.log in $(pwd)"
exit "${ONEPIPELINE_E2E_MAINTAIN_EXIT:-0}"
