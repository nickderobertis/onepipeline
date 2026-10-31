#!/usr/bin/env bash
# The visual-docs capture `lint` runs beside its other checks rather than after
# them: `start` launches `screenshots/ci-capture.sh` once per lane in $SHOTS_LANES,
# one after another, in the background, and returns; `wait` blocks until they
# have finished, prints what they said, and exits with their status. The two
# halves share DIR, where `start` writes `capture.log` and `capture.exit`.
#
#   SHOTS_LANES="shots/current/x86_64 shots/verify/x86_64" ci-capture-lanes.sh start DIR
#   ci-capture-lanes.sh wait DIR
#
# Exit: `start` 0 once launched; `wait` the first failing lane's status, or 0;
# 2 for a refused call.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

refuse() {
  echo "ci-capture-lanes: $1" >&2
  echo "ACTION: run 'SHOTS_LANES=\"<lane>...\" bash screenshots/ci-capture-lanes.sh start <dir>', then 'bash screenshots/ci-capture-lanes.sh wait <dir>'" >&2
  exit 2
}

[ "$#" -eq 2 ] || refuse "takes a mode and a directory, not $# argument(s)"
mode="$1"
dir="$2"
[ -d "$dir" ] || refuse "'$dir' is not a directory"

case "$mode" in
  start)
    [ -n "${SHOTS_LANES:-}" ] || refuse "SHOTS_LANES names no lane to capture"
    rm -f -- "$dir/capture.exit"
    # One status for the whole set: the first lane that fails stops the rest,
    # because a reproducibility lane after a failed capture compares nothing.
    nohup bash -c '
      status=0
      for lane in $SHOTS_LANES; do
        SHOTS_OUT="$lane" bash screenshots/ci-capture.sh || { status=$?; break; }
      done
      echo "$status" > "$1/capture.exit"
    ' _ "$dir" >"$dir/capture.log" 2>&1 </dev/null &
    echo "ci-capture-lanes: capturing ${SHOTS_LANES} in the background (log: $dir/capture.log)"
    ;;
  wait)
    [ -e "$dir/capture.log" ] || refuse "no capture was started into '$dir'"
    until [ -f "$dir/capture.exit" ]; do sleep 5; done
    cat "$dir/capture.log"
    status="$(cat "$dir/capture.exit")"
    if [ "$status" != 0 ]; then
      echo "ci-capture-lanes: the capture failed (exit $status); its output is above" >&2
      echo "ACTION: fix what it reports, then re-run 'SHOTS_OUT=<lane> bash screenshots/ci-capture.sh' for the lane that failed" >&2
    fi
    exit "$status"
    ;;
  *) refuse "'$mode' is not a mode; it is start or wait" ;;
esac
