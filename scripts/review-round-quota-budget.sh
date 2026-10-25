#!/usr/bin/env bash
# The command behind `budgets.yaml`'s `review-round-github-quota`.
#
# onebudgetspec runs it from this repository's root with `ONEBUDGETSPEC_RESULT` naming a fresh,
# empty file. It measures nothing itself: the charging and the report are the module beside it,
# over the record the e2e tier's busy-round journey leaves at
# `target/budget-records/review-round-quota.json`.
#
# Usage:
#   ONEBUDGETSPEC_RESULT=<file> bash scripts/review-round-quota-budget.sh
set -euo pipefail

if [ -z "${ONEBUDGETSPEC_RESULT:-}" ]; then
  echo "ONEBUDGETSPEC_RESULT is not set: this is a budget command, run by 'onebudgetspec check budgets.yaml --id review-round-github-quota', which names the file the result is written to" >&2
  exit 2
fi

exec node "$(dirname "$0")/review-round-quota-budget.mjs"
