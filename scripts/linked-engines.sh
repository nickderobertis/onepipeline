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
# It answers a second question first, and offline: **how many** copies of each
# engine the lock resolves. Currency is about which release is linked; that one
# is about whether "the release this build links" is a question with one answer
# at all.
#
# Exits 0 with one current copy of each engine, 1 naming every engine the lock
# splits or holds behind, 2 for an argument it cannot use, and 3 for a manifest,
# lock, or index it could not read — which says nothing about currency either
# way. `--index` also takes a directory in the sparse index's own layout.
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
  [ "${1//[^.]/}" = ".." ] || return 1
  # And a number the comparison below can actually make. Bash's integers are
  # 64-bit, and `[ 99999999999999999999 -lt 1 ]` is not false but an *error*:
  # `ver_cmp` reads that as neither less nor greater and answers "equal", which
  # orders an unreadable version against every real one and is precisely the
  # currency this must never claim. Eighteen digits is past every release the
  # registry has ever served and short of where `[` stops answering.
  local IFS=. part
  for part in $1; do
    [ "${#part}" -le 18 ] || return 1
  done
}

# The requirement `[workspace.dependencies]` states for one engine.
#
# Read from that table alone: `[dependencies]` names the same engines as
# `{ workspace = true }`, and a match there would report the word "true" as a
# version requirement. The whole declaration must be `name = "..."` — a table
# (`{ version = "1", path = "..." }`) yields nothing here, and the caller
# refuses by name, rather than the first quoted substring in it being read as
# the requirement. `req_window` then decides whether the string is one this
# check models.
requirement() {
  awk -v want="$1" '
    /^\[/ { inside = ($0 ~ /^\[workspace\.dependencies\]/); next }
    !inside { next }
    $0 ~ "^" want "[[:space:]]*=[[:space:]]*\"[^\"]*\"[[:space:]]*$" {
      match($0, /"[^"]*"/)
      print substr($0, RSTART + 1, RLENGTH - 2)
      exit
    }
  ' "$manifest"
}

# Every version of one package the lock resolves. More than one is what the
# unification refusal below is for: two crates in the graph required ranges that
# did not unify, so the graph carries the engine twice.
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
# The index is one JSON object per line, walked rather than searched: `"name"`
# is *not* a field that appears once on a record — every entry of its `deps`
# array carries one too — so counting the string across the line reads every
# record crates.io actually serves as unreadable. `read_record` below walks the
# outermost object and reads only its own members, skipping a nested object or
# array whole.
#
# What comes back is still a third party's, so this answers in tagged lines and
# each way a record can be unreadable gets its own tag, which the caller refuses
# by name: `not-json` for a line that is not one JSON object, `twice` for one
# carrying `name`, `vers` or `yanked` more than once, `unreadable` for one where
# any of the three is missing or is not the shape it should be, and
# `foreign <name>` for one filed under another crate. A readable release is
# `release <version>`, tagged like the rest because a bare version and a bare
# marker are the same shape and the index chooses the version. Any of these
# dropped silently would leave the lines around it answering "the newest release"
# for a file that had more.
#
# Build metadata is not part of an ordering — `1.2.4+meta` *is* 1.2.4, and
# crates.io serves versions spelled that way — so it is stripped rather than
# read as a prerelease and skipped, which would call a lock behind it current.
# llmlint: ignore-block[boundary_inputs_validated] the judged tier reads this as
# third-party input parsed in awk rather than by a library, and asks for a real
# parser. `read_record` is one, for the object level this consults: it walks the
# record token by token, tracks nesting so a `deps` entry's fields are never
# mistaken for the record's own, requires each of the three members it reads to
# appear exactly once and to hold the type it should, and refuses the record
# otherwise. It is checked against `json.loads` over all five engines' real index
# files, and the shapes it refuses are driven in `tests/linked_engines.rs`. The
# library the rule asks for does not exist in bash — the language this has to be
# in, because a per-change workflow and a release job both reach it through a recipe
# — and shelling out to one adds an interpreter to a release job whose whole
# purpose is to be reachable without a build.
index_versions() {
  local name="$1" path body attempt
  path="$(index_path "$name")"
  case "$index" in
    http://*|https://*)
      body=""
      # A registry read is the one part of this that fails for reasons having
      # nothing to do with the lock, so it is retried — and curl's own diagnosis
      # is held here rather than left on stderr, because an attempt a later one
      # recovers from is not something this check should report.
      errors="$(mktemp)"
      for attempt in 1 2 3; do
        if body="$(curl -fsSL "$index/$path" 2>"$errors")"; then break; fi
        body=""
        [ "$attempt" -eq 3 ] || sleep "$attempt"
      done
      if [ -z "$body" ]; then
        detail="$(tr '\n' ' ' <"$errors")"
        rm -f "$errors"
        die "the crates.io index at '$index' did not serve '$name': $detail" \
          "check reachability of '$index', or pass '--index' naming a mirror or a local sparse-index tree"
      fi
      rm -f "$errors"
      ;;
    *)
      [ -f "$index/$path" ] || die "no index entry for '$name' under '$index'" \
        "pass '--index' naming a sparse-index tree that files '$name' at '$path'"
      body="$(cat "$index/$path")"
      ;;
  esac
  printf '%s\n' "$body" | awk -v want="$name" '
    function skip_ws(s, i,   c) {
      while (i <= length(s)) {
        c = substr(s, i, 1)
        if (c != " " && c != "\t" && c != "\r" && c != "\n") break
        i++
      }
      return i
    }
    # Contents in STR when `keep`, 0 when it never closes. What an escape encodes
    # is never decoded: a name or a version carrying one is not a name or a
    # version this can read, and the refusals below say so by name.
    function scan_string(s, i, keep,   n, c) {
      n = length(s); STR = ""
      for (i++; i <= n; i++) {
        c = substr(s, i, 1)
        if (c == "\\") { if (keep) STR = STR substr(s, i, 2); i++; continue }
        if (c == "\"") return i + 1
        if (keep) STR = STR c
      }
      return 0
    }
    # Skipping the value whole is what keeps the `name` on a `deps` entry from
    # being read as the one on the record. (No apostrophes below: the whole
    # program is one shell single-quoted string, which any would end.)
    function scan_nested(s, i,   n, c, depth) {
      n = length(s); depth = 0
      while (i <= n) {
        c = substr(s, i, 1)
        if (c == "\"") { i = scan_string(s, i, 0); if (i == 0) return 0; continue }
        if (c == "{" || c == "[") depth++
        else if (c == "}" || c == "]") { depth--; if (depth <= 0) return i + 1 }
        i++
      }
      return 0
    }
    function scan_literal(s, i,   n, c, start) {
      n = length(s); start = i
      while (i <= n) {
        c = substr(s, i, 1)
        if (c == "," || c == "}" || c == "]" || c == " " || c == "\t" ||
            c == "\r" || c == "\n") break
        i++
      }
      LIT = substr(s, start, i - start)
      return (LIT == "") ? 0 : i
    }
    # Presence is noted apart from readability, so a second copy is caught even
    # where the first was the only readable one — and taking the first would
    # answer for a record that went on to say something else.
    function note(key) {
      if (key == "name")   { if (SAW_NAME)   TWICE = 1; SAW_NAME   = 1; return 1 }
      if (key == "vers")   { if (SAW_VERS)   TWICE = 1; SAW_VERS   = 1; return 1 }
      if (key == "yanked") { if (SAW_YANKED) TWICE = 1; SAW_YANKED = 1; return 1 }
      return 0
    }
    # A member is read only where it holds the shape it should: a `yanked`
    # spelled as a string is not a flag that happens to say `false`, so its OK_
    # stays unset and the caller refuses the record.
    function read_record(s,   i, n, c, key) {
      NAME = ""; VERS = ""; YANKED = ""
      SAW_NAME = 0; SAW_VERS = 0; SAW_YANKED = 0
      OK_NAME = 0; OK_VERS = 0; OK_YANKED = 0
      TWICE = 0; BAD = 0
      n = length(s)
      i = skip_ws(s, 1)
      if (substr(s, i, 1) != "{") { BAD = 1; return }
      i = skip_ws(s, i + 1)
      if (substr(s, i, 1) == "}") { if (skip_ws(s, i + 1) <= n) BAD = 1; return }
      while (1) {
        if (substr(s, i, 1) != "\"") { BAD = 1; return }
        i = scan_string(s, i, 1); if (i == 0) { BAD = 1; return }
        key = STR
        i = skip_ws(s, i)
        if (substr(s, i, 1) != ":") { BAD = 1; return }
        i = skip_ws(s, i + 1)
        c = substr(s, i, 1)
        if (c == "\"") {
          i = scan_string(s, i, (key == "name" || key == "vers"))
          if (i == 0) { BAD = 1; return }
          note(key)
          if (key == "name") { OK_NAME = 1; NAME = STR }
          else if (key == "vers") { OK_VERS = 1; VERS = STR }
        } else if (c == "{" || c == "[") {
          i = scan_nested(s, i); if (i == 0) { BAD = 1; return }
          note(key)
        } else {
          i = scan_literal(s, i); if (i == 0) { BAD = 1; return }
          note(key)
          if (key == "yanked" && (LIT == "true" || LIT == "false")) {
            OK_YANKED = 1; YANKED = LIT
          }
        }
        i = skip_ws(s, i)
        c = substr(s, i, 1)
        if (c == ",") { i = skip_ws(s, i + 1); continue }
        if (c == "}") { if (skip_ws(s, i + 1) <= n) BAD = 1; return }
        BAD = 1; return
      }
    }
    /^[[:space:]]*$/ { next }
    # One tagged line per record, because a version is whatever the index chose
    # to serve: a marker spelled as a bare character is a marker a `vers` could
    # also be, and the caller would then report the wrong one of these. The tag
    # comes first and holds no spaces, so `read -r verdict rest` splits it.
    {
      read_record($0)
      if (BAD) { print "not-json"; next }
      if (TWICE) { print "twice"; next }
      if (!OK_NAME || !OK_VERS || !OK_YANKED) { print "unreadable"; next }
      if (NAME != want) { print "foreign " NAME; next }
      if (YANKED == "true") next
      core = VERS
      sub(/\+.*$/, "", core)
      if (core ~ /-/) next
      print "release " core
    }
  '
}
# llmlint: ignore-end[boundary_inputs_validated]

