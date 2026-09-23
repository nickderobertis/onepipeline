#!/usr/bin/env bash
# Refresh THIS host's committed digest baseline from the capture in
# `shots/current` — the one place that decides which lane is rewritten and where,
# so `just screenshots-bless` and the pre-push guard's drift path cannot disagree
# about it. `$SHOTS_CURRENT` overrides the capture root.

# llmlint: ignore-file[changed_behavior_has_e2e] the capture's every step is a
# third-party tool `just check` does not install (`freeze`, `screencomp`), so a journey
# could only assert against stubs of those two. What it produces is checked instead: the
# visual-docs workflow re-derives the committed baseline on every pull request.
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
