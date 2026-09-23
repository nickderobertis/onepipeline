#!/usr/bin/env bash
# The one entry point three callers share — `just screenshots`, the pre-push
# guard, and the visual-docs workflow — so none of them can build a different
# thing from the one it captures. `SCREENSHOTS_NO_BUILD=1` skips the build.

# llmlint: ignore-file[changed_behavior_has_e2e] the capture is informational machinery
# whose every step is a third-party tool this repository deliberately does not install
# for `just check` — `freeze` renders the scenes and `screencomp` classifies them — so
# an offline journey of it could only assert against stubs of those two, which tests the
# stubs. What this machinery produces is checked instead, and more strictly than a
# journey would: the committed digest baseline is re-derived by
# `.github/workflows/visual-docs.yml` on every pull request, and a capture that stopped
# working, changed a byte, or lost a scene is a red check there.
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root"

# The real binary a user runs, the real `onevcs` CLI the world registers its
# repositories through, and the two scripted doubles this repository's own
# offline tier substitutes at their subprocess boundaries.
if [ -z "${SCREENSHOTS_NO_BUILD:-}" ]; then
  RUSTFLAGS="-D warnings" cargo build --release --locked \
    --bin onepipeline --package onevcs --bin onevcs >&2
  RUSTFLAGS="-D warnings" cargo build --release --locked --package onepipeline-testfakes >&2
fi

exec python3 screenshots/capture.py "$@"
