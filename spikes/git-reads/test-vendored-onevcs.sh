#!/usr/bin/env bash
# Measurement only, never lands: test the copied vendored crate outside this
# workspace. Its package omits two include_str! goldens, read from its exact tag.
# Exit 2: usage; 1: setup/evidence refusal; otherwise cargo's exit status.
# Usage: ONEVCS_SOURCE=<checkout> $0 WORKDIR [cargo test --lib arguments...]
set -euo pipefail
fail() { echo "vendored tests: $1" >&2; echo "next: $2" >&2; exit 1; }
if [ $# -lt 1 ]; then
  echo "vendored tests: missing WORKDIR" >&2
  echo "usage: ONEVCS_SOURCE=<checkout> $0 WORKDIR [cargo test --lib arguments...]" >&2
  exit 2
fi
trap 'fail "setup failed at line $LINENO" "check the preceding error, the writable WORKDIR and ONEVCS_SOURCE tag, then rerun"' ERR
: "${ONEVCS_SOURCE:?set ONEVCS_SOURCE to the onevcs checkout holding the vendored release tag}"
here=$(cd "$(dirname "$0")/../.." && pwd -P)
workdir=$(realpath -m "$1"); shift
case "$workdir/" in "$here/"*) fail "WORKDIR is inside the source tree" "choose a scratch directory outside this checkout" ;; esac
copy="$workdir/onevcs"
marker="$workdir/.git-reads-vendored-test-source"
if [ -L "$copy" ] || { [ -e "$copy" ] && { [ ! -f "$marker" ] || [ "$(cat "$marker")" != "$here" ]; }; }; then
  fail "refusing an unowned existing destination: $copy" "choose a new WORKDIR; only this wrapper's marked copies may be replaced"
fi
# llmlint: ignore-block[changed_behavior_has_e2e] This never-landing setup wrapper is exercised by this spike's recorded real cargo test invocations over the copied crate and exact-tag goldens; duplicating the full Rust build in a JavaScript fixture would add no separate interface proof. Unsafe-destination refusals are exercised before copying.
version=$(python3 - "$here/vendor/onevcs/Cargo.toml" <<'PY'
import sys,tomllib
with open(sys.argv[1],'rb') as f: print(tomllib.load(f)['package']['version'])
PY
)
mkdir -p "$copy"
printf '%s\n' "$here" > "$marker"
rsync -a --delete --exclude target "$here/vendor/onevcs/" "$copy/"
printf '\n[workspace]\n' >> "$copy/Cargo.toml"
mkdir -p "$copy/tests/golden"
for file in status-report-v13.json status-report-v13-minimal.json; do
  git -C "$ONEVCS_SOURCE" show "v$version:crates/onevcs/tests/golden/$file" > "$copy/tests/golden/$file"
done
cd "$copy"
CARGO_TARGET_DIR="$workdir/target" exec cargo test --lib "$@"

# llmlint: ignore-end[changed_behavior_has_e2e]
