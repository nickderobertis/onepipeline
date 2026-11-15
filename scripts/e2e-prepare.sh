#!/usr/bin/env bash
# Build the helpers the tier already needs before nextest starts its journey deadlines.
set -euo pipefail
if [ "$#" -eq 0 ]; then
  echo 'e2e-prepare: missing test command. ACTION: pass the coverage-unit command and its arguments.' >&2
  exit 2
fi
if [ "$(uname -s)" != Linux ]; then
  exec "$@"
fi
started=$SECONDS
if metadata="$(cargo metadata --offline --locked --no-deps --format-version 1)"; then
  :
else
  status=$?
  echo 'e2e-prepare: cannot inspect workspace dependencies. ACTION: resolve the Cargo metadata failure above before running tests.' >&2
  exit "$status"
fi
if helpers="$(printf '%s' "$metadata" | node -e '
  const data = JSON.parse(require("node:fs").readFileSync(0, "utf8"));
  if (!Array.isArray(data.packages) || data.packages.some(p => !Array.isArray(p.dependencies) || p.dependencies.some(d => typeof d.name !== "string"))) {
    throw new Error("Cargo metadata must list packages and their named dependencies");
  }
  const engines = new Set(["oneagentgraph", "oneharness", "onevcs"]);
  process.stdout.write(data.packages.some(p => p.dependencies.some(d => engines.has(d.name))) ? "needed" : "absent");
')"; then
  :
else
  status=$?
  echo 'e2e-prepare: cannot read workspace dependencies. ACTION: restore valid Cargo metadata before running tests.' >&2
  exit "$status"
fi
if [ "$helpers" = absent ]; then
  exec "$@"
fi
if just _e2e-doubles-build instrumented; then
  echo "e2e-prepare: helpers ready in $((SECONDS - started)) seconds" >&2
else
  status=$?
  echo "e2e-prepare: helper build failed after $((SECONDS - started)) seconds. ACTION: resolve the Cargo build failure above before running the tier." >&2
  exit "$status"
fi
exec "$@"
