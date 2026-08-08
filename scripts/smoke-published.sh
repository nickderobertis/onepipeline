#!/usr/bin/env bash
# Smoke-test an `onepipeline` that is already on PATH, and name the install
# that broke when it does not behave.
#
# One script, one set of assertions. `release.yml`'s verify jobs and
# `published-smoke.yml` run this over a binary they installed from PyPI or npm;
# CI's `install` job runs the identical file over the binary this repo just
# compiled. That is what stops a workflow's idea of "it works" from drifting from
# what actually ships — assertions inlined in a workflow keep passing after the
# surface around them changes.
#
# Deliberately toolchain-free: bash and the installed binary. The scheduled sweep
# runs this every week on every OS, for both registries, and anything it had to
# install first would be a second thing that can rot.
#
# While the crate is interface-only, the promise a published artifact makes is
# its *surface*: it reports its version, prints the documented command list, and
# refuses to pretend it ran a graph. Extend the assertions here as behavior
# lands — this file is what a release proves.
set -euo pipefail

expect_version=""
label="installed onepipeline"

fail() {
  echo "::error::$label: $1" >&2
  echo "ACTION: $2" >&2
  exit 1
}

# Every option takes a value, so a missing one is an argument error rather than
# a silently empty setting.
need_value() {
  if [ "$#" -lt 2 ]; then
    echo "$1 needs a value" >&2
    echo "ACTION: pass it as '$1 <value>'" >&2
    exit 2
  fi
}

while [ "$#" -gt 0 ]; do
  case "$1" in
    --expect-version) need_value "$@"; expect_version="$2"; shift 2 ;;
    # What installed the binary, so a red matrix leg names the platform and the
    # registry rather than only the assertion that failed.
    --label) need_value "$@"; label="$2"; shift 2 ;;
    *)
      echo "unknown option $1" >&2
      echo "ACTION: run 'smoke-published.sh [--expect-version V] [--label TEXT]'" >&2
      exit 2
      ;;
  esac
done

command -v onepipeline >/dev/null 2>&1 || fail "no 'onepipeline' on PATH" \
  "install it first — 'pip install onepipeline-cli' or 'npm install -g onepipeline-cli'"

# Windows ships the same bytes with CRLF once anything touches them, so strip CR
# rather than let a line ending decide the verdict.
reported="$(onepipeline --version 2>&1 | tr -d '\r')" || fail \
  "'--version' did not run: $reported" \
  "the installed binary cannot start — check the platform package the install selected"
if [ -n "$expect_version" ] && [ "$reported" != "onepipeline $expect_version" ]; then
  fail "reports '$reported', not 'onepipeline $expect_version'" \
    "the install resolved a different version than the one just published"
fi

help="$(onepipeline --help 2>&1 | tr -d '\r')" || fail \
  "'--help' did not run: $help" \
  "the installed binary cannot print its own surface — reinstall it and re-run"
for command in start adopt round channel next reply surface attest stop runs status host monitor results goals telemetry; do
  case "$help" in
    *"$command"*) ;;
    *) fail "'--help' does not list the '$command' command" \
         "the installed binary does not carry the documented command surface" ;;
  esac
done

# The refusal is part of the shipped contract while the crate is interface-only:
# a build that silently succeeded here would report an unimplemented run as one
# that settled. It must also refuse with a code the contract has not already
# spent — 0 applied, 1 queued, 2 refused, 3 nothing is driving the run — or a
# caller reads the refusal as one of those answers.
code=0
onepipeline next smoke-run >/dev/null 2>&1 || code=$?
case "$code" in
  70) ;;
  0) fail "'next' exited 0 without reading anything" \
       "an interface-only build must refuse; a caller reads exit 0 as a surface it consumed" ;;
  1|2|3) fail "'next' exited $code, a code the contract already assigns" \
       "the interface-only refusal must not be readable as applied, queued, refused, or undriven" ;;
  *) fail "'next' exited $code, which is neither the interface-only refusal (70) nor a code the contract assigns" \
       "run 'onepipeline next smoke-run' by hand to see what the installed binary is doing" ;;
esac

echo "$label: surface smoke test passed${expect_version:+ for $expect_version}"
