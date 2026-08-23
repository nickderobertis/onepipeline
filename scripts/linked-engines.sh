#!/usr/bin/env bash
# What this build links, held to what its own manifest already permits.
#
# `Cargo.lock` can resolve a sibling engine older than the requirement beside it
# allows, and nothing a reader reaches — the tag, the changelog, the requirement
# — disagrees with them; that has shipped three times. So this reads the lock,
# asks the crates.io sparse index what each requirement permits today, and
# reports the difference: `--format check` as a gate, `--format notes` as the
# markdown a release's notes carry.
#
# Exits 0 with nothing behind, 1 naming every engine that is, 2 for an argument
# it cannot use, and 3 for a manifest, lock, or index it could not read — which
# says nothing about currency either way. `--index` also takes a directory in
# the sparse index's own layout.
#
# Usage:
#   linked-engines.sh [--format check|notes] [--manifest PATH] [--lock PATH]
#                     [--index URL_OR_DIR]
set -euo pipefail

# The engines whose currency this repository claims: the two it composes, the
# verdict vocabulary it relays, and the two test-support pins whose drift would
# leave a double proving a fixture. Every one is pinned in
# `[workspace.dependencies]`, which is what makes the claim checkable at all.
SIBLINGS=(oneagentgraph onevcs onevcs-testing onejudge oneharness-core)

format=check
manifest=Cargo.toml
lock=Cargo.lock
# Overridable so a mirror — or a test's fixture tree — answers instead. The
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

# A version this check can order: exactly three numeric components, which is
# what the registry and the lock both write. Everything ordered below is checked
# against this first — a string from a file this script did not write reaching
# the numeric comparison is the one way this could report a currency it never
# established.
orderable() {
  case "$1" in
    ""|*[!0-9.]*|*..*|.*|*.) return 1 ;;
  esac
  [ "${1//[^.]/}" = ".." ]
}

# The requirement `[workspace.dependencies]` states for one engine. Read from
# that table alone: `[dependencies]` names the same engines as
# `{ workspace = true }`, and a match there would report the word "true" as a
# version requirement. What comes back is validated by `req_window` before it
# decides anything, so a value of the wrong type is refused rather than guessed.
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

# Every version of one package the lock resolves. More than one is ordinary: two
# crates in the graph can require ranges that do not unify, and this workspace
# deliberately carries `oneharness-core` twice.
#
# The `version = "..."` line is required rather than assumed, so a lock whose
# shape is not the one cargo writes yields nothing here — which the caller
# refuses by name — instead of a neighbouring field read as a version.
lock_versions() {
  awk -v want="$1" '
    $0 == "name = \"" want "\"" {
      getline line
      if (match(line, /^version = "[^"]*"$/)) {
        print substr(line, 12, length(line) - 12)
      }
    }
  ' "$lock"
}

# Order two versions: prints -1, 0 or 1. Both are `orderable` before they get
# here, so this needs no rule beyond the numbers.
ver_cmp() {
  local -a left right
  local a b i
  IFS=. read -r -a left <<<"$1"
  IFS=. read -r -a right <<<"$2"
  for i in 0 1 2; do
    a="${left[i]}"
    b="${right[i]}"
    if [ "$a" -lt "$b" ]; then echo -1; return; fi
    if [ "$a" -gt "$b" ]; then echo 1; return; fi
  done
  echo 0
}

ver_ge() { [ "$(ver_cmp "$1" "$2")" -ge 0 ]; }
ver_lt() { [ "$(ver_cmp "$1" "$2")" -lt 0 ]; }

# The window a requirement permits, as `lower upper` with `upper` exclusive —
# cargo's default `^`, whose 0.x rule is what this whole check turns on: `^0.3.0`
# permits every 0.3.z, so a lock at 0.3.6 with 0.3.9 published is behind without
# the requirement having said anything.
#
# Refuses a shape it does not model rather than guessing: a `~`, `=`, `>=`, `*`
# or comma-separated range read as a caret could report a currency this
# repository never claimed.
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

# Where the sparse index files one crate. The registry has shorter forms for
# one-, two- and three-character names; nothing in SIBLINGS is that short, and
# one that were would fail the read below by name rather than silently.
index_path() {
  printf '%s/%s/%s\n' "${1:0:2}" "${1:2:2}" "$1"
}

# Every version the registry serves for one crate that a plain requirement can
# resolve to: a yanked release is not a candidate, and neither is a prerelease,
# which a requirement without one never matches.
#
# The index is one JSON object per line and `"vers"` and `"yanked"` each appear
# exactly once on it — a dependency entry carries `"req"`, not either — so the
# fields are read positionally rather than by standing up a JSON parser this
# script otherwise has no need of; `release.yml` reads the same file the same
# way. What comes back is a third party's, so every version of it is `orderable`
# before it is compared and the caller refuses the file outright if one is not.
index_versions() {
  local name="$1" path body attempt
  path="$(index_path "$name")"
  case "$index" in
    http://*|https://*)
      body=""
      for attempt in 1 2 3; do
        if body="$(curl -fsSL "$index/$path")"; then break; fi
        body=""
        # A registry read is the one part of this that fails for reasons having
        # nothing to do with the lock, so it is retried before it is reported.
        [ "$attempt" -eq 3 ] || sleep "$attempt"
      done
      [ -n "$body" ] || die "the crates.io index at '$index' did not serve '$name'" \
        "check reachability of '$index', or pass '--index' naming a mirror or a local sparse-index tree"
      ;;
    *)
      [ -f "$index/$path" ] || die "no index entry for '$name' under '$index'" \
        "pass '--index' naming a sparse-index tree that files '$name' at '$path'"
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
# The subset that are behind, as `name resolved permitted`.
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
    orderable "$version" || die "'$lock' resolves '$name' at '$version', which is not a version this check can order" \
      "the lockfile is not one cargo wrote — regenerate it with 'cargo update --workspace'"
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
    [ -n "$version" ] || continue
    orderable "$version" || die "the index serves '$name' at '$version', which is not a version this check can order" \
      "'$index' is not answering in the crates.io sparse-index format — pass '--index' naming one that does"
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

# llmlint: ignore-block[tool_output_is_signal] the document below *is* this mode's
# product — `--format notes` asks for it, `release.yml` captures the stdout whole and
# appends it to a Release body, and there is no shorter form of "which version of each
# engine did this release link". The check mode, which is the one a gate reads, keeps
# to a line.
if [ "$format" = notes ]; then
  # An HTML comment the release job trims from before re-appending, so a re-run
  # replaces this section rather than stacking a second copy.
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
# llmlint: ignore-end[tool_output_is_signal]

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
