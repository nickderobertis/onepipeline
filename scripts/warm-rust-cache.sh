#!/usr/bin/env bash
# Build, testing nothing, what one of ci.yml's test jobs compiles, so
# `rust-cache-warm.yml` can save it from `main` under the key that job restores:
#
#   warm-rust-cache.sh linux    what `test-linux` builds: the instrumented test
#                               build every coverage tier runs, the debug build
#                               the stop-verdict tier and the npm suite run, the
#                               release build the stop-verdict tier times and the
#                               install proof installs, and the docs.
#   warm-rust-cache.sh cross    what `test-windows` and `test-macos` build: the
#                               uninstrumented test build, clippy's, and the
#                               release build the install proof installs.
#
# Each command is the one the job's own target runs, under the same flags,
# because a unit built under other flags is a different unit and the job would
# compile it again. The gate's builds deny warnings through RUSTFLAGS; the
# uninstrumented suite and clippy set none (`_tier-test-quick`, `_crate-lint`),
# so neither does their warm build; the instrumented one is cargo-llvm-cov's own.
#
# Exit: cargo's, or 2 for a refused call.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"
case "${1:-}" in
  linux)
    export RUSTFLAGS="-D warnings"
    # `_crate-test-archive`'s build, archived nowhere: the tiers' `cargo llvm-cov
    # nextest` runs find it fresh.
    cargo llvm-cov --no-report nextest-archive --locked --archive-file "$ROOT/target/warm-instrumented.tar.zst"
    rm -f -- "$ROOT/target/warm-instrumented.tar.zst"
    cargo build --locked --all-targets --quiet
    cargo build --release --locked --quiet --bin onepipeline
    RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --locked --quiet
    ;;
  cross)
    unset RUSTFLAGS
    cargo nextest archive --locked --archive-file "$ROOT/target/warm-tests.tar.zst"
    rm -f -- "$ROOT/target/warm-tests.tar.zst"
    cargo clippy --all-targets --locked --quiet -- -D warnings
    # scripts/install-proof.sh's build, which denies warnings.
    RUSTFLAGS="-D warnings" cargo build --release --locked --quiet
    ;;
  *)
    echo "warm-rust-cache: name the job family: linux or cross" >&2
    echo "ACTION: run 'scripts/warm-rust-cache.sh linux|cross'" >&2
    exit 2
    ;;
esac
