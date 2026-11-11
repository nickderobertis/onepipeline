#!/usr/bin/env bash
# Copied from dero-skills' create-repo skill; edit only the source.
# source: github.com/nickderobertis/dero-skills/skills/bootstrap/create-repo/assets/ci/coverage-unit.sh
# version: 3
# sha256: 9512ddce254386450d066aa55ed30a4e1af187bef307fad11442ee816d959968
#
# One coverage unit: one crate's tests, or one test tier's, run under cargo-llvm-cov and
# reduced to one LCOV report Nx can cache.
#
#   coverage-unit.sh <unit> -- <cargo llvm-cov arguments>   run the unit
#   coverage-unit.sh --cargo -- <cargo arguments>           cargo with the coverage build's
#                                                           flags and target directory: a
#                                                           `coverage-build` of a spawned
#                                                           binary, or a warm leg's compile
#   coverage-unit.sh --tool-version <tool>                  the version a key hashes, of the
#                                                           very tool the run uses: rustc,
#                                                           cargo-llvm-cov, cargo-nextest,
#                                                           llvm-profdata, llvm-cov or
#                                                           llvm-readobj
#
# Coverage is measured on Linux. On another platform the unit's cargo arguments run
# uninstrumented and write no report, as the test job there has always run them.
#
# <unit> is the Nx project name, or <project>-<tier> where a project owns several units:
# letters, digits, `-` and `_`, not starting with `-`.
#
# Reads: the instrumented binaries the run built under target/llvm-cov-target, and this
#   unit's own raw profiles there, target/llvm-cov-target/<unit>.*.profraw.
# Writes: target/coverage-units/<unit>.lcov, one LCOV report whose every SF: path is
#   relative to the workspace root, and target/llvm-cov-target/<unit>.profdata. Removes only
#   that report and this unit's raw profiles. No other unit's output is read or deleted.
# Environment: COVERAGE_UNIT_IGNORE_REGEX adds an alternative to the ignore rule, which is
#   otherwise the one `cargo llvm-cov report` uses (standard library, tests, examples and
#   benches, build output, the Cargo registry and the toolchain). It must be a valid POSIX
#   extended regular expression that cannot match every file. LLVM_PROFDATA and LLVM_COV
#   name those tools; otherwise PATH, then the toolchain's llvm-tools component.
# Exit: 0 when the report is written (or, off Linux, the tests passed); the cargo status when
#   a test fails; 1 when no profile was written, every profile was unmergeable, no
#   instrumented object was found, or `cargo llvm-cov show-env` printed anything but shell
#   assignments; 2 for a usage error or an unusable COVERAGE_UNIT_IGNORE_REGEX.
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd -P)"
cd "$root"

arguments="$*"
usage() {
  echo "coverage-unit.sh: cannot read the command line '$arguments'. ACTION: use one of these forms" >&2
  echo "usage: coverage-unit.sh <unit> -- <cargo llvm-cov arguments>" >&2
  echo "       coverage-unit.sh --cargo -- <cargo arguments>" >&2
  echo "       coverage-unit.sh --tool-version rustc|cargo-llvm-cov|cargo-nextest|llvm-profdata|llvm-cov|llvm-readobj" >&2
  exit 2
}

coverage_target="$root/target/llvm-cov-target"

# The llvm tool the run uses, so the key and the run cannot name different ones.
llvm_tool() {
  local name="$1" override="$2" sysroot host
  if [ -n "$override" ]; then
    printf '%s\n' "$override"
  elif command -v "$name" >/dev/null 2>&1; then
    command -v "$name"
  else
    sysroot="$(rustc --print sysroot)"
    host="$(rustc -vV | sed -n 's/^host: //p')"
    printf '%s\n' "$sysroot/lib/rustlib/$host/bin/$name"
  fi
}
profdata_tool() { llvm_tool llvm-profdata "${LLVM_PROFDATA:-}"; }
cov_tool() { llvm_tool llvm-cov "${LLVM_COV:-}"; }
readobj_tool() { llvm_tool llvm-readobj ""; }

on_linux() { [ "$(uname -s)" = Linux ]; }

