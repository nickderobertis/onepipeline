#!/usr/bin/env bash
# Time one `just` recipe of a checkout on this host, Nx-cold or Nx-warm.
#
#   spike/onepipeline-ci/local-profile.sh <label> <checkout> <out-dir> cold|warm <recipe> [args...]
#
# <checkout> must be a standalone clone, not a git worktree: in a worktree Nx
# shares the main clone's cache and task database across every worktree, so
# nothing there is cold, and overriding NX_WORKSPACE_DATA_DIRECTORY instead
# leaks into the nested Nx runs `npm/test/workspace-nx.test.mjs` starts and
# fails them with "Recursive task invocation detected". Nothing about the
# recipe's environment is changed here.
#
# `cold` deletes <checkout>/.nx/cache and <checkout>/.nx/workspace-data first;
# `warm` keeps what the previous run left. The cargo target directory is the
# checkout's own and is left as it stands.
#
# Writes <out-dir>/<label>.log (the recipe's output; Nx keeps each task's own
# output under .nx/cache/terminalOutputs, copied to <label>.terminal/ after the
# run) and <label>.json: wall clock, the host's uptime clock over the same
# interval (a VM pause shows as wall clock the uptime clock did not see, and is
# flagged), exit status, host load, and every Nx task each invocation ran with
# its cache status. Nx writes those to .nx/cache/run.json once per invocation
# and the next overwrites it, so a watcher this script starts (and stops by its
# own PID) snapshots each into <out-dir>/<label>.runs/.
set -uo pipefail
label=$1 checkout=$2 out=$3 mode=$4
shift 4
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
case "$mode" in
  cold) rm -rf "$checkout/.nx/cache" "$checkout/.nx/workspace-data" ;;
  warm) ;;
  *) echo "mode must be cold or warm" >&2; exit 2 ;;
esac
mkdir -p "$checkout/.nx/cache" "$out/$label.runs"
rm -f "$checkout/.nx/cache/run.json"
cd "$checkout" || exit 1
python3 -I "$here/nx-run-watch.py" "$checkout/.nx/cache/run.json" "$out/$label.runs" &
watcher=$!
load_before=$(cut -d' ' -f1-3 /proc/loadavg)
up_before=$(cut -d' ' -f1 /proc/uptime)
start_ms=$(date +%s%3N)
just "$@" >"$out/$label.log" 2>&1
status=$?
end_ms=$(date +%s%3N)
up_after=$(cut -d' ' -f1 /proc/uptime)
load_after=$(cut -d' ' -f1-3 /proc/loadavg)
sleep 2
kill "$watcher"
rm -rf "$out/$label.terminal"
cp -r "$checkout/.nx/cache/terminalOutputs" "$out/$label.terminal" 2>/dev/null
python3 -I "$here/nx-run-watch.py" --summarize "$out/$label.runs" "$start_ms" "$end_ms" "$status" \
  "$label" "$load_before" "$load_after" "$mode" "$*" "$up_before" "$up_after" "$(git rev-parse HEAD)" >"$out/$label.json"
exit "$status"
