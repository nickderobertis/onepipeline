#!/usr/bin/env bash
# DO NOT MERGE: the cache-size demonstration's harness. One measurement per call:
#   measure.sh headroom
#   measure.sh wheel-cold
#   measure.sh wheel-warm tgt|reg a|a-mtime|b|c|d
#   measure.sh fixture-generate
#   measure.sh fixture-check
#   measure.sh fixture-nx-output
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/../.."
target=x86_64-unknown-linux-gnu
wheel_dir="target/wheel-$target"
registry=target/wheel-cargo-registry
fixture=target/human-items-fixture
now() { date +%s.%N; }
secs() { awk -v a="$1" -v b="$2" 'BEGIN { printf "%.1f", b - a }'; }
say() { echo "MEASURE $*"; }
bytes() { du -sb "$@" | awk '{ s += $1 } END { print s + 0 }'; }
# The bytes actions/cache would store: its tar, zstd -T0 --long=30.
stored() { tar --posix -cf - -P "$@" | zstd -T0 --long=30 -q -c | wc -c; }

pull() {
  local t0; t0=$(now)
  docker pull -q quay.io/pypa/manylinux2014_x86_64 >/dev/null
  say "image_pull_s=$(secs "$t0" "$(now)")"
}

build() {
  local t0 status=0; t0=$(now)
  bash scripts/build-linux-wheel.sh --target "$target" --out dist-cache-size || status=$?
  say "build_s=$(secs "$t0" "$(now)") build_exit=$status"
  return "$status"
}

case "${1:-}" in
  headroom)
    result="$RUNNER_TEMP/headroom.json"
    families=()
    while read -r f; do families+=(--family "$f"); done < <(
      awk '/- id: onepipeline-cache-headroom/{p=1} p && /unit:/{exit} p && prev ~ /- --family/ {sub(/^ *- /, ""); print} {prev=$0}' budgets.yaml)
    echo "families: ${families[*]}"
    ONEBUDGETSPEC_RESULT="$result" bash scripts/actions-budget-figures.sh cache-headroom "${families[@]}"
    echo "--- figure"; cat "$result"
    echo "--- usage"; gh api "repos/$GITHUB_REPOSITORY/actions/cache/usage"
    echo "--- storage limit"; gh api "repos/$GITHUB_REPOSITORY/actions/cache/storage-limit" 2>&1 || true
    ;;
  wheel-cold)
    pull
    WHEEL_CARGO_REGISTRY="$registry" build
    say "target_dir_bytes=$(bytes "$wheel_dir")"
    say "registry_index_cache_bytes=$(bytes "$registry/index" "$registry/cache") registry_src_bytes=$(bytes "$registry/src")"
    say "stored_estimate_tgt=$(stored "$wheel_dir")"
    say "stored_estimate_reg=$(stored "$wheel_dir" "$registry/index" "$registry/cache")"
    ;;
  wheel-warm)
    candidate="${2:-}" case_="${3:-}"
    case "$case_" in
      a) ;;
      a-mtime) git ls-files -z | xargs -0 touch -h -d '2020-01-01T00:00:00Z' ;;
      b) echo '// cache-size: a change under src/ only' >> src/lib.rs ;;
      c) git show 67d2c932 -- Cargo.lock Cargo.toml | git apply -R ;;
      d)
        v=$(sed -n 's/^version = "\(.*\)"$/\1/p' Cargo.toml | head -1)
        n="${v%.*}.$(( ${v##*.} + 1 ))"
        sed -i "0,/^version = \"$v\"\$/s//version = \"$n\"/" Cargo.toml
        awk -v v="$v" -v n="$n" '/^name = "onepipeline"$/{f=1} f && $0 == "version = \"" v "\"" {$0 = "version = \"" n "\""; f=0} {print}' Cargo.lock > Cargo.lock.new
        mv Cargo.lock.new Cargo.lock
        ;;
      *) echo "case a|a-mtime|b|c|d" >&2; exit 2 ;;
    esac
    git diff --stat
    say "restored_target_dir_bytes=$(bytes "$wheel_dir")"
    pull
    case "$candidate" in
      tgt) build ;;
      reg) WHEEL_CARGO_REGISTRY="$registry" build ;;
      *) echo "candidate tgt|reg" >&2; exit 2 ;;
    esac
    ;;
  fixture-generate)
    t0=$(now)
    RUSTFLAGS="-D warnings" cargo build --release --locked --bin onepipeline --quiet
    say "release_build_s=$(secs "$t0" "$(now)")"
    t0=$(now)
    CACHE_SIZE_FIXTURE_OUT="$PWD/$fixture" HUMAN_ITEMS_BINARY="$PWD/target/release/onepipeline" RUSTFLAGS="-D warnings" \
      cargo test --locked --test human_items_workload -- --skip harness:: --test-threads 1 --nocapture 2>&1 | tee "$RUNNER_TEMP/fixture.log"
    say "test_wall_s=$(secs "$t0" "$(now)")"
    root=$(sed -n 's/.*cache-size: generate_s=[0-9.]* root=//p' "$RUNNER_TEMP/fixture.log")
    say "$(grep -o 'generate_s=[0-9.]*' "$RUNNER_TEMP/fixture.log")"
    say "fixture_bytes=$(bytes "$fixture") files=$(find "$fixture" -type f | wc -l)"
    for d in "$fixture"/*; do say "fixture_part $(basename "$d") bytes=$(bytes "$d")"; done
    say "files_naming_the_generation_root=$(grep -rlF -- "$root" "$fixture" | wc -l) root=$root"
    say "stored_estimate=$(stored "$fixture")"
    "$0" fixture-check
    ;;
  fixture-check)
    [ -d "$fixture" ] || { say "fixture_missing"; exit 1; }
    say "fixture_bytes=$(bytes "$fixture") files=$(find "$fixture" -type f | wc -l)"
    say "fixture_manifest_sha256=$(cd "$fixture" && find . -type f -printf '%P %s\n' | LC_ALL=C sort | sha256sum | cut -c1-16)"
    ;;
  fixture-nx-output)
    [ -d "$fixture" ] || { echo "no fixture: this target runs only after fixture-generate" >&2; exit 1; }
    ;;
  *) echo "see the header" >&2; exit 2 ;;
esac
