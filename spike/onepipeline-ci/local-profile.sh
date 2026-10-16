#!/usr/bin/env bash
# Time one `just` recipe of this repository on this host, Nx-cold or Nx-warm.
#
#   spike/onepipeline-ci/local-profile.sh <label> <nx-cache-dir> <out-dir> <recipe> [args...]
#
# <nx-cache-dir> holds both the Nx cache (NX_CACHE_DIRECTORY=<dir>/cache) and
# Nx's task database (NX_WORKSPACE_DATA_DIRECTORY=<dir>/workspace-data). Both
# are needed: in a git worktree Nx otherwise shares the main clone's cache and
# database across every worktree, and the database alone answers cache hits. An
# empty directory is a cold run, and the directory a previous identical run
# filled is a warm one; the checkout's own and the clone's `.nx` are untouched. The cargo target
# directory is the checkout's own and is left as it stands (CI restores it
# through rust-cache, so neither run is cargo-cold).
#
# Writes <out-dir>/<label>.log (the recipe's whole output, with nextest's
# per-test PASS lines, since NEXTEST_STATUS_LEVEL=pass), and <label>.json: the
# wall clock, the exit status, host load, and every Nx task the recipe ran, with
# its cache status. Nx writes those to `<nx-cache-dir>/run.json` once per
# invocation and the next invocation overwrites it, so a watcher this script
# starts (and stops by its own PID) snapshots each one into <out-dir>/<label>.runs/.
set -uo pipefail
label=$1 cache=$2 out=$3
shift 3
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
root="$(cd "$here/../.." && pwd)"
mkdir -p "$cache/cache" "$cache/workspace-data" "$out/$label.runs"
rm -f "$cache/cache/run.json"
cd "$root" || exit 1
python3 -I "$here/nx-run-watch.py" "$cache/cache/run.json" "$out/$label.runs" &
watcher=$!
load_before=$(cut -d' ' -f1-3 /proc/loadavg)
start_ms=$(date +%s%3N)
NX_CACHE_DIRECTORY="$cache/cache" NX_WORKSPACE_DATA_DIRECTORY="$cache/workspace-data" NEXTEST_STATUS_LEVEL=pass ONEPIPELINE_NX_SHOW_OUTPUT=1 \
  just "$@" >"$out/$label.log" 2>&1
status=$?
end_ms=$(date +%s%3N)
load_after=$(cut -d' ' -f1-3 /proc/loadavg)
sleep 2
kill "$watcher"
python3 -I "$here/nx-run-watch.py" --summarize "$out/$label.runs" "$start_ms" "$end_ms" "$status" \
  "$label" "$load_before" "$load_after" "$cache" "$*" >"$out/$label.json"
exit "$status"
