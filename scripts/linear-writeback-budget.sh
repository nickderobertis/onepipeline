#!/usr/bin/env bash
# The command behind `budgets.yaml`'s `linear-requests-per-writeback-settlement`.
#
# onebudgetspec runs it from this repository's root with `ONEBUDGETSPEC_RESULT` naming a fresh,
# empty file. It runs one journey of the e2e binary,
# `linear_writeback::consecutive_linear_settlements_each_land_their_own_status_and_metadata_in_their_own_attempt`:
# the compiled binary launches a plan out of a Linear project served by the loopback endpoint in
# `tests/e2e/linear_loopback.rs`, configured with the per-kind `status_mapping`, and the real
# write-back worker projects six consecutive settlements, each in an attempt of its own. The
# journey writes the result itself — the most requests the endpoint served for one attempt after
# the first — because only it can tell one attempt's requests from the next.
#
# Offline and credential-free: the endpoint is on 127.0.0.1 and the key it is sent is a
# placeholder. A journey that fails, or one that passes without writing the result, fails this
# command, which onebudgetspec reports as a measurement that errored rather than as a value.
#
# Usage:
#   ONEBUDGETSPEC_RESULT=<file> scripts/linear-writeback-budget.sh
set -euo pipefail

if [ -z "${ONEBUDGETSPEC_RESULT:-}" ]; then
  echo "ONEBUDGETSPEC_RESULT is not set: this is a budget command, run by 'onebudgetspec check budgets.yaml', which names the file the result is written to" >&2
  exit 2
fi

journey='linear_writeback::consecutive_linear_settlements_each_land_their_own_status_and_metadata_in_their_own_attempt'
cargo nextest run --locked --no-tests=fail -E "binary(e2e) and test(=${journey})" >&2

if [ ! -s "$ONEBUDGETSPEC_RESULT" ]; then
  echo "the journey ${journey} passed and wrote no result to ${ONEBUDGETSPEC_RESULT}" >&2
  exit 1
fi
