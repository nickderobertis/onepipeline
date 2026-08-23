#!/usr/bin/env bash
# What this build actually links, and whether it is what its own manifest
# already permits.
#
# The failure this exists to prevent has happened three times: a release ships a
# `Cargo.lock` that resolves a sibling engine *older* than `Cargo.toml`'s own
# requirement permits, and a downstream reader who checks the requirement — the
# only signal that is easy to reach — concludes the fix is in the binary. The
# tag exists, the changelog says it shipped, the requirement allows it, and the
# running binary has never contained it. Only the lock disagrees, and nothing
# was reading the lock.
#
# Two modes over one reading of the tree:
#
#   --format check   (default) exits 1 naming every engine the lock holds behind
#                    what its requirement permits, with the version it resolves,
#                    the version the requirement permits, and the `cargo update`
#                    spec that fixes it. `just lock-current` runs this, and
#                    `.github/workflows/lock-currency.yml` runs that recipe
#                    weekly.
#   --format notes   the markdown a release's notes carry, so "what does this
#                    release link?" is answered where a reader already is
#                    rather than only inside a published wheel's SBOM.
#                    `just linked-engines` runs this, and release.yml appends
#                    its output to the GitHub Release body.
#
# It reads the crates.io **sparse index** to learn what a requirement permits
# today, which is why the recipes sit outside `just check` — the same reason
# `deps-check` does, and recorded in AGENTS.md beside it. `--index` also accepts
# a directory in that same layout, which is what `tests/linked_engines.rs`
# drives it against so the offline gate can prove both modes end to end.
#
# Usage:
#   linked-engines.sh [--format check|notes] [--manifest PATH] [--lock PATH]
#                     [--index URL_OR_DIR]
set -euo pipefail

# The siblings whose currency is a claim this repository makes. `oneagentgraph`
# and `onevcs` are what it composes, `onejudge` is the verdict vocabulary it
# relays, and the two test-support pins are here because a double writing a
# stale shape is the same lie one build removed: it passes while proving a
# fixture. Every one of them is pinned in `[workspace.dependencies]`, which is
# what makes "the requirement already permitted it" checkable at all.
SIBLINGS=(oneagentgraph onevcs onevcs-testing onejudge oneharness-core)

format=check
manifest=Cargo.toml
lock=Cargo.lock
# Overridable so a mirror — or a test's fixture tree — can answer instead. The
# default is the registry cargo itself resolves from.
index="${ONEPIPELINE_CRATES_INDEX:-https://index.crates.io}"

usage="run 'linked-engines.sh [--format check|notes] [--manifest PATH] [--lock PATH] [--index URL_OR_DIR]'"

# Every option takes a value, so a missing one is an argument error rather than
# a silently empty setting.
need_value() {
  if [ "$#" -lt 2 ]; then
    echo "$1 needs a value" >&2
    echo "ACTION: $usage" >&2
    exit 2
  fi
}

# The tree could not be read at all, which says nothing about whether the lock
# is current — a distinct exit from the finding this check exists to report.
die() {
  echo "linked-engines: $1" >&2
  echo "ACTION: $2" >&2
  exit 3
}

while [ "$#" -gt 0 ]; do
  case "$1" in
    --format) need_value "$@"; format="$2"; shift 2 ;;
    --manifest) need_value "$@"; manifest="$2"; shift 2 ;;
    --lock) need_value "$@"; lock="$2"; shift 2 ;;
    --index) need_value "$@"; index="$2"; shift 2 ;;
    *)
      echo "unknown option $1" >&2
      echo "ACTION: $usage" >&2
      exit 2
      ;;
  esac
done

case "$format" in
  check|notes) ;;
  *)
    echo "unknown format '$format'" >&2
    echo "ACTION: $usage" >&2
    exit 2
    ;;
esac

[ -f "$manifest" ] || die "no manifest at '$manifest'" \
  "run this from the repository root, or pass '--manifest <path to Cargo.toml>'"
[ -f "$lock" ] || die "no lockfile at '$lock'" \
  "run this from the repository root, or pass '--lock <path to Cargo.lock>'"

