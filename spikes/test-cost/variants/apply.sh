#!/usr/bin/env bash
# Spike (spike-onepipeline-test-cost): turn this checkout into one measured variant.
# `baseline` leaves the tree as committed; every other variant applies its patch to
# the index (so line endings follow the checkout's own), commits it, and checks the
# tree out again, so the job runs a clean tree whose one extra commit is the variant.
set -euo pipefail
variant="${1:?name the variant}"
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
case "$variant" in
  baseline) echo "variant baseline: the committed tree, unchanged"; exit 0 ;;
  idle | runtime | pyramid | depsopt) git apply --cached "$here/$variant.patch" ;;
  *) echo "apply.sh: unknown variant '$variant'" >&2; exit 2 ;;
esac
git -c user.name="spike variant" -c user.email="spike@example.invalid" commit -q -m "spike variant: $variant"
git checkout -f HEAD -- .
if [ "$variant" = runtime ] && [ -n "${GITHUB_ENV:-}" ]; then
  # The vendored onevcs is not this crate's code: its lines stay out of the floor's count.
  echo "COVERAGE_UNIT_IGNORE_REGEX=/crates/testfakes/|/spikes/" >> "$GITHUB_ENV"
fi
git log --oneline -1
git show --stat --format= HEAD
