#!/bin/sh
# A run's **dispatch-env hook**, for the journeys in `dispatch_env_hook.rs` — the
# command a launch names with `--dispatch-env-hook`, run immediately before every
# node-scope dispatch. Not `run_end_hook.sh`, which fires once when a run ends.
#
# The engine runs this with no arguments and nothing on its stdin, so everything
# that varies per run is read from files keyed by the run it names:
#
#   $ONEPIPELINE_E2E_HOOK_RECORD/<run>.stdout  the document to print, verbatim
#                                              (default `{"version":1,"env":{}}`)
#   $ONEPIPELINE_E2E_HOOK_RECORD/<run>.exit    the status to exit with (default 0)
#   $ONEPIPELINE_E2E_HOOK_RECORD/<run>.exit-from-nth
#                                              exit with <run>.exit only from this
#                                              invocation on, so a node's earlier
#                                              dispatches are accepted
#   $ONEPIPELINE_E2E_HOOK_RECORD/<run>.hold    present: wait for <run>.go before
#                                              printing, for up to 300 seconds
#
# and what it was handed is recorded under
# $ONEPIPELINE_E2E_HOOK_RECORD/<run>/<n>/, one directory per invocation:
#
#   cwd          the directory it was started in
#   env          hook=, run_id=, run_root=, node_id=
#   environment  every variable of the environment it was started with, as
#                `env` prints them — the driver's own, which the journeys read
#                to prove no earlier dispatch's additions leaked into it
#   stdin        what was on its stdin, which the engine hands none of
#   started      written last, once everything above is on disk
#
# with the node it ran for appended to <run>/invocations each time. It says
# `dispatch-env hook ran for <node>` on stderr, so a journey can read that the
# engine kept its stderr and nothing of its stdout.
#
# Every record is checked as it is written, for `run_end_hook.sh`'s reason: these
# files are the only witness to what the engine handed the hook, so one that
# could not be written is refused out loud — exit 70 — rather than left missing.
#
# `dispatch_env_hook.bat` is the Windows half and answers the same way.
set -u

# A record this fixture could not write, or a scripted value it could not use:
# the first line names the file and what is wrong with it, and the second says
# where the shape of every scripted file is written down.
broke() {
  echo "dispatch_env_hook: $1" >&2
  echo "dispatch_env_hook: point ONEPIPELINE_E2E_HOOK_RECORD at a writable directory the journey owns, as dispatch_env_hook.rs does, and write each <run>.* file in the shape the header of this script describes" >&2
  exit 70
}

record=${ONEPIPELINE_E2E_HOOK_RECORD:?ONEPIPELINE_E2E_HOOK_RECORD names no directory to record into; set it to the world scratch dispatch_env_hook.rs creates}
run=${ONEPIPELINE_RUN_ID:?the engine named no run in ONEPIPELINE_RUN_ID, which every dispatch-env hook is given; run this only as the command a launch names with --dispatch-env-hook}
node=${ONEPIPELINE_NODE_ID:?the engine named no node in ONEPIPELINE_NODE_ID, which every dispatch-env hook is given; run this only as the command a launch names with --dispatch-env-hook}
# The run id becomes one path segment under the record directory, so it is held to
# the characters a run id is minted from before anything is written under it.
case "$run" in
  '' | . | .. | *[!A-Za-z0-9._-]*)
    broke "ONEPIPELINE_RUN_ID holds '$run', which is not a single run id path segment"
    ;;
esac
mkdir -p "$record/$run" || broke "cannot create $record/$run"

# Which invocation this is, **claimed** rather than counted, for the reason
# `run_end_hook.sh` gives: two dispatches at once cannot both take one directory.
ceiling=100
nth=1
until mkdir "$record/$run/$nth" 2>/dev/null; do
  nth=$((nth + 1))
  if [ "$nth" -gt "$ceiling" ]; then
    if [ -d "$record/$run/$ceiling" ]; then
      broke "this run has already recorded $ceiling invocations under $record/$run, which is this fixture's ceiling; a journey that needs more raises it in both halves, which both_dispatch_env_hook_halves_number_an_invocation_the_same_way holds in step"
    fi
    broke "no record directory could be created under $record/$run, so nothing there is claimable"
  fi
done
here="$record/$run/$nth"
printf '%s\n' "$node" >>"$record/$run/invocations" \
  || broke "cannot append to $record/$run/invocations"

pwd >"$here/cwd" || broke "cannot write $here/cwd"
{
  printf 'hook=%s\n' "${ONEPIPELINE_HOOK-}"
  printf 'run_id=%s\n' "$run"
  printf 'run_root=%s\n' "${ONEPIPELINE_RUN_ROOT-}"
  printf 'node_id=%s\n' "$node"
} >"$here/env" || broke "cannot write $here/env"
env >"$here/environment" || broke "cannot write $here/environment"
cat >"$here/stdin" || broke "cannot write $here/stdin"
# llmlint: ignore-block[tool_output_is_signal] this line is the fixture's evidence rather
# than narration: the journeys read it back out of the log the engine kept, and read that
# the engine kept nothing of what stdout said there, so a hook that said nothing on stderr
# would leave both claims unproven.
printf 'dispatch-env hook ran for %s\n' "$node" >&2
# llmlint: ignore-end[tool_output_is_signal]
: >"$here/started" || broke "cannot write $here/started"

if [ -f "$record/$run.hold" ]; then
  deadline=$(($(date +%s) + 300))
  until [ -f "$record/$run.go" ]; do
    if [ "$(date +%s)" -ge "$deadline" ]; then
      echo "dispatch_env_hook: nothing wrote $record/$run.go within 300 seconds; write it to release this hook" >&2
      exit 1
    fi
    sleep 0.05
  done
fi

status=0
if [ -f "$record/$run.exit" ]; then
  from=1
  if [ -f "$record/$run.exit-from-nth" ]; then
    from=$(cat "$record/$run.exit-from-nth") || broke "cannot read $record/$run.exit-from-nth"
    case "$from" in
      '' | *[!0-9]*) broke "$record/$run.exit-from-nth holds '$from', which is not an invocation number" ;;
    esac
  fi
  if [ "$nth" -ge "$from" ]; then
    status=$(cat "$record/$run.exit") || broke "cannot read $record/$run.exit"
  fi
fi
# An exit status is 0 to 255; a longer number is refused before it is compared.
case "$status" in
  '' | *[!0-9]* | ????*) broke "$record/$run.exit holds '$status', which is not an exit status from 0 to 255" ;;
esac
if [ "$status" -gt 255 ]; then
  broke "$record/$run.exit holds '$status', which is not an exit status from 0 to 255"
fi

# The document, last: a hook that fails may still have printed one, which the
# engine must not read.
if [ -f "$record/$run.stdout" ]; then
  cat "$record/$run.stdout" || broke "cannot read $record/$run.stdout"
else
  printf '{"version":1,"env":{}}\n'
fi
exit "$status"
