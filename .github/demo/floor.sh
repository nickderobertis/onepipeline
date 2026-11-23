#!/usr/bin/env bash
# DEMONSTRATION ONLY: time the process-start floor with each debug binary this job built.
set -uo pipefail
mkdir -p target/demo
status=0
for bin in target/debug/onepipeline target/debug/onepipeline.exe target/llvm-cov-target/debug/onepipeline; do
  [ -f "$bin" ] || continue
  node .github/demo/spawn-floor.mjs "target/demo/floor-$(printf '%s' "$bin" | tr '/' '-').json" "$bin" 200 || status=$?
done
exit "$status"
