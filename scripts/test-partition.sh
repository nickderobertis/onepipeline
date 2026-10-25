#!/usr/bin/env bash
# Print the nextest partition `ONEPIPELINE_TEST_PARTITION` names, or nothing when
# it names none, for `_tier-test-quick` to run its tier's share of.
#
# `count:K/N` is the one shape accepted, because it is the one `ci.yml`'s split of
# the Windows leg and `.github/cross-shards.mjs` account for. Anything else is
# refused rather than passed through: a K past N, a zero, or a typo would run a
# share that no other job makes up — a leg reporting green over tests nobody ran.
set -euo pipefail

partition="${ONEPIPELINE_TEST_PARTITION:-}"
if [ -z "$partition" ]; then
  exit 0
fi
if ! [[ "$partition" =~ ^count:([1-9][0-9]{0,2})/([1-9][0-9]{0,2})$ ]] \
  || [ "${BASH_REMATCH[1]}" -gt "${BASH_REMATCH[2]}" ]; then
  echo "test-partition: ONEPIPELINE_TEST_PARTITION holds '$partition', which is not count:K/N with 1 <= K <= N (at most three digits each)" >&2
  echo "test-partition: unset it to run every test, or set it to one shard of the split, such as count:1/2" >&2
  exit 2
fi
printf '%s\n' "$partition"
