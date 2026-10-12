#!/usr/bin/env bash
# The spike's one command: the differential harness, then the dependency checks the
# report cites. Usage: spikes/saphyr-diff/run.sh [HOST_ROOT]   (HOST_ROOT is read only)
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
repo="$(cd "$here/../.." && pwd)"
manifest="$here/Cargo.toml"

cargo run -q --release --manifest-path "$manifest" -- "$@"

echo
echo "# DEPENDENCIES"
echo "## what serde-saphyr and saphyr bring in"
cargo tree -q --manifest-path "$manifest" -e normal,build -p serde-saphyr
cargo tree -q --manifest-path "$manifest" -e normal,build -p saphyr
echo "## today's YAML crates under serde-saphyr / saphyr (0 = absent)"
for crate in serde_yaml serde_yaml_ng serde_norway serde_yml unsafe-libyaml unsafe-libyaml-norway libyml; do
  n=$( { cargo tree -q --manifest-path "$manifest" -e normal,build -p serde-saphyr
         cargo tree -q --manifest-path "$manifest" -e normal,build -p saphyr; } | grep -c " $crate v" || true)
  echo "- $crate: $n"
done
echo "## who pulls today's YAML crates into the build (unchanged by this repository's move)"
for crate in serde_norway serde_yaml_ng unsafe-libyaml unsafe-libyaml-norway; do
  cargo tree -q --manifest-path "$manifest" -e normal,build -i "$crate" --depth 1 || true
done
echo "## cargo deny with this repository's deny.toml over the harness's lock"
echo "## (the lock is the repository's own plus serde-saphyr, saphyr and serde_path_to_error)"
(cd "$repo" && cargo deny --manifest-path "$manifest" check --config deny.toml licenses bans sources 2>&1 | tail -n 1)
(cd "$repo" && cargo deny --manifest-path "$manifest" check --config deny.toml advisories 2>&1 \
  | grep -E '^(error|warning)\[|^advisories' || true)
