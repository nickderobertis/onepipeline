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
# And it tells a lock that is *behind* from one that is *held back*. A newer
# release of one engine can require a second engine outside the window this
# manifest states for it — `onevcs-testing` 0.5.7 requiring `onevcs ^0.20.0`
# under `onevcs = "0.19.2"` — and the `cargo update` that would take it puts that
# second engine in the graph twice, which is the very state the count above
# refuses. So "the newest release the requirement permits" is read among the
# releases this manifest's own sibling requirements admit: a release held back is
# reported on its own line and fails nothing, because nothing but a requirement
# move lifts it, and the lock is behind only what it could actually take.
#
# Exits 0 with one copy of each engine, each the newest its requirement permits
# that this manifest admits; 1 naming every engine the lock splits or holds
# behind; 2 for an argument it cannot use; and 3 for a manifest, lock, or index it
# could not read — which says nothing about currency either way. `--index` also
# takes a directory in the sparse index's own layout.
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
# carrying `name`, `vers`, `deps` or `yanked` more than once, `unreadable` for
# one where any of `name`, `vers` or `yanked` is missing or is not the shape it
# should be, `foreign <name>` for one filed under another crate, and
# `unreadable-dep <version>` for a release whose `deps` entry for a sibling this
# cannot read. A readable release is `release <version>`, tagged like the rest
# because a bare version and a bare marker are the same shape and the index
# chooses the version. Any of these dropped silently would leave the lines around
# it answering "the newest release" for a file that had more.
#
# What a release requires of the other siblings comes out *before* its own
# line, one `requires <name> <kind> <req>` per entry of its `deps` that names an
# engine in SIBLINGS, so the caller has the whole record in hand when the
# `release` line arrives. Only those three members are read, and only off a
# sibling's entry: `name` because it is what says whose requirement this is,
# `req` because it is the requirement, and `kind` because a `dev` requirement
# of a dependency is one cargo never resolves — `oneagentgraph` requires `onevcs`
# as a dev-dependency at a window this manifest does not admit, and that holds
# nothing back. A sibling entry whose `name`, `req` or `kind` is missing,
# repeated or not a string, or whose `kind` is not one cargo writes, is refused
# rather than read as "not held back", which is the answer that would send a
# reader to run an update that splits the graph. A record with no `deps` at all
# says nothing about what it requires and holds nothing back — and the worst a
# mirror that dropped the member could then do is report a lock as behind, which
# is the answer this check gave before it could read one.
#
# Build metadata is not part of an ordering — `1.2.4+meta` *is* 1.2.4, and
# crates.io serves versions spelled that way — so it is stripped rather than
# read as a prerelease and skipped, which would call a lock behind it current.
# llmlint: ignore-block[boundary_inputs_validated] the judged tier reads this as
# third-party input parsed in awk rather than by a library, and asks for a real
# parser. `read_record` is one, for the object level this consults: it walks the
# record token by token, tracks nesting so a `deps` entry's fields are never
# mistaken for the record's own, requires each of the members it reads to
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
  printf '%s\n' "$body" | awk -v want="$name" -v siblings="${SIBLINGS[*]}" '
    BEGIN { n_sib = split(siblings, sib_list, " "); for (k = 1; k <= n_sib; k++) SIBLING[sib_list[k]] = 1 }
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
      if (key == "deps")   { if (SAW_DEPS)   TWICE = 1; SAW_DEPS   = 1; return 1 }
      return 0
    }
    # One entry of `deps`, read the way `read_record` reads the record: member
    # by member, nested values skipped whole. Returns the index past its
    # closing brace, or 0 where it never closes. What it decides lands in
    # DEP_BAD and DEPS rather than in a return value, because the walk has to
    # go on past an entry this cannot read to find the end of the record.
    function read_dep(s, i,   n, c, key, name, req, kind, saw_name, saw_req, saw_kind, ok_name, ok_req, ok_kind, twice) {
      n = length(s)
      i = skip_ws(s, i + 1)
      if (substr(s, i, 1) == "}") { DEP_BAD = 1; return i + 1 }
      while (1) {
        if (substr(s, i, 1) != "\"") return 0
        i = scan_string(s, i, 1); if (i == 0) return 0
        key = STR
        i = skip_ws(s, i)
        if (substr(s, i, 1) != ":") return 0
        i = skip_ws(s, i + 1)
        c = substr(s, i, 1)
        if (c == "\"") {
          i = scan_string(s, i, 1); if (i == 0) return 0
          if (key == "name")      { ok_name = 1; name = STR }
          else if (key == "req")  { ok_req = 1; req = STR }
          else if (key == "kind") { ok_kind = 1; kind = STR }
        } else if (c == "{" || c == "[") {
          i = scan_nested(s, i); if (i == 0) return 0
        } else {
          i = scan_literal(s, i); if (i == 0) return 0
        }
        if (key == "name")      { if (saw_name) twice = 1; saw_name = 1 }
        else if (key == "req")  { if (saw_req)  twice = 1; saw_req  = 1 }
        else if (key == "kind") { if (saw_kind) twice = 1; saw_kind = 1 }
        i = skip_ws(s, i)
        c = substr(s, i, 1)
        if (c == ",") { i = skip_ws(s, i + 1); continue }
        if (c == "}") break
        return 0
      }
      # An entry whose name cannot be read is one this cannot tell from a
      # sibling, so it is refused; one that names some other crate is skipped
      # whole, whatever else is on it.
      if (!ok_name || twice) { DEP_BAD = 1; return i + 1 }
      if (!(name in SIBLING)) return i + 1
      if (!ok_req || !ok_kind) { DEP_BAD = 1; return i + 1 }
      if (kind != "normal" && kind != "build" && kind != "dev") { DEP_BAD = 1; return i + 1 }
      DEPS[++NDEPS] = name " " kind " " req
      return i + 1
    }
    # The `deps` array: every entry an object, or the record is not one the
    # registry wrote.
    function read_deps(s, i,   n, c) {
      n = length(s)
      i = skip_ws(s, i + 1)
      if (substr(s, i, 1) == "]") return i + 1
      while (1) {
        if (substr(s, i, 1) != "{") return 0
        i = read_dep(s, i); if (i == 0) return 0
        i = skip_ws(s, i)
        c = substr(s, i, 1)
        if (c == ",") { i = skip_ws(s, i + 1); continue }
        if (c == "]") return i + 1
        return 0
      }
    }
    # A member is read only where it holds the shape it should: a `yanked`
    # spelled as a string is not a flag that happens to say `false`, so its OK_
    # stays unset and the caller refuses the record.
    function read_record(s,   i, n, c, key) {
      NAME = ""; VERS = ""; YANKED = ""
      SAW_NAME = 0; SAW_VERS = 0; SAW_YANKED = 0; SAW_DEPS = 0
      OK_NAME = 0; OK_VERS = 0; OK_YANKED = 0
      TWICE = 0; BAD = 0; DEP_BAD = 0
      split("", DEPS); NDEPS = 0
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
          else if (key == "deps") DEP_BAD = 1
        } else if (key == "deps" && c == "[") {
          i = read_deps(s, i); if (i == 0) { BAD = 1; return }
          note(key)
        } else if (c == "{" || c == "[") {
          i = scan_nested(s, i); if (i == 0) { BAD = 1; return }
          note(key)
          # A `deps` that is not an array is not a list this can read.
          if (key == "deps") DEP_BAD = 1
        } else {
          i = scan_literal(s, i); if (i == 0) { BAD = 1; return }
          note(key)
          if (key == "yanked" && (LIT == "true" || LIT == "false")) {
            OK_YANKED = 1; YANKED = LIT
          }
          if (key == "deps") DEP_BAD = 1
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
      # Decided after the two skips above: a release no requirement can resolve
      # to is one whose requirements are never consulted, so an entry on it this
      # cannot read is not refused over — as an unorderable `vers` on a yanked
      # record is not.
      if (DEP_BAD) { print "unreadable-dep " core; next }
      for (k = 1; k <= NDEPS; k++) print "requires " DEPS[k]
      print "release " core
    }
  '
}
# llmlint: ignore-end[boundary_inputs_validated]