# One row per resolved copy, in the order the report prints them.
rows=()
# The subset that are behind, as `name resolved permitted`.
behind=()

# Every array this file fills is expanded below as `${a[@]+"${a[@]}"}` rather
# than as `"${a[@]}"`. macOS ships bash 3.2 — the last GPLv2 release, and what
# this repository's own `cross (macos-latest)` leg runs this script on — and
# under `set -u` that bash treats `"${a[@]}"` on an *empty* array as an unbound
# variable and aborts, where 4.4 and later expand it to nothing. An engine with
# no copy outside its window is the ordinary case rather than an error, so the
# one form that means "the elements, or nothing" on both bashes is used at every
# such site, not only at the one that found this. Each element survives whole:
# the inner expansion is quoted, so a row carrying spaces stays one word.

split=()

# One copy of each engine, asked before the registry is asked anything.
#
# Asked first, and off the network, for two reasons. A split graph makes the
# currency question below ambiguous rather than merely unanswered — "the release
# this build links" has two answers — so there is nothing worth asking the index
# until it is one. And the refusal is then reachable with no index at all, which
# is what lets the deterministic tier drive it.
for name in "${SIBLINGS[@]}"; do
  # Captured before it is read, for the reason the same read is below: `die`
  # inside a process substitution exits that subshell alone, so a loop fed by
  # one would see an unreadable lock as a lock with nothing in it.
  resolved="$(lock_versions "$name")" || die "'$lock' could not be read for '$name'" \
    "make '$lock' readable, or pass '--lock <path to Cargo.lock>'"
  copies=()
  while read -r version; do
    [ -n "$version" ] || continue
    copies+=("$version")
  done <<<"$resolved"
  [ "${#copies[@]}" -le 1 ] || split+=("$name ${copies[*]}")
