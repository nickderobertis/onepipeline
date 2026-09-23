#!/usr/bin/env bash
# Refresh THIS host's committed digest baseline from the capture in
# `shots/current` — the one place that decides which lane is rewritten and where,
# so `just screenshots-bless` and the pre-push guard's drift path cannot disagree
# about it. `$SHOTS_CURRENT` overrides the capture root.

# llmlint: ignore-file[changed_behavior_has_e2e] the capture is informational machinery
# whose every step is a third-party tool this repository deliberately does not install
# for `just check` — `freeze` renders the scenes and `screencomp` classifies them — so
# an offline journey of it could only assert against stubs of those two, which tests the
# stubs. What this machinery produces is checked instead, and more strictly than a
# journey would: the committed digest baseline is re-derived by
# `.github/workflows/visual-docs.yml` on every pull request, and a capture that stopped
# working, changed a byte, or lost a scene is a red check there.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

if ! command -v screencomp >/dev/null 2>&1; then
  echo "bless: screencomp is not installed, so the baseline cannot be refreshed." >&2
  echo "       Install it, then re-run 'just screenshots-bless':" >&2
  echo "       https://github.com/nickderobertis/screencomp#install" >&2
  exit 1
fi

current="${SHOTS_CURRENT:-shots/current}"
if [ ! -d "$current" ]; then
  echo "bless: there is no capture to bless at $current." >&2
  echo "       Capture one first: just screenshots" >&2
  exit 1
fi

lane="$(bash "$here/host-arch.sh")"
screencomp manifest --input "$current" --arch "$lane" --output "shots/baseline/${lane}.json"