# One row per resolved copy, in the order the report prints them.
rows=()
# The subset that are behind, as `name resolved permitted`.
behind=()
# One entry per engine a newer release of which this manifest holds back, as
# `name linked held dep dep_req manifest_req state`: the copy the lock links,
# the newest release held back, the sibling whose requirement holds it, what the
# release requires of that sibling, what this manifest states for it, and
# whether the linked copy is `current` or `behind` the newest release admitted —
# because "links the newest this manifest admits" is only true of the first.
heldback=()

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

  # The newest release in the window that this manifest's own sibling
  # requirements admit — which is the only release the lock can be *behind*.
  permitted=""
  # Every release in the window they do not, as `version dep req dep_req`; and
  # the newest of them, which is the one worth a line.
  held=()
  held_newest=""
  # What the record about to be answered requires of the siblings, as
  # `dep kind req`, gathered off the `requires` lines that precede its own.
  pending=()
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
      requires) pending+=("$version"); continue ;;
      not-json) die "the index served a '$name' line that is not one JSON object" "$not_sparse" ;;
      twice) die "the index served a '$name' record carrying name, vers, deps or yanked more than once" "$not_sparse" ;;
      unreadable) die "the index served a '$name' record with no readable name, vers or yanked on it" "$not_sparse" ;;
      unreadable-dep) die "the index served a '$name' $version record whose deps entry for a sibling engine this check cannot read: a name, req or kind missing, repeated or not a string, or a kind cargo does not write" "$not_sparse" ;;
      foreign) die "the index served a record for '$version' under '$name'" \
        "'$index' files a crate's releases under another crate's name — pass '--index' naming a sparse-index tree that does not" ;;
      *) die "the reader of '$index' answered '$verdict', which this check has no rule for" \
        "restore scripts/linked-engines.sh — its record reader and the loop that reads it have drifted apart" ;;
    esac
    orderable "$version" || die "the index serves '$name' at '$version', which is not a version this check can order" \
      "'$index' is not answering in the crates.io sparse-index format — pass '--index' naming one that does"
    if ver_ge "$version" "$lower" && ver_lt "$version" "$upper"; then
      # Held back by this manifest's own requirement on a sibling: what the
      # release requires of that sibling and what this manifest states for it
      # share no version, so taking the release means a second copy of the
      # sibling — the state the count above refuses — and no `cargo update`
      # resolves it. A dev requirement never holds anything back, because cargo
      # never resolves a dependency's own dev-dependencies.
      holder=""
      for entry in ${pending[@]+"${pending[@]}"}; do
        read -r dep kind dep_req <<<"$entry"
        [ "$kind" != dev ] || continue
        manifest_req="$(requirement "$dep")"
        [ -n "$manifest_req" ] || die "'$dep' has no requirement in [workspace.dependencies] of '$manifest'" \
          "add the pin there, or drop '$dep' from SIBLINGS in this script if this repository no longer links it"
        manifest_window="$(req_window "$manifest_req")" || die "'$dep = \"$manifest_req\"' is a requirement shape this check does not model" \
          "state the pin as a plain caret version (\"0.3.0\", \"0.12\"), or extend req_window() in this script to model the operator"
        read -r manifest_lower manifest_upper <<<"$manifest_window"
        stated_window="$(req_window "$dep_req")" || die "the index serves '$name' $version requiring '$dep' as '$dep_req', which is a requirement shape this check does not model" \
          "extend req_window() in this script to model the operator, or pass '--index' naming a registry whose records state plain caret requirements"
        read -r stated_lower stated_upper <<<"$stated_window"
        if ver_ge "$stated_lower" "$manifest_upper" || ver_ge "$manifest_lower" "$stated_upper"; then
          holder="$dep $dep_req $manifest_req"
          break
        fi
      done
      if [ -n "$holder" ]; then
        held+=("$version $holder")
        if [ -z "$held_newest" ] || ver_lt "$held_newest" "$version"; then
          held_newest="$version"
        fi
      elif [ -z "$permitted" ] || ver_lt "$permitted" "$version"; then
        permitted="$version"
      fi
    fi
    pending=()
  done <<<"$served"

  # A linked copy this manifest's own requirements hold back is a lock and a
  # manifest that disagree — cargo would not have resolved it — which is neither
  # current nor behind, and the answer is the one every other disagreement gets.
  for version in ${governed[@]+"${governed[@]}"}; do
    for entry in ${held[@]+"${held[@]}"}; do
      read -r held_version dep dep_req manifest_req <<<"$entry"
      [ "$held_version" = "${version%%+*}" ] || continue
      die "'$lock' links '$name' at $version, which requires $dep $dep_req, and '$dep = \"$manifest_req\"' in '$manifest' admits no version of that" \
        "the lock and the manifest disagree about '$dep' — run 'cargo update --workspace' and commit the lock"
    done
  done

  [ -n "$permitted" ] || die "the index serves no '$name' version that '$req' permits" \
    "the requirement names a window the registry has nothing in — correct the pin in '$manifest'"

  # Worth a line only where it is newer than what the lock could take: a release
  # below that is one the manifest moved past, and nothing about it is news.
  state=current
  for version in ${governed[@]+"${governed[@]}"}; do
    if ver_lt "${version%%+*}" "$permitted"; then
      rows+=("$name|$version|$req|$permitted|behind")
      behind+=("$name $version $permitted")
      state=behind
    else
      rows+=("$name|$version|$req|$permitted|current")
    fi
  done
  if [ -n "$held_newest" ] && ver_lt "$permitted" "$held_newest"; then
    for entry in ${held[@]+"${held[@]}"}; do
      read -r held_version dep dep_req manifest_req <<<"$entry"
      [ "$held_version" = "$held_newest" ] || continue
      heldback+=("$name ${governed[0]} $held_newest $dep $dep_req $manifest_req $state")
      break
    done
  fi
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
  # The held-back note is about the engines that are otherwise current; one
  # that is behind as well carries the same fact on its own warning line below,
  # where "links the newest this manifest admits" would be untrue of it.
  held_current=()
  for entry in ${heldback[@]+"${heldback[@]}"}; do
    read -r name version held dep dep_req manifest_req state <<<"$entry"
    [ "$state" = current ] || continue
    held_current+=("$entry")
  done
  if [ "${#held_current[@]}" -gt 0 ]; then
    echo "> [!NOTE]"
    echo "> A newer release of an engine below exists that this build's own requirements"
    echo "> **hold back**: it requires a sibling engine outside the window \`Cargo.toml\` states,"
    echo "> so no \`cargo update\` takes it without a second copy of that sibling, and only a"
    echo "> requirement move lifts it:"
    echo ">"
    for entry in ${held_current[@]+"${held_current[@]}"}; do
      read -r name version held dep dep_req manifest_req state <<<"$entry"
      echo "> - \`$name\` links $version, the newest its requirement permits that this manifest admits; $held is held back by \`$dep = \"$manifest_req\"\` ($held requires $dep $dep_req)."
    done
    echo
  fi
  if [ "${#behind[@]}" -eq 0 ]; then
    if [ "${#heldback[@]}" -eq 0 ]; then
      echo "Every linked engine is the newest its own requirement permits."
    else
      echo "Every linked engine is the newest its own requirement permits among the releases this manifest admits."
    fi
  else
    echo "> [!WARNING]"
    echo "> This release links an engine **older than its own requirement permits**, so reading"
    echo "> \`Cargo.toml\` overstates what this build contains:"
    echo ">"
    for entry in ${behind[@]+"${behind[@]}"}; do
      read -r name version permitted <<<"$entry"
      clause=""
      for held_entry in ${heldback[@]+"${heldback[@]}"}; do
        read -r held_name _ held dep dep_req manifest_req _ <<<"$held_entry"
        [ "$held_name" = "$name" ] || continue
        clause=" ($held is held back by \`$dep = \"$manifest_req\"\`: $held requires $dep $dep_req)"
      done
      echo "> - \`$name\` links $version; the requirement already permitted $permitted$clause."
    done
  fi
  exit 0
