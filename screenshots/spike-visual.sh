#!/usr/bin/env bash
# SPIKE (spike-visual, throwaway — never lands). Re-captures every before and
# after screenshot of the driver-journal-scaling plan's three view changes:
#
#   bash screenshots/spike-visual.sh OUT_DIR [BASE_COMMIT]
#
# "after" is this checkout's `onepipeline` (the mock); "before" is BASE_COMMIT's
# (default a77106ce, the unchanged base), built from `git archive` under
# target/spike-visual/. Writes <scene>-{before,after}.png (and the .txt each was
# rendered from) into OUT_DIR. Needs `freeze` on PATH, as `just screenshots` does.
set -euo pipefail

out="${1:?usage: spike-visual.sh OUT_DIR [BASE_COMMIT]}"
base="${2:-a77106ce}"
repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root"

RUSTFLAGS="-D warnings" cargo build --release --locked --bin onepipeline >&2
RUSTFLAGS="-D warnings" cargo build --release --locked --package onevcs --bin onevcs >&2
RUSTFLAGS="-D warnings" cargo build --release --locked --package onepipeline-testfakes >&2

work="$repo_root/target/spike-visual"
src="$work/base-src"
commit="$(git rev-parse "$base^{commit}")"
if [ "$(cat "$work/base-src.commit" 2>/dev/null || true)" != "$commit" ]; then
  rm -rf "$src"
  mkdir -p "$src"
  git archive "$commit" | tar -x -C "$src"
  echo "$commit" > "$work/base-src.commit"
fi
(cd "$src" && CARGO_TARGET_DIR="$work/base-target" cargo build --release --locked --bin onepipeline >&2)

exec python3 screenshots/spike_visual.py "$work/base-target/release" target/release "$out"
