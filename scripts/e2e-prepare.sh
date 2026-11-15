#!/usr/bin/env bash
# Build the helpers the tier already needs before nextest starts its journey deadlines.
set -euo pipefail
if [ "$#" -eq 0 ]; then
  echo 'e2e-prepare: missing test command. ACTION: pass the coverage-unit command and its arguments.' >&2
  exit 2
fi
started=$SECONDS
if just _e2e-doubles-build instrumented; then
  echo "e2e-prepare: helpers ready in $((SECONDS - started)) seconds" >&2
else
  status=$?
  echo "e2e-prepare: helper build failed after $((SECONDS - started)) seconds. ACTION: resolve the Cargo build failure above before running the tier." >&2
  exit "$status"
fi
exec "$@"
