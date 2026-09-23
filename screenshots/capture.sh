#!/usr/bin/env bash
# The one entry point three callers share — `just screenshots`, the pre-push
# guard, and the visual-docs workflow — so none of them can build a different
# thing from the one it captures. `SCREENSHOTS_NO_BUILD=1` skips the build.

# llmlint: ignore-file[changed_behavior_has_e2e] the capture's every step is a
# third-party tool `just check` does not install (`freeze`, `screencomp`), so a journey
# could only assert against stubs of those two. What it produces is checked instead: the
# visual-docs workflow re-derives the committed baseline on every pull request.
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
