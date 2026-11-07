#!/usr/bin/env bash
# `report-job:budgets`: the repository-pinned checker over this project's own budget, reading what
# `report-job:test` saved with the released reader it saved beside it.
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/../.."
checker="$(just --evaluate onebudgetspec-root)/bin/onebudgetspec"
if [ ! -x "$checker" ]; then
  echo 'The repository-pinned onebudgetspec is missing; run just bootstrap.' >&2
  exit 1
fi
PATH="$PWD/.github/reporting/.telemetry/tools:$PATH" exec "$checker" check .github/reporting/budgets.yaml --save-results target/budget-results/report-job.json