done

# The check refuses; `--format notes` reports, for the reason it reports an
# engine that is behind rather than refusing to compose notes at all — a release
# whose graph split is the one whose notes should least be silent about it.
if [ "$format" = check ] && [ "${#split[@]}" -gt 0 ]; then
  {
    echo "'$lock' resolves an engine at more than one version:"
    echo
    for entry in ${split[@]+"${split[@]}"}; do
      read -r name versions <<<"$entry"
      echo "  $name: $versions"
    done
    echo
    echo "ACTION: find the crate whose requirement pins the older copy — 'cargo tree"
    echo "--invert --package <name>@<version>' names it — and move it, or move the pin in"
    echo "'$manifest' that no longer unifies with it. A copy of an engine that only one"
    echo "half of the graph can reach is a fix the other half does not have, and no"
    echo "currency check can see it: each copy is separately current."
  } >&2
  exit 1
fi

for name in "${SIBLINGS[@]}"; do
  req="$(requirement "$name")"
  [ -n "$req" ] || die "'$name' has no requirement in [workspace.dependencies] of '$manifest'" \
    "add the pin there, or drop '$name' from SIBLINGS in this script if this repository no longer links it"

  if ! window="$(req_window "$req")"; then
    die "'$name = \"$req\"' is a requirement shape this check does not model" \
      "state the pin as a plain caret version (\"0.3.0\", \"0.12\"), or extend req_window() in this script to model the operator"
  fi
  read -r lower upper <<<"$window"

  # A reading that failed is not a reading that found nothing. `die` inside a
  # process substitution exits that subshell alone, so a loop fed by one sees an
  # empty answer and the check then refuses for the wrong reason — telling a
  # reader whose registry was unreachable to correct a pin that is correct.
  # Both answers are captured before they are read, so a reading that failed
  # ends the run instead of being mistaken for one that found nothing.
  resolved="$(lock_versions "$name")" || die "'$lock' could not be read for '$name'" \
    "make '$lock' readable, or pass '--lock <path to Cargo.lock>'"

  governed=()
  ungoverned=()
  while read -r version; do
    [ -n "$version" ] || continue
    # Build metadata is not part of an ordering — `1.2.4+meta` *is* 1.2.4 — and
    # cargo writes it into the lock whenever a crate publishes that way, which
    # `index_versions` already strips on the registry's side. So the comparison
    # reads the release and the report keeps what the lock spells, which is what
    # makes the `cargo update -p <name>@<version>` printed below name a copy
    # that is actually there.
    core="${version%%+*}"
    orderable "$core" || die "'$lock' resolves '$name' at '$version', which is not a version this check can order" \
      "the lockfile is not one cargo wrote — regenerate it with 'cargo update --workspace'"
    if ver_ge "$core" "$lower" && ver_lt "$core" "$upper"; then
      governed+=("$version")
    else
      ungoverned+=("$version")
    fi
  done <<<"$resolved"

  [ "${#governed[@]}" -gt 0 ] || die "'$lock' resolves no '$name' that '$req' permits" \
    "the lock and the manifest disagree about '$name' — run 'cargo update --workspace' and commit the lock"

  # `index_versions` has already named what failed and what to do about it, so
  # this only has to stop rather than write a second diagnosis over the first.
  served="$(index_versions "$name")" || exit 3

  permitted=""
  # Each tag `index_versions` emits names a different way the record was
  # unreadable, so the refusal says what it saw rather than that something was
  # wrong with it. A tag this does not know is a reader and a caller that have
  # drifted apart, which is refused rather than skipped: skipped, it would leave
  # the lines around it answering "the newest release" for a file that had more.
  not_sparse="'$index' is not answering in the crates.io sparse-index format — pass '--index' naming one that does"
  while read -r verdict version; do
    [ -n "$verdict" ] || continue
    case "$verdict" in
      release) ;;
      not-json) die "the index served a '$name' line that is not one JSON object" "$not_sparse" ;;
      twice) die "the index served a '$name' record carrying name, vers or yanked more than once" "$not_sparse" ;;
      unreadable) die "the index served a '$name' record with no readable name, vers or yanked on it" "$not_sparse" ;;
      foreign) die "the index served a record for '$version' under '$name'" \
        "'$index' files a crate's releases under another crate's name — pass '--index' naming a sparse-index tree that does not" ;;
      *) die "the reader of '$index' answered '$verdict', which this check has no rule for" \
        "restore scripts/linked-engines.sh — its record reader and the loop that reads it have drifted apart" ;;
    esac
    orderable "$version" || die "the index serves '$name' at '$version', which is not a version this check can order" \
      "'$index' is not answering in the crates.io sparse-index format — pass '--index' naming one that does"
    if ver_ge "$version" "$lower" && ver_lt "$version" "$upper"; then
      if [ -z "$permitted" ] || ver_lt "$permitted" "$version"; then
        permitted="$version"
      fi
    fi
  done <<<"$served"
  [ -n "$permitted" ] || die "the index serves no '$name' version that '$req' permits" \
    "the requirement names a window the registry has nothing in — correct the pin in '$manifest'"

  for version in ${governed[@]+"${governed[@]}"}; do
    if ver_lt "${version%%+*}" "$permitted"; then
      rows+=("$name|$version|$req|$permitted|behind")
      behind+=("$name $version $permitted")
    else
      rows+=("$name|$version|$req|$permitted|current")
    fi
  done
  # A copy outside the window is in the build because another crate in the graph
  # requires it, so no requirement of this repository's is a claim about it.
  # Reported anyway: it is linked, and this is the answer to what is linked.
  for version in ${ungoverned[@]+"${ungoverned[@]}"}; do
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
  for row in ${rows[@]+"${rows[@]}"}; do
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
  if [ "${#split[@]}" -gt 0 ]; then
    echo "> [!WARNING]"
    echo "> This release links an engine at **more than one version**, so which copy a"
    echo "> given crate in the graph reaches is decided by its own requirement rather"
    echo "> than by anything recorded here:"
    echo ">"
    for entry in ${split[@]+"${split[@]}"}; do
      read -r name versions <<<"$entry"
      echo "> - \`$name\` links $versions."
    done
    echo
  fi
  if [ "${#behind[@]}" -eq 0 ]; then
    echo "Every linked engine is the newest its own requirement permits."
  else
    echo "> [!WARNING]"
    echo "> This release links an engine **older than its own requirement permits**, so reading"
    echo "> \`Cargo.toml\` overstates what this build contains:"
    echo ">"
    for entry in ${behind[@]+"${behind[@]}"}; do
      read -r name version permitted <<<"$entry"
      echo "> - \`$name\` links $version; the requirement already permitted $permitted."
    done
  fi
  exit 0
fi
# llmlint: ignore-end[tool_output_is_signal]

if [ "${#behind[@]}" -eq 0 ]; then
  summary=""
  for row in ${rows[@]+"${rows[@]}"}; do
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
  for entry in ${behind[@]+"${behind[@]}"}; do
    read -r name version permitted <<<"$entry"
    echo "  $name: links $version, but its requirement already permits $permitted"
    echo "    fix: cargo update -p $name@$version"
  done
  echo
  echo "ACTION: run the update(s) above and commit the lock. The spec is version-qualified"
  echo "because that is what names the copy this lock actually holds, and it is the only"
  echo "spelling cargo accepts where a graph carries an engine twice — which is a state"
  echo "the unification refusal above ends rather than one this has to survive. Then ask"
  echo "what is in the gap: if the newer engine"
  echo "carries behaviour this crate depends on, record that floor as a test beside"
  echo "'the_linked_oneagentgraph_produces_the_whole_turn_this_crate_relays' in"
  echo "src/agentgraph.rs, which is where this repository writes down *why* a floor matters."
  echo "This check only knows the lock is behind."
} >&2
exit 1
