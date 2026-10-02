#!/usr/bin/env bash
# Run the nextest command given, unchanged, and when it exits non-zero name its
# exit status on stderr.
#
# Nothing else is touched: no pipe, so a terminal still gets nextest's progress
# bar, and the status this exits with is nextest's own. The one added line is
# what `scripts/rerun-failed.sh` reads when nextest ends a run with tests unrun
# and names no reason, because nextest itself never prints the code it exits with.
set -uo pipefail

if [ "$#" -eq 0 ]; then
  echo "usage: nextest-run.sh <nextest command...>" >&2
  exit 2
fi

"$@"
status=$?
if [ "$status" -ne 0 ]; then
  echo "nextest-run: nextest exited with status $status" >&2
fi
exit "$status"
