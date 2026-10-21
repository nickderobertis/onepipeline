#!/usr/bin/env bash
# THROWAWAY — the one command that re-captures every screenshot of the
# tracker-dependency-reads spike-visual report:
#
#   bash spike/tracker-holds/capture.sh OUT_DIR
#
# Builds the binaries once (the before binary is this branch's base, which the
# spike leaves untouched — refused otherwise), then captures each before with that
# binary and each after with spike/tracker-holds/mock.py wrapping it. Needs the
# pinned `freeze` (`just screenshots-tools`). SPIKE_NO_BUILD=1 skips the build.
set -euo pipefail
repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$repo_root"
base=a4f178e07f5493795dc581f94dc8a90ae21e7861
git diff --quiet "$base" -- src crates Cargo.toml Cargo.lock \
  || { echo "spike: the engine differs from base $base; the befores would not be the base's" >&2; exit 2; }
if [ "${SPIKE_NO_BUILD:-0}" != 1 ]; then
  cargo build --release --locked --bin onepipeline >&2
  cargo build --release --locked --package onevcs --bin onevcs >&2
  cargo build --release --locked --package onepipeline-testfakes >&2
fi
exec python3 spike/tracker-holds/capture.py "${1:?usage: capture.sh OUT_DIR}"
