#!/usr/bin/env bash
# Measurement only, never lands: trace and fold two real publication-policy
# journeys in every mode. These local durations are not hosted performance data.
set -euo pipefail
if [ "$#" -ne 1 ]; then
  echo "usage: $0 NEW_OUTPUT_DIRECTORY (optionally set CARGO_TARGET_DIR)" >&2
  exit 2
fi
out=$(realpath -m "$1")
if [ -e "$out" ]; then
  echo "refusing to mix this slice with existing output: $out" >&2
  exit 1
fi
mkdir -p "$out"
filter='test(=destination::a_hook_git_would_skip_and_a_hook_that_cannot_start_are_told_apart) | test(=destination::a_rules_check_this_build_cannot_read_a_policy_off_leaves_the_node_unchecked)'
sides=()
for mode in stock batched native reuse reuse+batched reuse+native; do
  job="$out/$mode/local/test-linux"
  mkdir -p "$job/test-profile/launch" "$job/test-profile/e2e"
  ONEVCS_SPIKE_GIT_READS="$mode" ONEPIPELINE_LAUNCH_TRACE="$job/test-profile/launch" \
    cargo nextest run --locked --status-level pass --test e2e -E "$filter" 2>&1 |
    while IFS= read -r line; do
      printf '%s %s\n' "$(date -u +%Y-%m-%dT%H:%M:%S.%NZ)" "$line"
    done > "$job/job.log"
  for name in a_hook_git_would_skip_and_a_hook_that_cannot_start_are_told_apart a_rules_check_this_build_cannot_read_a_policy_off_leaves_the_node_unchecked; do
    cp "target/test-profile/e2e/destination.$name.json" "$job/test-profile/e2e/"
  done
  sides+=("$mode=$out/$mode")
done
node scripts/git-reads-compare.mjs compare "${sides[@]}" > "$out/comparison.md"
echo "Trace counts and fold: $out/comparison.md (local timings are not priced)"
