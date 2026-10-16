#!/usr/bin/env bash
# Re-take every measurement the spike report cites, by calling measure.sh once
# per configuration: first each configuration under load (--load on, LOADED_RUNS
# runs, LOADED_PARALLEL configurations at a time, so every loaded measurement
# shares the same heavier-than-nproc regime), then each idle (--load off,
# IDLE_RUNS runs, one at a time with nothing else of this harness running).
# Results are appended, one tab-separated line per measurement, to RECORD.
#
#   spike/nx-output-loss/matrix.sh [RECORD]
#
# LOADED_RUNS (200), IDLE_RUNS (50) and LOADED_PARALLEL (3) override the defaults.
set -euo pipefail
here="$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd)"
record="${1:-${ONEPIPELINE_NODE_SCRATCH_DIR:-${TMPDIR:-/tmp}}/nxloss/results.tsv}"
loaded_runs="${LOADED_RUNS:-200}" idle_runs="${IDLE_RUNS:-50}" parallel="${LOADED_PARALLEL:-3}"
mkdir -p "$(dirname "$record")"

configs=()
for terminal in tty ci; do
  for invoke in nx nxsh nxsh-show; do
    for pty in default false true; do
      configs+=("--arrangement onepipeline --invoke $invoke --pty $pty --terminal $terminal")
    done
  done
done
for invoke in nx nxsh nxsh-show; do
  configs+=("--arrangement onepipeline --invoke $invoke --terminal notty")
done
configs+=(
  "--arrangement onepipeline --invoke nxsh --terminal ci --nx-patch close"
  "--arrangement onepipeline --invoke nx --terminal ci --nx-patch close"
  "--arrangement onepipeline --invoke nxsh-show --terminal ci --mode replay"
)
for terminal in notty ci; do
  for invoke in nx nxsh; do
    configs+=("--arrangement ai-orchestrator --invoke $invoke --terminal $terminal")
  done
  configs+=("--arrangement ai-orchestrator --invoke nxsh --terminal $terminal --mode replay")
done

# Prepare every workspace copy once, serially, before any measurement runs: one
# per concurrent slot, so no two measurements share a workspace.
for ((slot = 0; slot < parallel; slot++)); do
  "$here/measure.sh" --arrangement onepipeline --slot "$slot" --runs 1 >/dev/null
  "$here/measure.sh" --arrangement onepipeline --nx-patch close --slot "$slot" --runs 1 >/dev/null
  "$here/measure.sh" --arrangement ai-orchestrator --slot "$slot" --runs 1 >/dev/null
done

logdir="$(dirname "$record")/matrix-logs"
mkdir -p "$logdir"
# Each slot works through its own share of the configurations, one at a time.
for ((slot = 0; slot < parallel; slot++)); do
  (
    n=0
    for config in "${configs[@]}"; do
      n=$((n + 1))
      ((n % parallel == slot)) || continue
      # shellcheck disable=SC2086
      "$here/measure.sh" $config --slot "$slot" --load on --runs "$loaded_runs" --record "$record" >"$logdir/loaded-$n.log" 2>&1 || true
    done
  ) &
done
wait
n=0
for config in "${configs[@]}"; do
  n=$((n + 1))
  # shellcheck disable=SC2086
  "$here/measure.sh" $config --load off --runs "$idle_runs" --record "$record" >"$logdir/idle-$n.log" 2>&1 || true
done
echo "matrix: ${#configs[@]} configurations measured; results in $record"
