#!/usr/bin/env bash
# The capture entry point: one command that builds what the capture drives and
# then drives it. `just screenshots` wraps exactly this, and the pre-push guard
# and the visual-docs workflow call it directly.
#
# What it captures, and why each scene, is `screenshots/AGENTS.md`.
# `SCREENSHOTS_NO_BUILD=1` skips the build when the binaries are already current.
#
# llmlint: ignore-file[new_code_lands_in_a_project] the visual-docs capture is
# informational machinery outside every Nx target the gate reaches, and outside
# the crate on purpose (the 95% coverage floor is measured over the crate).
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root"

# The real binary a user runs, the real `onevcs` CLI the world registers its
# repositories through, and the two scripted doubles this repository's own
# offline tier substitutes at their subprocess boundaries.
if [ -z "${SCREENSHOTS_NO_BUILD:-}" ]; then
  cargo build --release --locked \
    --bin onepipeline --package onevcs --bin onevcs >&2
  cargo build --release --locked --package onepipeline-testfakes >&2
fi

exec python3 scripts/screenshots.py "$@"
