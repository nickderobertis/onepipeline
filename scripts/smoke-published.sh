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
# The promise a published artifact makes: it reports its version, prints the
# documented command list, and answers a real command against a real ledger with
# the code the contract assigns. Extend the assertions here as behavior lands —
# this file is what a release proves.
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
    "the install resolved a different version — wait for the registry to serve $expect_version, then reinstall pinned: 'pip install onepipeline-cli==$expect_version' or 'npm install -g onepipeline-cli@$expect_version'"
fi

help="$(onepipeline --help 2>&1 | tr -d '\r')" || fail \
  "'--help' did not run: $help" \
  "the installed binary cannot print its own surface — reinstall it and re-run"
# The list below is not a second source: `tests/contract.rs`'s
# `the_smoke_scripts_command_list_is_the_binarys_whole_surface` parses this very
# line and asserts it equals the binary's subcommands, so a command added or
# renamed fails the gate here rather than leaving a published artifact unchecked.
for command in start adopt channel next reply surface attest stop runs status host monitor results goals transcript telemetry; do
  case "$help" in
    *"$command"*) ;;
    *) fail "'--help' does not list the '$command' command" \
         "the installed binary predates that command — reinstall the version under test, or drop '$command' from this list if the contract no longer names it" ;;
  esac
done

# The two hidden verbs, deliberately absent from the list above: neither is on
# `--help`, because neither is a surface a user types. `start --detach` spawns
# *this binary* at `drive-run`, which is the process that drives the whole run,
# and at `drive` when that run was given an observer graph — so a published
# artifact without either cannot launch a detached run at all, and `--help`
# gives nothing to notice that in.
#
# What the binary actually said is carried into the failure rather than
# discarded: "no such subcommand" is the artifact predating the verb, and
# anything else is a build that has it and could not run it — two different
# faults, and a probe that reported them identically would send the reader after
# the wrong one.
hidden_action() {
  case "$2" in
    *"unrecognized subcommand"* | *"unknown subcommand"* | *"not found"*)
      echo "the installed binary predates the verb — reinstall the version under test" ;;
    *)
      echo "the verb is present and would not run — run 'onepipeline $1 --help' by hand against the $label and fix what it reports; do not publish, because every detached launch spawns it" ;;
  esac
}

if ! said="$(onepipeline drive-run --help 2>&1)"; then
  fail "the 'drive-run' command a detached launch retains did not run: ${said}" \
    "$(hidden_action drive-run "${said}")"
fi
if ! said="$(onepipeline drive --help 2>&1)"; then
  fail "the 'drive' command an observed detached launch retains did not run: ${said}" \
    "$(hidden_action drive "${said}")"
fi

# Reading a run nobody recorded is the smallest command that reaches the ledger,
# and its answer is fixed: exit 2, naming the run. A build that exited 0 here
# would be reporting a surface it never read, and one that exited 3 would be
# sending a planner to intervene in a run that does not exist.
# llmlint: ignore-block[changed_behavior_has_e2e] this script *is* the release's own
# end-to-end test — CI's `install` job and both post-publish verify jobs run this exact
# file against a real installed binary on every platform, which exercises the passing
# branch below continuously. Its failure branches are diagnostics for that run, and a
# harness that installed a deliberately broken binary to drive each one would be a second
# thing to keep in sync with the first. Revisit if this script grows logic of its own.
code=0
# Keep stderr: the refusal itself is what the passing branch expects to be there,
# and on any other exit it is the only account of what went wrong.
refusal="$(onepipeline next smoke-run 2>&1 >/dev/null)" || code=$?
case "$code" in
  2)
    case "$refusal" in
      *smoke-run*) ;;
      *) fail "'next' refused without naming the run it could not find: ${refusal:-it printed nothing}" \
           "the refusal has to name the run and the root it searched, or a caller cannot tell a typo from an outage" ;;
    esac
    ;;
  0) fail "'next' exited 0 without reading anything" \
       "a caller reads exit 0 as a surface it consumed; a run nobody recorded has none" ;;
  3) fail "'next' exited 3 for a run that does not exist" \
       "exit 3 sends a planner to intervene in a live run; an unknown run is a refusal (2)" ;;
  *) fail "'next' exited $code, which is not the refusal the contract assigns: ${refusal:-it printed nothing}" \
       "fix what that error names, or reinstall the binary if it cannot start at all" ;;
esac
# llmlint: ignore-end[changed_behavior_has_e2e]

echo "$label: surface smoke test passed${expect_version:+ for $expect_version}"
