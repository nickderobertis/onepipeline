#!/usr/bin/env bash
# What a public registry currently serves for one artifact this repository
# publishes.
#
# **What it answers for is not written here.** It reads the `[[target]]`
# identifiers out of `release-targets.toml` at this repository's root, which is
# also the document naming this script as its `probe`.
#
# A run of this crate holds a node under `published` adoption until every
# dependency it names has *released* the work it depends on — see
# `src/release.rs`. That hold is answered, per repository, by that repository's
# own probe, and this is this repository's. It is the release-target contract's
# `script` form and is spawned exactly as `src/release.rs` spawns one: a direct
# subprocess, no shell interposed, this repository's root as the working
# directory, and an environment carrying `PATH` and `HOME` (plus the two Windows
# equivalents) and nothing else — no credential of any kind. Every target the
# declaration names is on a public registry, so an unauthenticated read is all
# this needs and all it may have.
#
# **Three answers, and the third is not the second.**
#
#   * exit 0, one line on stdout — that is the version the registry serves now;
#   * exit 0, nothing on stdout — the registry has no release of it yet;
#   * any non-zero exit, with the reason on stderr — **not answered**.
#
# A consumer holds indefinitely on the third and must never read it as evidence
# that a release has not happened, so nothing here answers "no release" for a
# question it could not put: an identifier this does not recognise, a registry
# that did not answer, and an answer it could not read are each non-zero. The
# exit codes are 2 for the identifier, 3 for the registry, and 4 for this
# repository's own declaration when this cannot read it; all three are "not
# answered", and a caller needs no more than "non-zero".
#
# It answers well inside the sixty seconds the contract allows: one request, at
# most three attempts of `TIMEOUT` seconds with a one- and two-second backoff
# between them.
#
# Usage:
#   release-probe.sh <registry>:<name>
set -euo pipefail

