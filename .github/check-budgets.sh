#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.."
checker_root="$(just --evaluate onebudgetspec-root)"
checker="$checker_root/bin/onebudgetspec"
[ -x "$checker" ] || checker="${checker}.exe"
if [ ! -x "$checker" ]; then
  echo 'The repository-pinned onebudgetspec is missing; run just bootstrap.' >&2
  exit 1
fi
exec "$checker" check .github/budgets.yaml
