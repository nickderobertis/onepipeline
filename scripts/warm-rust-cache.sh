#!/usr/bin/env bash
# Build, testing nothing, what one of ci.yml's test jobs compiles, so
# `rust-cache-warm.yml` can save it from `main` under the key that job restores:
#
#   warm-rust-cache.sh linux    what `test-linux` builds: the instrumented test
#                               build every coverage tier runs, the debug build
#                               the stop-verdict tier and the npm suite run, the
#                               release build the stop-verdict tier times and the
#                               install proof installs, and the docs; and
#                               clippy's build, for `lint`, which restores this
#                               key too.
#   warm-rust-cache.sh cross    what `test-windows` and `test-macos` build: the
#                               uninstrumented test build, clippy's, and the
#                               release build the install proof installs.
#
# Each build is the justfile recipe the job's own target runs, so it is compiled
# under the flags the job compiles it under: a unit built under other flags is a
# different unit, and the job would build it again.
#
# Exit: the recipe's, or 2 for a refused call.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"
archive="$ROOT/target/warm-archive.tar.zst"
trap 'rm -f -- "$archive"' EXIT

[ "$#" -eq 1 ] || {
  echo "warm-rust-cache: takes exactly one argument, the job family, not $#" >&2
  echo "ACTION: run 'scripts/warm-rust-cache.sh linux|cross'" >&2
  exit 2
}

# llmlint: ignore-block[changed_behavior_has_e2e] each mode is the justfile's own build recipes, compiling the workspace;
# rust-cache-warm.yml runs them for real on every push to main, and a pull request
# restoring what they saved is what shows the keys agree (.github/ci-shape holds
# the keys equal). Its refusal is in tests/ci_jobs/proofs.test.mjs.
case "${1:-}" in
  linux)
    ONEPIPELINE_TEST_ARCHIVE="$archive" just _crate-test-archive
    just _crate-build _stop-verdict-build _crate-doc _crate-lint
    ;;
  cross)
    ONEPIPELINE_TEST_ARCHIVE="$archive" just _crate-test-quick-archive
    just _crate-lint _stop-verdict-build
    ;;
  *)
    echo "warm-rust-cache: name the job family: linux or cross" >&2
    echo "ACTION: run 'scripts/warm-rust-cache.sh linux|cross'" >&2
    exit 2
    ;;
esac
# llmlint: ignore-end[changed_behavior_has_e2e]
