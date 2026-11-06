#!/usr/bin/env bash
# Build the job's diagnostic archive only when Nx actually executes a test tier.
# RUNNER_TEMP gives every job a fresh destination; concurrent tier misses share it.
# Exit 0 for a completed/reused archive or disabled archive output; 2 for usage;
# 130/143 for INT/TERM. Other filesystem and archive-builder failures propagate.
set -euo pipefail
if [ "$#" -ne 1 ]; then
  echo "name one archive mode; use coverage or quick" >&2
  exit 2
fi
mode=$1
case "$mode" in coverage | quick) ;; *) echo "unknown archive mode: $mode; use coverage or quick" >&2; exit 2 ;; esac
archive=${ONEPIPELINE_TEST_ARCHIVE:-}
[ -n "$archive" ] || exit 0
[ ! -f "$archive" ] || exit 0
mkdir -p "$(dirname "$archive")"
lock="${archive}.lock"
while :; do
  if node - "$lock" <<'NODE'
const { mkdirSync, statSync } = require("node:fs");
const lock = process.argv[2];
try {
  mkdirSync(lock);
} catch (error) {
  if (error.code === "EEXIST") {
    try {
      if (statSync(lock).isDirectory()) process.exit(75);
    } catch (inspection) {
      // The holder can release the lock between mkdir and inspection.
      if (inspection.code === "ENOENT") process.exit(75);
      error = inspection;
    }
  }
  console.error(`cannot create archive lock ${lock}: ${error.message}; remove a conflicting file and ensure its parent is writable`);
  process.exit(1);
}
NODE
  then
    break
  else
    status=$?
    [ "$status" -eq 75 ] || exit "$status"
  fi
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
