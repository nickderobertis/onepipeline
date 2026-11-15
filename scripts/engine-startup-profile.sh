#!/usr/bin/env bash
# Local attribution only (manager c13.0); hosted comparisons use the existing CI trace.
set -euo pipefail
if [ "$#" -ne 2 ]; then
  echo 'usage: bash scripts/engine-startup-profile.sh E2E_BINARY OUTPUT_DIR (requires samply and permission to sample)' >&2
  exit 2
fi
binary="$(realpath "$1")"
out="$2"
mkdir -p "$out"
out="$(realpath "$out")"
journey=driver::a_driver_that_died_mid_dispatch_is_adoptable_and_the_fresh_one_finishes_it
command -v samply >/dev/null || { echo 'install samply before recording' >&2; exit 2; }
git rev-parse HEAD > "$out/source-commit.txt"
git diff HEAD > "$out/source.diff"
export ONEPIPELINE_LAUNCH_TRACE="$out/launch"
export NEXTEST_TEST_NAME="$journey"
mkdir -p "$ONEPIPELINE_LAUNCH_TRACE"
samply record --save-only --unstable-presymbolicate --rate 2000 \
  --output "$out/profile.json.gz" -- "$binary" --exact "$journey" --nocapture
