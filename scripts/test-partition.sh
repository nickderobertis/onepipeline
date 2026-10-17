#!/usr/bin/env bash
# Print the nextest partition `ONEPIPELINE_TEST_PARTITION` names, or nothing when
# it names none, for `_tier-test-quick` to run its tier's share of.
#
# `count:K/N` is the one shape accepted: nextest deals each binary's tests out in
# its listing order, one to each of the N shards in turn, so the N shards of one
# build run every test of a tier exactly once on whatever platform lists them. The
# Windows leg of `ci.yml` is split across N jobs on that property, and
# `.github/cross-shards.mjs` proves it of this host's listing. Anything else is
# refused rather than passed through: `hash:` or `slice:` would also run each test
# once, but a K past N, a zero, or a typo would run a share that no other job
# makes up — a leg reporting green over tests nobody ran.
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
