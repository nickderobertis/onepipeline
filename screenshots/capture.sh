#!/usr/bin/env bash
# The capture entry point: build what the capture drives, then drive it.
# `just screenshots` wraps exactly this, and the pre-push guard and the
# visual-docs workflow call it directly. What it captures is `AGENTS.md` beside
# it. `SCREENSHOTS_NO_BUILD=1` skips the build when the binaries are current.
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
