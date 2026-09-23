#!/bin/sh
# The host's `maintain` command for the journeys in `maintenance.rs`: what a
# `workspaces.yml` names under `maintain.command`, run by `onevcs pool maintain`
# in an idle slot's worktree with no shell.
#
# It leaves one line in `maintained.log` in the directory it is started in —
# the slot's worktree — which is the marker a journey reads to say the slot was
# maintained, and how many times. The line carries **the arguments it was given**,
# so a journey can read back what reached the spawn: `maintain.command` is an argv
# whose first element is the program and whose rest are its arguments, and the one
# way to see the rest arrived in order is to have the program say so.
#
# Its first argument is a path to wait for before writing, for up to 300 seconds,
# so a journey can hold a sweep open long enough to look at it from outside; the
# empty string is no wait, which is also how a journey names an argument that is
# the empty string and sees it come through. Every further argument is recorded
# and nothing else. A hold nobody releases ends as a maintenance that failed
# rather than one that never ended, which the sibling's own `timeout` would
# otherwise decide. `ONEPIPELINE_E2E_MAINTAIN_EXIT` names the status it exits with
# after writing, `0` unset, so a journey can see a command that failed recorded as
# one.
set -u

if [ "$#" -ge 1 ] && [ -n "$1" ]; then
  deadline=$(( $(date +%s) + 300 ))
  until [ -f "$1" ]; do
    if [ "$(date +%s)" -ge "$deadline" ]; then
      echo "maintain: nothing wrote $1 within 300 seconds, so this hold expired and the sibling records the maintenance as failed. Write that path to release a hold you meant to keep; a hold that ran this long is a journey that ended without releasing it, so read the journey's own panic for why it stopped" >&2
      exit 1
    fi
    sleep 0.05
  done
fi

if ! printf 'maintained in %s at %s with [%s]\n' "$(pwd)" "$(date +%s)" "$*" >>maintained.log; then
  echo "maintain: cannot write maintained.log in $(pwd); this runs in the slot's worktree, which onevcs cut under ONEVCS_HOME, so check that state root is on a writable mount and that nothing holds the worktree read-only" >&2
  exit 1
fi
echo "maintain: wrote maintained.log in $(pwd)"
# `ONEPIPELINE_E2E_MAINTAIN_EXIT` reaches `exit`, so it is checked before it gets
# there. A value that is not a status is a fixture exiting on something the shell
# invented rather than on what a journey asked for: `exit 300` wraps round to 44,
# and a word exits 2 with a message from the shell. Refused rather than clamped or
# defaulted, and with no leading zero, for the reason `hook.sh`'s own ceiling
# gives — `maintain.bat` counts with `set /a`, which reads one as octal, so the two
# halves would exit differently on the same `010`. 64 is `sysexits.h`'s EX_USAGE,
# which is what the other fixture answers a caller it does not understand with.
asked=${ONEPIPELINE_E2E_MAINTAIN_EXIT:-0}
if [ "$asked" != "0" ]; then
  refused="maintain: ONEPIPELINE_E2E_MAINTAIN_EXIT holds '$asked', which is not a status this fixture can exit with. Set it to a whole number between 1 and 255 with no leading zero, or unset it for a maintenance that succeeds"
  case "$asked" in
    '' | 0* | *[!0-9]* | ????*)
      echo "$refused" >&2
      exit 64
      ;;
  esac
  if [ "$asked" -gt 255 ]; then
    echo "$refused" >&2
    exit 64
  fi
  echo "maintain: ONEPIPELINE_E2E_MAINTAIN_EXIT=$asked asks for a maintenance that fails after writing maintained.log, so this exits $asked and the sibling records a command that failed. Unset it for one that succeeds" >&2
  exit "$asked"
fi
