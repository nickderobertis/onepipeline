#!/usr/bin/env bash
# The command behind `budgets.yaml`'s `linear-requests-per-writeback-settlement`.
#
# onebudgetspec runs it from this repository's root with `ONEBUDGETSPEC_RESULT` naming a fresh,
# empty file. It runs one e2e journey through `just test-e2e`, and the journey writes the result
# itself, because only the journey can tell one write-back attempt's requests from the next. A
# journey that fails, or passes without writing the result, fails this command, which
# onebudgetspec reports as a measurement that errored rather than as a value.
#
# Usage:
#   ONEBUDGETSPEC_RESULT=<file> scripts/linear-writeback-budget.sh
set -euo pipefail

if [ -z "${ONEBUDGETSPEC_RESULT:-}" ]; then
  echo "ONEBUDGETSPEC_RESULT is not set: this is a budget command, run by 'onebudgetspec check budgets.yaml' ('just budgets'), which names the file the result is written to" >&2
  exit 2
fi

journey='linear_writeback::the_costliest_linear_settlement_after_the_first_is_reported_as_measured'
# llmlint: ignore-block[changed_behavior_has_e2e] the real invocation — this line through `just` and
# nextest to the journey's result file — is the budget check itself, `just budgets`, which errs
# rather than reports when the filter selects nothing or the result never arrives; `gate`
# checks its reported value against the budget. A test running it inside the suite would run the
# whole journey a second time per gate. `tests/budgets.rs` holds everything around it: the exact
# invocation, the journey it names, the refusals, and the checker parsing the journey's result.
just test-e2e "test(=${journey})" >&2
# llmlint: ignore-end[changed_behavior_has_e2e]

if [ ! -s "$ONEBUDGETSPEC_RESULT" ]; then
  echo "the journey ${journey} wrote no result to ${ONEBUDGETSPEC_RESULT}: it writes one only when that variable reaches it, so check that nothing between this script and the test binary drops it, then re-run 'just budgets'" >&2
  exit 1
fi
