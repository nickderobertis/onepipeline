#!/usr/bin/env bash
# Measurement only (spike-git-reads, never lands): run the vendored onevcs copy's own
# unit tests. vendor/onevcs sits inside this workspace, which cargo refuses to test
# in place, and its crates.io package leaves out two golden files its status tests
# include; so the crate is copied to WORKDIR with an empty [workspace] table and
# those files are taken from the onevcs checkout ONEVCS_SOURCE at the tag the copy
# was vendored from. Arguments after WORKDIR go to `cargo test --lib`.
set -euo pipefail
usage() { echo "usage: ONEVCS_SOURCE=<onevcs checkout> $0 WORKDIR [cargo test args...]" >&2; exit 2; }
[ $# -ge 1 ] || usage
: "${ONEVCS_SOURCE:?set ONEVCS_SOURCE to an onevcs checkout holding tag v0.67.0}"
workdir=$1; shift
here=$(cd "$(dirname "$0")/../.." && pwd)
copy="$workdir/onevcs"
mkdir -p "$copy"
rsync -a --delete --exclude target "$here/vendor/onevcs/" "$copy/"
printf '\n[workspace]\n' >> "$copy/Cargo.toml"
mkdir -p "$copy/tests/golden"
for file in status-report-v13.json status-report-v13-minimal.json; do
  git -C "$ONEVCS_SOURCE" show "v0.67.0:crates/onevcs/tests/golden/$file" > "$copy/tests/golden/$file"
done
cd "$copy"
CARGO_TARGET_DIR="$workdir/target" exec cargo test --lib "$@"
