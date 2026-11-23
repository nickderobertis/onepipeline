#!/usr/bin/env bash
# DEMONSTRATION ONLY: run the profiled sample again, one test at a time, after the
# job's own run, so each journey's time without the rest of the tier beside it is
# recorded. `coverage` runs it the way test-linux's e2e tier runs (instrumented);
# `quick` the way the Windows and macOS legs run it. Records go to target/demo/.
set -uo pipefail
mode=$1
mkdir -p target/demo
filter="binary(e2e) and ($(sed '/^$/d; s/.*/test(=&)/' .github/demo/sample.txt | paste -sd'|' - | sed 's/|/ or /g'))"
rm -rf target/test-profile
status=0
if [ "$mode" = coverage ]; then
  just _tier-test onepipeline-e2e-demo-sample "$filter" || status=$?
else
  just _tier-test-quick "$filter" || status=$?
fi
rm -rf target/demo/sample-alone
cp -r target/test-profile target/demo/sample-alone
exit "$status"
