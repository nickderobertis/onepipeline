#!/usr/bin/env bash
# The command behind `budgets.yaml`'s `check-visibility-github-quota`.
#
# onebudgetspec runs it from this repository's root with `ONEBUDGETSPEC_RESULT` naming a fresh,
# empty file. It measures nothing itself: the charging and the report are the module beside it,
# over the record the e2e tier's check-visibility workload journey leaves at
# `target/budget-records/check-visibility-quota.json`.
#
# Usage:
#   ONEBUDGETSPEC_RESULT=<file> bash scripts/check-visibility-quota-budget.sh
set -euo pipefail

if [ -z "${ONEBUDGETSPEC_RESULT:-}" ]; then
  echo "ONEBUDGETSPEC_RESULT is not set: this is a budget command, run by 'onebudgetspec check budgets.yaml --id check-visibility-github-quota', which names the file the result is written to" >&2
  exit 2
fi

exec node "$(dirname "$0")/check-visibility-quota-budget.mjs"