# Applies `cargo llvm-cov show-env --sh`, refusing the whole of it unless every line is one
# `export NAME=value` whose value is a bare word or single-quoted, so nothing evaluated here
# can expand or run. The variables hold paths and flags, which that form always carries.
apply_show_env() {
  local exports line q="'"
  local assignment="^export [A-Za-z_][A-Za-z0-9_]*=(${q}[^${q}]*${q}(\\\\${q}${q}[^${q}]*${q})*|[A-Za-z0-9_./:=,@%+-]*)\$"
  exports="$(cargo llvm-cov show-env --sh)"
  while IFS= read -r line; do
    if ! [[ "$line" =~ $assignment ]]; then
      echo "coverage-unit.sh: 'cargo llvm-cov show-env --sh' printed something other than an assignment: $line" >&2
      echo "ACTION: check the cargo-llvm-cov on PATH against the pin in .github/ci-tool-versions" >&2
      exit 1
    fi
  done <<<"$exports"
  eval "$exports"
}

# `nextest ...` and `test ...` as an uninstrumented cargo command.
run_uninstrumented() {
  if [ "$1" = nextest ]; then
    shift
    cargo nextest run "$@"
  else
    cargo "$@"
  fi
}

[ "$#" -ge 1 ] || usage

case "$1" in
--tool-version)
  [ "$#" -eq 2 ] || usage
  # Off Linux no coverage tool runs, and the platform is its own key input.
  if ! on_linux; then
    case "$2" in
    rustc | cargo-llvm-cov | cargo-nextest | llvm-profdata | llvm-cov | llvm-readobj)
      echo "$2: unused off Linux"
      exit 0
      ;;
    *) usage ;;
    esac
  fi
  case "$2" in
  rustc) rustc -vV ;;
  cargo-llvm-cov) cargo llvm-cov --version ;;
  cargo-nextest) cargo nextest --version ;;
  llvm-profdata) "$(profdata_tool)" --version ;;
  llvm-cov) "$(cov_tool)" --version ;;
  llvm-readobj) "$(readobj_tool)" --version ;;
  *) usage ;;
  esac
  exit 0
  ;;
--cargo)
  [ "$#" -ge 3 ] && [ "$2" = "--" ] || usage
  shift 2
  if ! on_linux; then
    exec cargo "$@"
  fi
  apply_show_env
  unset LLVM_PROFILE_FILE
  CARGO_TARGET_DIR="$coverage_target" exec cargo "$@"
  ;;
-*) usage ;;
esac

unit="$1"
[ "$#" -ge 3 ] && [ "$2" = "--" ] || usage
shift 2
case "$unit" in
*[!A-Za-z0-9_-]* | "")
  echo "coverage-unit.sh: '$unit' is not a unit name. ACTION: name it with letters, digits, '-' and '_'" >&2
  exit 2
  ;;
esac

if ! on_linux; then
  run_uninstrumented "$@"
  exit
fi

# Checked before anything is cleared: an unusable fragment must cost neither the run nor the
# previous report, and one matching the empty string would ignore every file.
ignore_fragment="${COVERAGE_UNIT_IGNORE_REGEX:-}"
if [ -n "$ignore_fragment" ]; then
  set +e
  [[ "" =~ $ignore_fragment ]]
  fragment_status=$?
  set -e
  if [ "$fragment_status" -eq 2 ]; then
    echo "coverage-unit.sh: COVERAGE_UNIT_IGNORE_REGEX '$ignore_fragment' is not a valid regular expression. ACTION: correct it to a POSIX extended regular expression" >&2
    exit 2
  elif [ "$fragment_status" -eq 0 ]; then
    echo "coverage-unit.sh: COVERAGE_UNIT_IGNORE_REGEX '$ignore_fragment' matches every file, so it would ignore the whole report. ACTION: narrow it to the paths to leave out" >&2
    exit 2
  fi
fi

report_dir="$root/target/coverage-units"
report="$report_dir/$unit.lcov"
indexed="$coverage_target/$unit.profdata"
mkdir -p "$report_dir" "$coverage_target"
# A rejected or partial run leaves no report, so a stale one cannot look like this run's.
rm -f "$report" "$indexed"
find "$coverage_target" -maxdepth 1 -type f -name "$unit.*.profraw" -delete

status=0
LLVM_PROFILE_FILE_NAME="$unit.%p-%14m.profraw" cargo llvm-cov --no-report "$@" || status=$?
[ "$status" -eq 0 ] || exit "$status"

shopt -s nullglob
profiles=("$coverage_target/$unit".*.profraw)
shopt -u nullglob
if [ "${#profiles[@]}" -eq 0 ]; then
  echo "coverage-unit.sh: $unit wrote no profile under $coverage_target. ACTION: check that its arguments select tests" >&2
  exit 1