# The declaration, beside this script rather than under the caller's working
# directory. The host spawns a probe with this repository's root as its working
# directory, but the file this reads is one a *checkout* carries, and its place in
# that checkout is fixed relative to this script — so anchoring on `$0` answers
# the same document however the script was reached.
case "$0" in
  */*) here="${0%/*}" ;;
  *) here="." ;;
esac
declaration="$here/../release-targets.toml"

# One line per `[[target]]` identifier, and `!` for a document this cannot read.
#
# Deliberately narrow rather than a TOML implementation in awk: it takes `id`
# only inside a `[[target]]` table, requires the value to be a quoted
# `<registry>:<name>` and nothing else on the line, and refuses the document
# outright otherwise. Whether the document is *valid* is decided by `onevcs`'s
# canonical reader, in `tests/release_targets.rs`; this one only has to agree with
# it about the identifiers or refuse.
#
# llmlint: ignore-block[boundary_inputs_validated] this is the validation the rule
# asks for, at the level this consults: it tracks which table each line belongs to
# so a `[[retired]]` id is never read as a published one, requires each identifier
# to match the shape a registry serves, and refuses the document rather than
# answering from part of it. The library the rule would prefer does not exist in
# bash — the language this has to be in, because the host spawns it as a direct
# subprocess with no toolchain, no build, and no interpreter it did not find on
# `PATH` — and every shape it refuses is driven in
# `npm/test/release-targets.test.mjs`.
declared_targets='
{
  line = $0
  sub(/^[ \t]+/, "", line)
  sub(/[ \t]+$/, "", line)
  if (line == "" || substr(line, 1, 1) == "#") next
  if (substr(line, 1, 1) == "[") { sub(/[ \t]*#.*$/, "", line); table = line; next }
  if (table != "[[target]]") next
  if (line !~ /^id[ \t]*=/) next
  sub(/^id[ \t]*=[ \t]*/, "", line)
  if (line !~ /^"[^"]*"([ \t]*#.*)?$/) { bad = 1; exit }
  sub(/^"/, "", line)
  sub(/".*$/, "", line)
  if (line !~ /^[A-Za-z0-9][A-Za-z0-9_-]*:[A-Za-z0-9][A-Za-z0-9._@\/-]*$/) { bad = 1; exit }
  print line
  found = 1
}
END { if (bad || !found) print "!" }
'
# llmlint: ignore-end[boundary_inputs_validated]

# Where each registry is asked, fixed here and settable from nowhere else. The
# host hands this `PATH` and `HOME` and nothing else, so an environment variable
# is not something it could pass in production — and one read here anyway would
# be a way for a caller's environment to decide which registry an answer came
# from, on the one question a consumer holds a node against.
#
# What a test substitutes instead is the program that makes the request, on the
# search path the host already gives it: `npm/test/release-targets.test.mjs`
# hands the probe a `PATH` carrying a `curl` that points these three names at a
# fixture registry, and reads them out of the lines below so a moved endpoint
# reaches that fixture rather than the public registry.
CRATES_INDEX="https://index.crates.io"
PYPI_API="https://pypi.org/pypi"
NPM_REGISTRY="https://registry.npmjs.org"

# One request's budget. Three attempts of eight seconds, with one- and
# two-second backoffs, is twenty-seven seconds worst case — comfortably inside
# the sixty the contract allows, and long enough that a slow registry answers
# rather than being reported as one that did not.
attempts=3
timeout=8

# Not answered. Every exit here is non-zero, which is the whole of what a caller
# reads; the code separates the identifier from the registry, and both from this
# repository's own declaration, for a person.
refuse() {
  echo "release-probe: $1" >&2
  echo "ACTION: $2" >&2
  exit "$3"
}

# Every identifier this answers for, in the declaration's own publication order.
# Its reasoning — the qualification, and what is a `covers` entry rather than a
# target — is in that document, where a reader without this script can read it.
TARGETS=()

# A declaration this cannot read is **not answered**, and never an empty set of
# targets: answering "not an artifact this repository publishes" for every
# identifier would refuse a real target, and answering nothing would be worse.
if ! read_targets="$(awk "$declared_targets" "$declaration" 2>&1)"; then
  refuse "cannot read the release declaration at '$declaration': ${read_targets//$'\n'/ }" \
    "check that '$declaration' is checked in and that an 'awk' this host can run is on PATH; this is not answered, and says nothing about whether a release happened" 4
fi
if [ "$read_targets" = "!" ] || [ -z "$read_targets" ]; then
  refuse "'$declaration' names no [[target]] identifier this could read" \
    "check that every [[target]] in '$declaration' carries an id of the form <registry>:<name>; this is not answered, and says nothing about whether a release happened" 4
fi
while read -r target; do
  if [ -n "$target" ]; then
    TARGETS+=("$target")
  fi
done <<<"$read_targets"

usage="run 'release-probe.sh <registry>:<name>', where the identifier is one of: ${TARGETS[*]}"

if [ "$#" -ne 1 ]; then
  refuse "expected exactly one registry-qualified identifier, got $#" "$usage" 2
fi

identifier="$1"
recognised=false
for target in "${TARGETS[@]}"; do
  if [ "$target" = "$identifier" ]; then
    recognised=true
  fi
done
# An identifier this does not publish is **not answered**, never an empty answer:
# reporting "no release yet" for an artifact this repository has nothing to say
# about is the one mistake that would let a consumer launch on a release that
# never happened.
if [ "$recognised" != true ]; then
  refuse "'$identifier' is not an artifact this repository publishes, so this cannot say whether it is released" \
    "$usage; a target this repository has started publishing is declared in release-targets.toml at this repository's root" 2
fi

registry="${identifier%%:*}"
name="${identifier#*:}"

# A JSON reader for the two shapes the registries answer in, walked rather than
# searched. `"name"` and `"version"` are not fields that appear once in either
# document — a crates.io index record carries one `"name"` per entry of its
# `deps` array, and a PyPI or npm document nests them several levels deep — so a
# reader that took the first match, or counted the string across the line, would
# report a dependency's name where the artifact's belongs.
#
# `walk` reads the members of one object and skips a nested value whole. Each way
# a document can be unreadable gets its own tag, and the caller refuses by name:
# reading an unreadable answer as "no release" is the failure this whole file is
# written against.
#
# llmlint: ignore-block[boundary_inputs_validated] this is the parser the rule asks
# for, at the level this consults: it walks the document token by token, tracks
# nesting so a `deps` entry's fields are never read as the record's own, requires
# each member it reads to appear exactly once and to hold the type it should, and
# refuses the document otherwise. The library the rule would prefer does not exist
# in bash — the language this has to be in, because the host spawns it as a direct
# subprocess with no toolchain, no build, and no interpreter it did not find on
# `PATH` — and every shape it refuses is driven in
# `npm/test/release-targets.test.mjs`.
json_walker='
function skip_ws(s, i,   c) {
  while (i <= length(s)) {
    c = substr(s, i, 1)
    if (c != " " && c != "\t" && c != "\r" && c != "\n") break
    i++
  }
  return i
}
# Contents in STR when `keep`, 0 when the string never closes. What an escape
# encodes is never decoded: a version carrying one is not a version this can
# answer, and the caller refuses it by shape.
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
# A member is taken only under the key it was asked for, and a second copy of
# that key is DUP rather than a value quietly replaced: a document that says a
# version twice is one where the answer depends on which copy is read.
function take(key, type, value) {
  if (K1 != "" && key == K1) { if (T1 != "") DUP = 1; T1 = type; V1 = value }
  else if (K2 != "" && key == K2) { if (T2 != "") DUP = 1; T2 = type; V2 = value }
  else if (K3 != "" && key == K3) { if (T3 != "") DUP = 1; T3 = type; V3 = value }
}
function walk(s, k1, k2, k3,   i, n, c, key, vstart) {
  K1 = k1; K2 = k2; K3 = k3
  T1 = ""; T2 = ""; T3 = ""; V1 = ""; V2 = ""; V3 = ""
  BAD = 0; DUP = 0
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
    vstart = i
    if (c == "\"") {
      i = scan_string(s, i, (key == K1 || key == K2 || key == K3))
      if (i == 0) { BAD = 1; return }
      take(key, "string", STR)
    } else if (c == "{" || c == "[") {
      i = scan_nested(s, i); if (i == 0) { BAD = 1; return }
      take(key, (c == "{") ? "object" : "array", substr(s, vstart, i - vstart))
    } else {
      i = scan_literal(s, i); if (i == 0) { BAD = 1; return }
      take(key, "literal", LIT)
    }
    i = skip_ws(s, i)
    c = substr(s, i, 1)
    if (c == ",") { i = skip_ws(s, i + 1); continue }
    if (c == "}") { if (skip_ws(s, i + 1) <= n) BAD = 1; return }
    BAD = 1; return
  }
}
'

