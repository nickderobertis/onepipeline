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
# `run_end_hook.bat` is the Windows half and answers the same way.
set -u

record=${ONEPIPELINE_E2E_HOOK_RECORD:?ONEPIPELINE_E2E_HOOK_RECORD names no directory to record into}
run=${ONEPIPELINE_RUN_ID:?the engine named no run}
mkdir -p "$record/$run" || exit 70

count=0
if [ -f "$record/$run/invocations" ]; then
  count=$(wc -l <"$record/$run/invocations")
fi
here="$record/$run/$((count + 1))"
mkdir -p "$here" || exit 70
printf '%s\n' "${ONEPIPELINE_HOOK-}" >>"$record/$run/invocations"

pwd >"$here/cwd"
cat >"$here/stdin"
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
} >"$here/env"

i=1
while [ "$i" -le 25 ]; do
  printf 'said line %s\n' "$i"
  i=$((i + 1))
done
printf 'said on stderr\n' >&2
: >"$here/started"

if [ -f "$record/$run.hold" ]; then
  deadline=$(($(date +%s) + 300))
  until [ -f "$record/$run.go" ]; do
    if [ "$(date +%s)" -ge "$deadline" ]; then
      printf 'nothing wrote %s within 300 seconds\n' "$record/$run.go" >&2
      exit 1
    fi
    sleep 0.05
  done
fi

status=0
if [ -f "$record/$run.exit" ]; then
  status=$(cat "$record/$run.exit")
fi
exit "$status"
