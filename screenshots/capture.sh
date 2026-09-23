#!/usr/bin/env bash
# The one entry point three callers share — `just screenshots`, the pre-push
# guard, and the visual-docs workflow — so none of them can build a different
# thing from the one it captures. `SCREENSHOTS_NO_BUILD=1` skips the build; it
# is held to a value it can have, below, rather than read as present-means-on.

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
#
# One `cargo build` per package: `--package` scopes EVERY `--bin` in the same
# invocation, so asking one command for this crate's binary and the sibling's
# looks for both in the sibling and fails with "no bin target named
# `onepipeline` in `onevcs` package".
#
# `SCREENSHOTS_NO_BUILD` is external input and it decides whether the scenes are
# rendered from the binaries in this tree. Present-means-on would read
# `SCREENSHOTS_NO_BUILD=0` as "skip the build" — the opposite of what that
# typed, and a capture of a stale binary is a baseline blessed for code nobody
# here has. So the value is held to one it can have and anything else is
# refused rather than guessed.
case "${SCREENSHOTS_NO_BUILD:-0}" in
  0 | false | no) build=1 ;;
  1 | true | yes) build=0 ;;
  *)
    echo "capture: SCREENSHOTS_NO_BUILD is '${SCREENSHOTS_NO_BUILD}', which is neither" >&2
    echo "         0/false/no nor 1/true/yes. Refusing rather than guessing whether to" >&2
    echo "         rebuild the binaries the scenes are rendered from." >&2
    exit 2
    ;;
esac
if [ "$build" = 1 ]; then
  RUSTFLAGS="-D warnings" cargo build --release --locked --bin onepipeline >&2
  RUSTFLAGS="-D warnings" cargo build --release --locked --package onevcs --bin onevcs >&2
  RUSTFLAGS="-D warnings" cargo build --release --locked --package onepipeline-testfakes >&2
fi

exec python3 screenshots/capture.py "$@"