# One tagged line per crates.io index record: `release <version>` for a candidate
# a plain requirement could resolve to, `skip` for one it never could — a yanked
# release or a prerelease — and a refusal tag for a record this cannot read. The
# tag comes first and holds no spaces, so `read -r tag rest` splits it.
crates_records='
# A blank line is not a record: the index ends with a newline, and a reader that
# took the emptiness after it for an unreadable record would refuse every crate
# on the registry.
/^[[:space:]]*$/ { next }
{
  walk($0, "name", "vers", "yanked")
  if (BAD) { print "not-json"; next }
  if (DUP) { print "twice"; next }
  if (T1 != "string" || T2 != "string" || T3 != "literal") { print "unreadable"; next }
  if (V1 != want) { print "foreign"; next }
  if (V3 != "true" && V3 != "false") { print "unreadable"; next }
  if (V3 == "true") { print "skip"; next }
  core = V2
  sub(/\+.*$/, "", core)
  if (core ~ /-/) { print "skip"; next }
  print "release " core
}
'

# The string member one JSON document holds at `key1` or at `key1.key2`. The
# document is joined with spaces rather than read line by line, because a
# registry is free to serve it pretty-printed and JSON tokens stay
# self-delimiting across the join.
json_member='
{ doc = doc " " $0 }
END {
  walk(doc, key1, "", "")
  if (BAD) { print "bad"; exit }
  if (DUP) { print "twice"; exit }
  if (key2 == "") {
    if (T1 == "") { print "missing"; exit }
    if (T1 != "string") { print "bad"; exit }
    print "value " V1
    exit
  }
  if (T1 == "") { print "missing"; exit }
  if (T1 != "object") { print "bad"; exit }
  inner = V1
  walk(inner, key2, "", "")
  if (BAD) { print "bad"; exit }
  if (DUP) { print "twice"; exit }
  if (T1 == "") { print "missing"; exit }
  if (T1 != "string") { print "bad"; exit }
  print "value " V1
}
'
# llmlint: ignore-end[boundary_inputs_validated]

# What the last registry read answered. A shell variable rather than a file, so
# this needs no temporary directory to create, no trap to clear one up, and no
# `mktemp` or `rm` on the host — and no cleanup that could carry its own failure
# out as an exit status, which a consumer reads as *not answered*.
body=""

