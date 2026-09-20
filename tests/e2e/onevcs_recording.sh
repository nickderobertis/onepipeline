#!/bin/sh
# The `onevcs` executable the journeys in `maintenance.rs` point the engine at
# through `ONEPIPELINE_ONEVCS_BIN`: it records the verb it was asked — one line
# per call, the arguments as given — into the file `ONEPIPELINE_E2E_ONEVCS_CALLS`
# names, and then runs the real binary `ONEPIPELINE_E2E_ONEVCS_REAL` names with
# exactly those arguments, answering whatever it answers.
#
# Nothing is substituted: every answer is the linked release's own. What this
# adds is the sibling's call record, which is how a journey tells one sweep of
# the registry from two without a clock on either side.
set -u

if [ -z "${ONEPIPELINE_E2E_ONEVCS_CALLS-}" ] || [ -z "${ONEPIPELINE_E2E_ONEVCS_REAL-}" ]; then
  echo "onevcs_recording: ONEPIPELINE_E2E_ONEVCS_CALLS and ONEPIPELINE_E2E_ONEVCS_REAL must both be set; maintenance.rs sets them on the world" >&2
  exit 64
fi
if ! printf '%s\n' "$*" >>"$ONEPIPELINE_E2E_ONEVCS_CALLS"; then
  echo "onevcs_recording: cannot append to $ONEPIPELINE_E2E_ONEVCS_CALLS; point ONEPIPELINE_E2E_ONEVCS_CALLS at a file under the world's own scratch root, the way maintenance.rs does, and check that root is on a writable mount" >&2
  exit 1
fi
if [ ! -x "$ONEPIPELINE_E2E_ONEVCS_REAL" ]; then
  echo "onevcs_recording: ONEPIPELINE_E2E_ONEVCS_REAL=$ONEPIPELINE_E2E_ONEVCS_REAL is not an executable; point it at the real onevcs binary the suite built, the way maintenance.rs does with harness::onevcs_binary()" >&2
  exit 64
fi
exec "$ONEPIPELINE_E2E_ONEVCS_REAL" "$@"