# The requirement `[workspace.dependencies]` states for one engine. Read from
# that table alone: `[dependencies]` names the same engines as
# `{ workspace = true }`, and a match there would report the word "true" as a
# version requirement.
requirement() {
  awk -v want="$1" '
    /^\[/ { inside = ($0 ~ /^\[workspace\.dependencies\]/); next }
    !inside { next }
    {
      key = $0
      sub(/[[:space:]]*=.*$/, "", key)
      if (key != want) next
      if (match($0, /"[^"]*"/)) { print substr($0, RSTART + 1, RLENGTH - 2); exit }
    }
  ' "$manifest"
}

# Every version of one package the lock resolves. More than one is ordinary:
# two crates in the graph can require ranges that do not unify, and this
# workspace deliberately carries `oneharness-core` twice.
lock_versions() {
  awk -v want="$1" '
    $0 == "name = \"" want "\"" {
      getline line
      if (match(line, /"[^"]*"/)) print substr(line, RSTART + 1, RLENGTH - 2)
    }
  ' "$lock"
}

# Order two `X.Y.Z` versions: prints -1, 0 or 1. Registry and lock versions are
# always three numeric components, and a prerelease is dropped before it gets
# here, so this needs no ordering rule beyond the numbers.
ver_cmp() {
  local -a left right
  local a b i
  IFS=. read -r -a left <<<"$1"
  IFS=. read -r -a right <<<"$2"
  for i in 0 1 2; do
    a="${left[i]:-0}"
    b="${right[i]:-0}"
    if [ "$a" -lt "$b" ]; then echo -1; return; fi
    if [ "$a" -gt "$b" ]; then echo 1; return; fi
  done
  echo 0
}

ver_ge() { [ "$(ver_cmp "$1" "$2")" -ge 0 ]; }
ver_lt() { [ "$(ver_cmp "$1" "$2")" -lt 0 ]; }

# The window a requirement permits, printed as `lower upper` with `upper`
# exclusive — cargo's default `^` operator, whose 0.x rule is what this whole
# check turns on: `^0.3.0` permits every 0.3.z, so a lock at 0.3.6 with 0.3.9
# published is behind without the requirement having said anything.
#
# Refuses a requirement shape it does not model rather than guessing at one: a
# `~`, `=`, `>=`, `*` or comma-separated range here would be silently read as a
# caret and could report a currency this repository never claimed.
req_window() {
  local core major minor patch upper dots
  core="${1#^}"
  case "$core" in
    ""|*[!0-9.]*|*..*|.*|*.) return 1 ;;
  esac
  dots="${core//[^.]/}"
  [ "${#dots}" -le 2 ] || return 1
  IFS=. read -r major minor patch <<<"$core"
  if [ "$major" -ne 0 ]; then
    upper="$((major + 1)).0.0"
  elif [ -z "${minor:-}" ]; then
    upper="1.0.0"
  elif [ "$minor" -ne 0 ]; then
    upper="0.$((minor + 1)).0"
  elif [ -z "${patch:-}" ]; then
    upper="0.1.0"
  else
    upper="0.0.$((patch + 1))"
  fi
  printf '%s %s\n' "$major.${minor:-0}.${patch:-0}" "$upper"
}

# Where the sparse index files one crate, by the registry's own prefix rule.
index_path() {
  local n="$1"
  case "${#n}" in
    1) printf '1/%s\n' "$n" ;;
    2) printf '2/%s\n' "$n" ;;
    3) printf '3/%s/%s\n' "${n:0:1}" "$n" ;;
    *) printf '%s/%s/%s\n' "${n:0:2}" "${n:2:2}" "$n" ;;
  esac
}

# Every version the registry serves for one crate that a plain requirement can
# resolve to: yanked releases are not candidates, and neither are prereleases,
# which a requirement without one never matches.
#
# The index is one JSON object per line and `"vers"` and `"yanked"` each appear
# exactly once on it — a dependency entry carries `"req"`, not either of these —
# so the fields are read positionally rather than by standing up a JSON parser
# this script otherwise has no need of. `release.yml` reads the same file the
# same way to decide whether a version is already on crates.io.
index_versions() {
  local name="$1" path body attempt
  path="$(index_path "$name")"
  case "$index" in
    http://*|https://*)
      body=""
      for attempt in 1 2 3; do
        if body="$(curl -fsSL "$index/$path")"; then break; fi
        body=""
        # A registry read is the one part of this that can fail for reasons
        # that have nothing to do with the lock, so it is retried before it is
        # reported as unreadable.
        [ "$attempt" -eq 3 ] || sleep "$((attempt * 2))"
      done
      [ -n "$body" ] || die "the crates.io index at '$index' did not serve '$name'" \
        "check network reachability to '$index', or pass '--index' pointing at a mirror"
      ;;
    *)
      [ -f "$index/$path" ] || die "no index entry for '$name' under '$index'" \
        "pass '--index' pointing at a sparse-index tree that files '$name' at '$path'"
      body="$(cat "$index/$path")"
      ;;
  esac
  printf '%s\n' "$body" | awk '
    {
      vers = ""; yanked = "true"
      if (match($0, /"vers":"[^"]*"/)) vers = substr($0, RSTART + 8, RLENGTH - 9)
      if (match($0, /"yanked":(true|false)/)) yanked = substr($0, RSTART + 9, RLENGTH - 9)
      if (vers != "" && yanked == "false" && vers !~ /[-+]/) print vers
    }
  '
}

# One row per resolved copy, in the order the report prints them.
rows=()
# The subset of those rows that are behind, as `name resolved permitted`.
behind=()