# Returns 0 with the document in `$body` for a 200, and 1 for a 404 — which is every
# registry's way of saying it serves no such artifact, and the one status that is
# an *answer* rather than a failure to get one. A refused, unreachable, or
# unexpected registry exits 3 from here: it is not answered, and there is no
# value to return that a caller could not mistake for one.
#
# A registry read fails for reasons that have nothing to do with whether a
# release happened, so a hiccup is retried; a status that will not change on a
# retry is not.
fetch() {
  local url="$1" what="$2" attempt code detail answered status
  detail=""
  attempt=1
  # A counted loop rather than `seq`: the host hands this a `PATH` and nothing
  # else, and every program this reaches for is one more thing that has to be on
  # it for a release to be observed at all.
  while [ "$attempt" -le "$attempts" ]; do
    code=""
    # The status code comes back on the end of the document, as its own last
    # line: a body of any shape can precede it, and the split below takes the
    # code off the end rather than looking for it inside. curl's own diagnostic
    # is joined to the same capture, so a failed attempt a later one recovers
    # from stays out of this run's stderr.
    if answered="$(curl -sS -w '\n%{http_code}' --max-time "$timeout" "$url" 2>&1)"; then
      code="${answered##*$'\n'}"
      body="${answered%$'\n'*}"
      case "$code" in
        200) return 0 ;;
        404) return 1 ;;
        408|429|5??) detail="answered HTTP $code" ;;
        *)
          refuse "$what: $url answered HTTP $code" \
            "check that '$url' is the registry endpoint this artifact is served from" 3
          ;;
      esac
    else
      # `$?` here is the request's own status, and 127 is the shell's "no such
      # program": a `curl` that never ran is not a registry that did not answer,
      # and sending its reader to check reachability would have them looking at
      # the network for a program that is missing.
      status=$?
      detail="${answered//$'\n'/ }"
      if [ "$status" -eq 127 ]; then
        refuse "$what: cannot run 'curl' to ask $url: $detail" \
          "check that a 'curl' this host can run is on PATH; this is not answered, and says nothing about whether a release happened" 3
      fi
    fi
    if [ "$attempt" -ne "$attempts" ]; then
      # A backoff that did not happen is a retry that never waited, so a host
      # that cannot pause is not answered rather than asked three times at once.
      if ! sleep "$attempt"; then
        refuse "$what: cannot wait $attempt second(s) before asking $url again" \
          "check that a 'sleep' this host can run is on PATH; this is not answered, and says nothing about whether a release happened" 3
      fi
    fi
    attempt=$((attempt + 1))
  done
  refuse "$what: $url did not answer in $attempts attempts: $detail" \
    "check reachability of '$url'; this is not answered, and says nothing about whether a release happened" 3
}

# What this will print as a released version: something a consumer can hold a
# node against and a person can read. Every version any of these registries
# serves for this repository comes from `Cargo.toml`, so this is deliberately
# tight — a registry answering anything else is a registry this could not read.
version_shaped() {
  # Numeric components separated by dots, then at most a prerelease or build
  # suffix: `0.16.3`, npm's `0.17.0-rc.1`, and PyPI's normalized `0.17.0rc1` are
  # answers; `1`, `1abc` and `1abc.2` are not, and neither is a page of prose a
  # registry served where a version belonged.
  local shape='^[0-9]+(\.[0-9]+){1,3}([-+.]?[0-9A-Za-z][0-9A-Za-z.+-]*)?$'
  [ "${#1}" -le 64 ] || return 1
  [[ "$1" =~ $shape ]]
}

at_least() {
  local -a left right
  local i
  IFS=. read -r -a left <<<"$1"
  IFS=. read -r -a right <<<"$2"
  for i in 0 1 2; do
    if [ "${left[i]}" -lt "${right[i]}" ]; then return 1; fi
    if [ "${left[i]}" -gt "${right[i]}" ]; then return 0; fi
  done
  return 0
}

# An `X.Y.Z` this can order. Anything else from a registry is refused rather than
# compared: three numeric components is what crates.io serves, and a string this
# cannot order reaching the comparison is the one way this could name a version
# that is not the newest.
orderable() {
  case "$1" in
    ""|*[!0-9.]*|*..*|.*|*.) return 1 ;;
  esac
  local IFS=. part count
  count=0
  for part in $1; do
    [ "${#part}" -le 9 ] || return 1
    count=$((count + 1))
  done
  [ "$count" -eq 3 ]
}