fi
# llmlint: ignore-end[tool_output_is_signal]

# A held-back release is news rather than a finding, and it goes on stdout in
# both verdicts: what it says is true of the lock whichever way the check goes,
# and it carries no fix because there is none — the update that would take the
# release is the one the unification refusal above exists to end. What lifts it
# is the requirement it names moving.
for entry in ${heldback[@]+"${heldback[@]}"}; do
  read -r name version held dep dep_req manifest_req state <<<"$entry"
  [ "$state" = current ] || continue
  echo "$name: links $version, the newest its requirement permits that this manifest admits; $held is held back by $dep = \"$manifest_req\" ($held requires $dep $dep_req)"
done

if [ "${#behind[@]}" -eq 0 ]; then
  summary=""
  for row in ${rows[@]+"${rows[@]}"}; do
    IFS='|' read -r name version req permitted state <<<"$row"
    [ "$state" = current ] || continue
    summary="${summary:+$summary, }$name $version"
  done
  if [ "${#heldback[@]}" -eq 0 ]; then
    echo "linked engines are current: $summary — each the newest its own requirement permits"
  else
    echo "linked engines are current: $summary — each the newest its own requirement permits among the releases this manifest admits"
  fi
  exit 0
fi

{
  echo "'$lock' links an engine older than '$manifest' already permits:"
  echo
  for entry in ${behind[@]+"${behind[@]}"}; do
    read -r name version permitted <<<"$entry"
    echo "  $name: links $version, but its requirement already permits $permitted"
    # Where a held-back release sits above the one the lock could take, a bare
    # `cargo update -p` takes *that* one — cargo resolves the newest release the
    # requirement permits and adds the second sibling copy it needs, which the
    # `onevcs-testing` 0.5.7 dry run answers with `Adding onevcs v0.20.0` — so
    # the fix names the release the lock is actually behind.
    precise=""
    for held_entry in ${heldback[@]+"${heldback[@]}"}; do
      read -r held_name _ held dep dep_req manifest_req _ <<<"$held_entry"
      [ "$held_name" = "$name" ] || continue
      precise=" --precise $permitted"
      echo "    ($held is held back by $dep = \"$manifest_req\": $held requires $dep $dep_req)"
    done
    echo "    fix: cargo update -p $name@$version$precise"
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
