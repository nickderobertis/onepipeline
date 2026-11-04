#!/usr/bin/env bash
# Build the job's diagnostic archive only when Nx actually executes a test tier.
# RUNNER_TEMP gives every job a fresh destination; concurrent tier misses share it.
set -euo pipefail
mode=${1:?name coverage or quick}
case "$mode" in coverage | quick) ;; *) echo "unknown archive mode: $mode; use coverage or quick" >&2; exit 2 ;; esac
archive=${ONEPIPELINE_TEST_ARCHIVE:-}
[ -n "$archive" ] || exit 0
[ ! -f "$archive" ] || exit 0
mkdir -p "$(dirname "$archive")"
lock="${archive}.lock"
while ! mkdir "$lock" 2>/dev/null; do
  [ ! -f "$archive" ] || exit 0
  sleep 1
done
pending="$lock/$(basename "$archive")"
trap 'rm -f "$pending"; rmdir "$lock"' EXIT
trap 'exit 143' TERM
trap 'exit 130' INT
[ ! -f "$archive" ] || exit 0
if [ "$mode" = coverage ]; then
  ONEPIPELINE_TEST_ARCHIVE="$pending" just _crate-test-archive
else
  ONEPIPELINE_TEST_ARCHIVE="$pending" just _crate-test-quick-archive
fi
mv "$pending" "$archive"
