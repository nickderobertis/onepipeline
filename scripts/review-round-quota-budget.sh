#!/usr/bin/env bash
# The command behind `budgets.yaml`'s `review-round-github-quota`.
#
# onebudgetspec runs it from this repository's root with `ONEBUDGETSPEC_RESULT` naming a fresh,
# empty file. It measures nothing itself: the e2e tier's busy-round journey
# (`review_round::the_busy_review_round_ends_answered_and_true_and_records_what_it_spent`)
# records the remote double's call ledger and the engine's store operations at
# `target/budget-records/review-round-quota.json`, and this charges that telemetry — the
# ledger's own charges, each `gh` call the double did not answer at GitHub's published cost,
# each store task written 2 GraphQL points and each store read 1 — and reports the sum through
# the onebudgetspec SDK, with the measure's breakdown in its detail. The SDK is the TypeScript
# one this repository already installs, so the charging lives in the module beside this file.
#
# Usage:
#   ONEBUDGETSPEC_RESULT=<file> bash scripts/review-round-quota-budget.sh
set -euo pipefail

if [ -z "${ONEBUDGETSPEC_RESULT:-}" ]; then
  echo "ONEBUDGETSPEC_RESULT is not set: this is a budget command, run by 'onebudgetspec check budgets.yaml --id review-round-github-quota', which names the file the result is written to" >&2
  exit 2
fi

exec node "$(dirname "$0")/review-round-quota-budget.mjs"