for name in "${SIBLINGS[@]}"; do
  req="$(requirement "$name")"
  [ -n "$req" ] || die "'$name' has no requirement in [workspace.dependencies] of '$manifest'" \
    "add the pin there, or drop '$name' from SIBLINGS in this script if this repository no longer links it"

  if ! window="$(req_window "$req")"; then
    die "'$name = \"$req\"' is a requirement shape this check does not model" \
      "state the pin as a plain caret version (\"0.3.0\", \"0.12\"), or extend req_window() in this script to model the operator"
  fi
  read -r lower upper <<<"$window"

  governed=()
  ungoverned=()
  while read -r version; do
    [ -n "$version" ] || continue
    if ver_ge "$version" "$lower" && ver_lt "$version" "$upper"; then
      governed+=("$version")
    else
      ungoverned+=("$version")
    fi
  done < <(lock_versions "$name")

  [ "${#governed[@]}" -gt 0 ] || die "'$lock' resolves no '$name' that '$req' permits" \
    "the lock and the manifest disagree about '$name' — run 'cargo update --workspace' and commit the lock"

  permitted=""
  while read -r version; do
    if ver_ge "$version" "$lower" && ver_lt "$version" "$upper"; then
      if [ -z "$permitted" ] || ver_lt "$permitted" "$version"; then
        permitted="$version"
      fi
    fi
  done < <(index_versions "$name")
  [ -n "$permitted" ] || die "the index serves no '$name' version that '$req' permits" \
    "the requirement names a window the registry has nothing in — correct the pin in '$manifest'"

  for version in "${governed[@]}"; do
    if ver_lt "$version" "$permitted"; then
      rows+=("$name|$version|$req|$permitted|behind")
      behind+=("$name $version $permitted")
    else
      rows+=("$name|$version|$req|$permitted|current")
    fi
  done
  # A copy outside the window is in the build because another crate in the graph
  # requires it, so no requirement of this repository's is a claim about it.
  # Reported anyway: it is linked, and this is the answer to what is linked.
  for version in "${ungoverned[@]}"; do
    rows+=("$name|$version|$req|$permitted|transitive")
  done
done

if [ "$format" = notes ]; then
  # An HTML comment the release job trims from before re-appending, so a re-run
  # of a release replaces this section rather than stacking a second copy.
  echo "<!-- linked-engines -->"
  echo "### Linked engines"
  echo
  echo "The sibling engines this release actually links, resolved from its own \`Cargo.lock\`:"
  echo
  echo "| Engine | Linked | Requirement | Newest the requirement permits |"
  echo "| --- | --- | --- | --- |"
  for row in "${rows[@]}"; do
    IFS='|' read -r name version req permitted state <<<"$row"
    case "$state" in
      behind)
        echo "| \`$name\` | **$version** | \`$req\` | **$permitted** — this release is behind it |" ;;
      transitive)
        echo "| \`$name\` | $version | — | — (another crate in the graph requires this copy) |" ;;
      *)
        echo "| \`$name\` | $version | \`$req\` | $permitted |" ;;
    esac
  done
  echo
  if [ "${#behind[@]}" -eq 0 ]; then
    echo "Every linked engine is the newest its own requirement permits."
  else
    echo "> [!WARNING]"
    echo "> This release links an engine **older than its own requirement permits**, so reading"
    echo "> \`Cargo.toml\` overstates what this build contains:"
    echo ">"
    for entry in "${behind[@]}"; do
      read -r name version permitted <<<"$entry"
      echo "> - \`$name\` links $version; the requirement already permitted $permitted."
    done
  fi
  exit 0
fi

if [ "${#behind[@]}" -eq 0 ]; then
  summary=""
  for row in "${rows[@]}"; do
    IFS='|' read -r name version req permitted state <<<"$row"
    [ "$state" = current ] || continue
    summary="${summary:+$summary, }$name $version"
  done
  echo "linked engines are current: $summary — each the newest its own requirement permits"
  exit 0
fi

{
  echo "'$lock' links an engine older than '$manifest' already permits:"
  echo
  for entry in "${behind[@]}"; do
    read -r name version permitted <<<"$entry"
    echo "  $name: links $version, but its requirement already permits $permitted"
    echo "    fix: cargo update -p $name@$version"
  done
  echo
  echo "ACTION: run the update(s) above and commit the lock. The spec is version-qualified"
  echo "because this workspace carries an engine at two versions, where 'cargo update -p"
  echo "<name>' is refused as ambiguous. Then ask what is in the gap: if the newer engine"
  echo "carries behaviour this crate depends on, record that floor as a test beside"
  echo "'the_linked_oneagentgraph_produces_the_whole_turn_this_crate_relays' in"
  echo "src/agentgraph.rs, which is where this repository writes down *why* a floor matters."
  echo "This check only knows the lock is behind."
} >&2
exit 1