fi

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
printf '%s\n' "${profiles[@]}" >"$work/profiles"
if ! "$(profdata_tool)" merge -sparse -failure-mode=all -f "$work/profiles" -o "$indexed" 2>"$work/merge.err"; then
  cat "$work/merge.err" >&2
  echo "coverage-unit.sh: every profile of $unit was unmergeable. ACTION: check for a run killed before it wrote one" >&2
  exit 1
fi

# The objects that wrote into this profile, by build id, so a binary another unit left in
# the shared target directory never lends this report its lines.
if ! "$(profdata_tool)" show --binary-ids "$indexed" >"$work/show" 2>"$work/show.err"; then
  cat "$work/show.err" >&2
  echo "coverage-unit.sh: llvm-profdata could not list the binaries of $unit. ACTION: check the profile at $indexed" >&2
  exit 1
fi
sed -n '/^Binary IDs:/,$p' "$work/show" | sed 1d | tr -d ' ' | tr '[:upper:]' '[:lower:]' | sort -u >"$work/ids"
readobj="$(readobj_tool)"
# Other units compile into this directory at the same time, so a file vanishing mid-scan is
# routine; any other find error loses objects silently and must stop the report.
if ! find "$coverage_target" -type f -perm -u+x \
  -not -path '*/build/*' -not -path '*/incremental/*' -not -path '*/.fingerprint/*' -print0 \
  >"$work/objects" 2>"$work/find.err"; then
  if grep -qvF 'No such file or directory' "$work/find.err"; then
    cat "$work/find.err" >&2
    echo "coverage-unit.sh: could not list $coverage_target for $unit. ACTION: check the target directory's permissions" >&2
    exit 1
  fi
fi
objects=()
while IFS= read -r -d '' candidate; do
  [ "$(head -c4 "$candidate" | od -An -c | tr -d ' ')" = '177ELF' ] || continue
  notes="$("$readobj" --notes "$candidate")" || continue
  id="$(printf '%s\n' "$notes" | sed -n 's/.*Build ID: //p' | head -1 | tr '[:upper:]' '[:lower:]')"
  [ -n "$id" ] && grep -qxF "$id" "$work/ids" && objects+=(-object "$candidate")
done <"$work/objects"
if [ "${#objects[@]}" -eq 0 ]; then
  echo "coverage-unit.sh: no object under $coverage_target ran for $unit. ACTION: check that the linker writes a build id and the unit's binaries were built" >&2
  exit 1
fi

# The rule `cargo llvm-cov report` applies, anchored on this checkout.
# shellcheck disable=SC2016 # `$` here is a literal inside sed's bracket expression.
escape() { printf '%s' "$1" | sed 's/[][\\.*^$()|{}?+/-]/\\&/g'; }
root_re="$(escape "$root")"
cargo_home="${CARGO_HOME:-$HOME/.cargo}"
rustup_home="${RUSTUP_HOME:-$HOME/.rustup}"
ignore="/rustc/([0-9a-f]+|[0-9]+\.[0-9]+\.[0-9]+)/"
ignore+="|^$root_re(/.*)?/(tests|examples|benches)/"
ignore+="|^$root_re(/.*)?/(tests\.rs|[0-9a-zA-Z_-]+[_-]tests\.rs)\$"
ignore+="|^$root_re/target/llvm-cov-target(\$|/)"
ignore+="|^$(escape "$cargo_home")/(registry|git)/"
ignore+="|^$(escape "$rustup_home")/toolchains(\$|/)"
[ -z "$ignore_fragment" ] || ignore+="|$ignore_fragment"

if ! "$(cov_tool)" export -format=lcov -instr-profile="$indexed" "${objects[@]}" \
  -ignore-filename-regex="$ignore" >"$work/report.lcov" 2>"$work/export.err"; then
  cat "$work/export.err" >&2
  echo "coverage-unit.sh: llvm-cov could not export $unit. ACTION: check that its objects match the profile" >&2
  exit 1
fi

# Relative to the workspace root, so a report replayed on another checkout names the same file.
awk -v prefix="SF:$root/" 'index($0, prefix) == 1 { print "SF:" substr($0, length(prefix) + 1); next } { print }' \
  "$work/report.lcov" >"$work/relative.lcov"
if outside="$(grep -m1 '^SF:/' "$work/relative.lcov")"; then
  echo "coverage-unit.sh: $unit reports a file outside the workspace: ${outside#SF:}. ACTION: add it to COVERAGE_UNIT_IGNORE_REGEX or move it into the workspace" >&2
  exit 1
fi
mv "$work/relative.lcov" "$report"
