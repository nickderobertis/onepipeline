#!/usr/bin/env bash
# The settlement-readability-latency budget: the journey writes its own journal-stamp delta.
set -euo pipefail
if [ -z "${ONEBUDGETSPEC_RESULT:-}" ]; then
  echo "ONEBUDGETSPEC_RESULT is not set: run this measurement through just budgets" >&2
  exit 2
fi
journey='loopcost::the_settlement_readability_latency_is_reported_from_its_journal_stamps'
just test-e2e "test(=${journey})" >&2
if [ ! -s "$ONEBUDGETSPEC_RESULT" ]; then
  echo "the journey ${journey} wrote no measured budget result; inspect its output above and verify ONEBUDGETSPEC_RESULT reaches the test process, then re-run just budgets" >&2
  exit 1
fi
