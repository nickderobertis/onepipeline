#!/usr/bin/env bash
# DO NOT MERGE: the cache-size demonstration's harness. One measurement per call:
#   measure.sh headroom
#   measure.sh headroom-derived
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
  headroom-derived)
    # The registered reader's arithmetic over listings narrowed by each key's first
    # character, reconciled against the usage endpoint's count. Entries this harness
    # saved (cache-size-*) are reported apart and not counted.
    api="repos/$GITHUB_REPOSITORY/actions"
    out="$RUNNER_TEMP/entries.ndjson"; : > "$out"
    echo "--- usage"; gh api "$api/cache/usage" | tee "$RUNNER_TEMP/usage.json"; echo
    echo "--- storage limit"; gh api "$api/cache/storage-limit" 2>&1 || true
    seen=0 requests=2
    pending=({0..9} {a..z} {A..Z} . _ -)
    while [ "${#pending[@]}" -gt 0 ]; do
      c="${pending[0]}"; pending=("${pending[@]:1}")
      doc=$(gh api "$api/caches?per_page=100&key=$c"); requests=$((requests + 1))
      total=$(jq .total_count <<<"$doc")
      [ "$total" -gt 0 ] || continue
      if [ "$total" -gt 100 ] && [ "$c" != n ]; then
        exact=$(jq --arg c "$c" '[.actions_caches[] | select(.key == $c)] | length' <<<"$doc")
        [ "$exact" -eq 0 ] || { echo "key $c is held by $total entries" >&2; exit 1; }
        for d in {0..9} {a..z} {A..Z} . _ -; do pending+=("$c$d"); done
        continue
      fi
      if [ "$c" = n ]; then
        nx=$(gh api "$api/caches?per_page=1&key=nx-cache-" --jq .total_count); requests=$((requests + 1))
        say "prefix n: $total entries, nx-cache-: $nx"
        [ "$nx" -eq "$total" ] || { echo "non-result keys under n: $((total - nx))" >&2; exit 1; }
        seen=$((seen + total)); continue
      fi
      [ "$total" -le 100 ] || { echo "prefix $c holds $total entries" >&2; exit 1; }
      jq -c '.actions_caches[] | {key, ref, size_in_bytes, created_at, last_accessed_at}' <<<"$doc" >> "$out"
      seen=$((seen + total))
    done
    count=$(jq .active_caches_count "$RUNNER_TEMP/usage.json")
    used=$(jq .active_caches_size_in_bytes "$RUNNER_TEMP/usage.json")
    say "entries seen by prefix: $seen of $count; requests: $requests"
    families=$(awk '/- id: onepipeline-cache-headroom/{p=1} p && /unit:/{exit} p && prev ~ /- --family/ {sub(/^ *- /, ""); print} {prev=$0}' budgets.yaml | jq -R . | jq -s .)
    jq -s -r --argjson fam "$families" --argjson used "$used" '
      def fam($k): ($fam | map(select(. as $p | $k | startswith($p))) | .[0]);
      (map(select(.key | startswith("cache-size-")))) as $mine
      | (map(select(.key | startswith("cache-size-") | not))) as $theirs
      | ($theirs | map(. + {family: fam(.key)}) | group_by(.family)) as $groups
      | ([$groups[] | select(.[0].family != null) | sort_by(.created_at) | reverse | .[1:][] ] ) as $older
      | ($mine | map(.size_in_bytes) | add // 0) as $minebytes
      | ($older | map(.size_in_bytes) | add // 0) as $olderbytes
      | ($theirs | map(.size_in_bytes) | add // 0) as $nonresult
      | (10 * 1073741824) as $allowance
      | ($used - $minebytes - $olderbytes) as $counted
      | "allowance: \($allowance) bytes (\($allowance / 1e9) GB, GitHub default)",
        ($theirs | sort_by(-.size_in_bytes)[] | "entry \(.size_in_bytes)\t\(.ref)\t\(.created_at)\t\(.last_accessed_at)\t\(.key)"),
        ($mine[] | "harness entry (not counted) \(.size_in_bytes)\t\(.ref)\t\(.key)"),
        "older copies in named families: \($older | length), \($olderbytes) bytes, not counted",
        "non-result bytes: \($nonresult)",
        "result bytes (usage less non-result less harness): \($used - $nonresult - $minebytes)",
        "counted: \($counted) bytes",
        "MEASURE headroom_derived_gb=\((($allowance - $counted) / 1e9 * 1000 | round) / 1000)"
    ' "$out"
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
