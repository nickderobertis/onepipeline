#!/bin/sh
# A run's **run-end hook**, for the journeys in `run_end_hooks.rs` — the command a
# launch names with `--success-hook` or `--failure-hook`. Not `hook.sh`, which is
# the body of a repository's git `pre-push` hook and is run by git rather than by
# the engine.
#
# The engine runs this with no arguments, so everything that varies per run is
# read from files keyed by the run it names:
#
#   $ONEPIPELINE_E2E_HOOK_RECORD/<run>.exit   the status to exit with (default 0)
#   $ONEPIPELINE_E2E_HOOK_RECORD/<run>.hold   present: wait for <run>.go before
#                                             exiting, for up to 300 seconds
#
# and what it was handed is recorded under
# $ONEPIPELINE_E2E_HOOK_RECORD/<run>/<n>/, one directory per invocation:
#
#   cwd       the directory it was started in
#   stdin     the document on its stdin
#   env       hook=, run_id=, run_root=, then launcher= and session= or the
#             words `launcher unset` / `session unset`
#   started   written last, once everything above is on disk
#
# with the hook's name appended to <run>/invocations each time. It says
# `said line 1` to `said line 25` on stdout and `said on stderr` on stderr, so a
# journey can read which of its output the engine kept.
#
# Every record is checked as it is written. The journeys read these files as the
# only witness to what the engine handed a hook, so one that could not be written
# is refused out loud — exit 70, naming the file and what to fix — rather than
# left missing for a journey to read as a hook that was never run.
#
# `run_end_hook.bat` is the Windows half and answers the same way.
set -u

# A record this fixture could not write, or a scripted value it could not use.
broke() {
  echo "run_end_hook: $1" >&2
  echo "run_end_hook: point ONEPIPELINE_E2E_HOOK_RECORD at a writable directory the journey owns, as run_end_hooks.rs does, and write <run>.exit as a whole number" >&2
  exit 70
}

record=${ONEPIPELINE_E2E_HOOK_RECORD:?ONEPIPELINE_E2E_HOOK_RECORD names no directory to record into; set it to the world scratch run_end_hooks.rs creates}
run=${ONEPIPELINE_RUN_ID:?the engine named no run in ONEPIPELINE_RUN_ID, which every run-end hook is given}
mkdir -p "$record/$run" || broke "cannot create $record/$run"

count=0
if [ -f "$record/$run/invocations" ]; then
  count=$(wc -l <"$record/$run/invocations") || broke "cannot read $record/$run/invocations"
fi
here="$record/$run/$((count + 1))"
mkdir -p "$here" || broke "cannot create $here"
printf '%s\n' "${ONEPIPELINE_HOOK-}" >>"$record/$run/invocations" \
  || broke "cannot append to $record/$run/invocations"

pwd >"$here/cwd" || broke "cannot write $here/cwd"
cat >"$here/stdin" || broke "cannot write $here/stdin"
{
  printf 'hook=%s\n' "${ONEPIPELINE_HOOK-}"
  printf 'run_id=%s\n' "$run"
  printf 'run_root=%s\n' "${ONEPIPELINE_RUN_ROOT-}"
  if [ "${ONEPIPELINE_LAUNCHER+set}" = set ]; then
    printf 'launcher=%s\n' "$ONEPIPELINE_LAUNCHER"
  else
    printf 'launcher unset\n'
  fi
  if [ "${ONEPIPELINE_LAUNCHER_SESSION+set}" = set ]; then
    printf 'session=%s\n' "$ONEPIPELINE_LAUNCHER_SESSION"
  else
    printf 'session unset\n'
  fi
} >"$here/env" || broke "cannot write $here/env"

# llmlint: ignore-block[tool_output_is_signal] this output is the fixture's evidence,
# not narration: the journeys read these 26 lines back out of the log the engine kept,
# off the attached driver's relayed stderr, and as the last 20 lines `results` repeats,
# so a hook that said nothing would leave all three of those claims unproven.
i=1
while [ "$i" -le 25 ]; do
  printf 'said line %s\n' "$i"
  i=$((i + 1))
done
printf 'said on stderr\n' >&2
# llmlint: ignore-end[tool_output_is_signal]
: >"$here/started" || broke "cannot write $here/started"

if [ -f "$record/$run.hold" ]; then
  deadline=$(($(date +%s) + 300))
  until [ -f "$record/$run.go" ]; do
    if [ "$(date +%s)" -ge "$deadline" ]; then
      echo "run_end_hook: nothing wrote $record/$run.go within 300 seconds; write it to release this hook" >&2
      exit 1
    fi
    sleep 0.05
  done
fi

status=0
if [ -f "$record/$run.exit" ]; then
  status=$(cat "$record/$run.exit") || broke "cannot read $record/$run.exit"
fi
case "$status" in
  '' | *[!0-9]*) broke "$record/$run.exit holds '$status', which is not an exit status" ;;
esac
exit "$status"