# Where the crates.io sparse index files one crate, including the short forms the
# registry uses for one-, two- and three-character names.
crates_path() {
  case "${#1}" in
    1) printf '1/%s\n' "$1" ;;
    2) printf '2/%s\n' "$1" ;;
    3) printf '3/%s/%s\n' "${1:0:1}" "$1" ;;
    *) printf '%s/%s/%s\n' "${1:0:2}" "${1:2:2}" "$1" ;;
  esac
}

# The newest release crates.io serves that a plain requirement could resolve to.
#
# The index files every release ever published, in no order this may rely on, so
# the answer is the greatest of the candidates rather than the last line. A yanked
# release and a prerelease are not candidates; an index with none of them left is
# a crate with nothing currently served, which is the empty answer.
ask_crates() {
  local tag rest best parsed
  if ! fetch "$CRATES_INDEX/$(crates_path "$name")" "the crates.io index"; then
    return 0
  fi
  if ! parsed="$(printf '%s\n' "$body" | awk -v want="$name" "$json_walker$crates_records")"; then
    refuse "the reader of the crates.io index for '$name' did not run" \
      "check that an 'awk' this host can run is on PATH; this is not answered, and says nothing about whether a release happened" 3
  fi
  best=""
  while read -r tag rest; do
    case "$tag" in
      release)
        if ! orderable "$rest"; then
          refuse "the crates.io index serves '$name' at version '$rest', which this cannot order against the others" \
            "check https://crates.io/crates/$name; this is not answered, and says nothing about whether a release happened" 3
        fi
        if [ -z "$best" ] || at_least "$rest" "$best"; then
          best="$rest"
        fi
        ;;
      skip|"") ;;
      foreign)
        refuse "the crates.io index served a record filed under another crate for '$name'" \
          "check that '$CRATES_INDEX' is serving the crates.io sparse index" 3
        ;;
      *)
        refuse "the crates.io index served a record for '$name' this could not read ($tag)" \
          "check that '$CRATES_INDEX' is serving the crates.io sparse index" 3
        ;;
    esac
  done <<<"$parsed"
  if [ -n "$best" ]; then
    printf '%s\n' "$best"
  fi
}

member_of() {
  printf '%s\n' "$body" | awk -v key1="$1" -v key2="$2" "$json_walker$json_member"
}

# What the reader made of one registry's answer, refusing when it did not run at
# all: an answer nothing read is not an answer, and an empty one here would be
# read as a release that has not happened.
read_member() {
  local answer
  if ! answer="$(member_of "$1" "$2")"; then
    refuse "the reader of $3's answer for '$name' did not run" \
      "check that an 'awk' this host can run is on PATH; this is not answered, and says nothing about whether a release happened" 3
  fi
  printf '%s' "$answer"
}

# What PyPI serves as the current release of a distribution: its own `info.version`,
# which is the version `pip install <name>` resolves to.
ask_pypi() {
  local answer tag value
  if ! fetch "$PYPI_API/$name/json" "the PyPI API"; then
    return 0
  fi
  answer="$(read_member info version PyPI)"
  tag="${answer%% *}"
  value="${answer#* }"
  if [ "$tag" != "value" ] || ! version_shaped "$value"; then
    refuse "PyPI answered for '$name' with no version this could read (${answer:-nothing at all})" \
      "check https://pypi.org/project/$name/; this is not answered, and says nothing about whether a release happened" 3
  fi
  printf '%s\n' "$value"
}

# What npm serves as the current release of a package: the manifest under its
# `latest` dist-tag, which is what `npm install <name>` resolves to. A package
# with no such tag is served as a 404, which is the empty answer.
ask_npm() {
  local answer tag value
  if ! fetch "$NPM_REGISTRY/$name/latest" "the npm registry"; then
    return 0
  fi
  answer="$(read_member version "" npm)"
  tag="${answer%% *}"
  value="${answer#* }"
  if [ "$tag" != "value" ] || ! version_shaped "$value"; then
    refuse "npm answered for '$name' with no version this could read (${answer:-nothing at all})" \
      "check https://www.npmjs.com/package/$name; this is not answered, and says nothing about whether a release happened" 3
  fi
  printf '%s\n' "$value"
}

case "$registry" in
  crate) ask_crates ;;
  pypi) ask_pypi ;;
  npm) ask_npm ;;
  *)
    # Unreachable through the declaration's own targets, and refused rather
    # than assumed: a target declared for a registry nothing here asks would
    # otherwise answer empty, which is the one answer this file may never give
    # by accident.
    refuse "'$identifier' names the registry '$registry', which this does not know how to ask" \
      "$usage" 2
    ;;
esac
