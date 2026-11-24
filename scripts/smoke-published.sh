#!/usr/bin/env bash
# Smoke-test an `onepipeline` that is already on PATH, and name the install
# that broke when it does not behave.
#
# One script, one set of assertions. `release.yml`'s build jobs run this over
# each file they built and test-installed, and again over what they installed back
# from the Release, PyPI and npm; CI's `install` job runs the identical file over
# the binary this repo just compiled. That is what stops a workflow's idea of "it works" from drifting from
# what actually ships — assertions inlined in a workflow keep passing after the
# surface around them changes.
#
# Deliberately toolchain-free: bash and the installed binary. Every release runs
# this on every OS, for every registry, and anything it had to install first would
# be a second thing that can rot.
#
# The promise a published artifact makes: it reports its version, prints the
# documented command list, and answers a real command against a real ledger with
# the code the contract assigns. Extend the assertions here as behavior lands —
# this file is what a release proves.
set -euo pipefail

expect_version=""
label="installed onepipeline"
build="release"

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
    # Which build is on PATH. Everything published is `release`; `debug` is only for the
    # packaging test that stages a dev-profile wheel to save the gate a release compile,
    # and it turns the time scale check below into its debug-build counterpart.
    --build)
      need_value "$@"
      case "$2" in
        release | debug) build="$2" ;;
        *)
          echo "--build must be 'release' or 'debug', not '$2'" >&2
          echo "ACTION: pass '--build release' (the default) or '--build debug'" >&2
          exit 2
          ;;
      esac
      shift 2
      ;;
    *)
      echo "unknown option $1" >&2
      echo "ACTION: run 'smoke-published.sh [--expect-version V] [--label TEXT] [--build release|debug]'" >&2
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
for command in human-items scratch start plan adopt label channel next reply surface decline-ruling attest stop shutdown runs status host monitor watch unwatched unpublished stop-guard ask flow publish-branch repo-recover integrate results review readjust supersessions goals transcript agents telemetry template boundary; do
  case "$help" in
    *"$command"*) ;;
    *) fail "'--help' does not list the '$command' command" \
         "reinstall the version under test — 'pip install --force-reinstall onepipeline-cli${expect_version:+==$expect_version}' or 'npm install -g onepipeline-cli${expect_version:+@$expect_version}' — or, if the contract no longer names '$command', drop it from the 'for command in' list above" ;;
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
# end-to-end test — CI's `install` job and every release build job's verify step run this exact
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

# A published binary is a release build, and a release build never reads the debug build's
# time scale (`src/timescale.rs`): whatever `ONEPIPELINE_TEST_TIME_SCALE` says — a factor,
# or a value no factor is — it is not refused, and the waits the binary applies are the
# shipped ones. The cheapest wait it states is the write-back's default per-item budget,
# which `adopt` names when it refuses a budget of zero, before it reads or writes anything;
# the empty run directory is there only so that refusal is the one reached.
# `tests/published_smoke.rs` holds the figure below to the crate's
# `DEFAULT_WRITEBACK_ITEM_BUDGET_SECONDS`, and drives this check over the debug build, which
# it refuses — unless `--build debug` names that build, as the staged dev-profile wheel of
# `npm/test/packaging/launcher.test.mjs` does, when the check holds it to the debug build's behaviour.
shipped_item_budget=12
scale_root="$(mktemp -d)"
mkdir "$scale_root/nothing-recorded"
for scale in 0.25 not-a-number; do
  code=0
  said="$(ONEPIPELINE_RUNS_DIR="$scale_root" ONEPIPELINE_TEST_TIME_SCALE="$scale" \
    onepipeline adopt nothing-recorded --writeback-item-budget 0 2>&1 >/dev/null | tr -d '\r')" || code=$?
  if [ "$build" = release ]; then
    case "$said" in
      *"take $shipped_item_budget seconds per item"*) continue ;;
    esac
    rm -rf "$scale_root"
    fail "under ONEPIPELINE_TEST_TIME_SCALE=$scale, 'adopt' did not name the shipped $shipped_item_budget-second write-back budget (exit $code): ${said:-it printed nothing}" \
      "a release build applies the shipped waits whatever that variable says, so this is a debug build or one that reads its test settings — publish only the release build"
  fi
  # `--build debug`: the debug build reads the scale, so it applies a quarter of the shipped
  # budget, and refuses a value that is no factor by the variable's name.
  case "$scale:$code:$said" in
    "0.25:2:"*"take $((shipped_item_budget / 4)) seconds per item"*) ;;
    "not-a-number:2:"*"ONEPIPELINE_TEST_TIME_SCALE must be a positive, finite number"*) ;;
    *)
      rm -rf "$scale_root"
      fail "under ONEPIPELINE_TEST_TIME_SCALE=$scale, the debug build did not apply or refuse the scale (exit $code): ${said:-it printed nothing}" \
        "a debug build reads that variable, so this is a release build or one that ignores its test settings — pass '--build release' for it" ;;
  esac
done
# Nor does a release build read the debug build's journey clock (`ONEPIPELINE_TEST_CLOCK`,
# beside the scale in `src/timescale.rs`) or its shadow source's create lease
# (`ONEPIPELINE_TEST_SHADOW_CREATE_SECONDS`): handed a clock that names nothing and a lease that
# is no number, it is not refused and still applies the shipped wait, where the debug build
# refuses them by name before the verb runs — so no published binary can be put on a clock a
# test moves.
code=0
said="$(ONEPIPELINE_RUNS_DIR="$scale_root" ONEPIPELINE_TEST_CLOCK="$scale_root/no-clock-here" \
  ONEPIPELINE_TEST_SHADOW_CREATE_SECONDS=not-a-number \
  onepipeline adopt nothing-recorded --writeback-item-budget 0 2>&1 >/dev/null | tr -d '\r')" || code=$?
case "$build:$code:$said" in
  "release:"*"take $shipped_item_budget seconds per item"*) ;;
  "debug:2:"*"ONEPIPELINE_TEST_CLOCK must name a directory"*) ;;
  release:*)
    rm -rf "$scale_root"
    fail "under ONEPIPELINE_TEST_CLOCK naming no clock, 'adopt' did not name the shipped $shipped_item_budget-second write-back budget (exit $code): ${said:-it printed nothing}" \
      "a release build reads no journey clock, so this is a debug build or one that reads its test settings — publish only the release build" ;;
  *)
    rm -rf "$scale_root"
    fail "under ONEPIPELINE_TEST_CLOCK naming no clock, the debug build did not refuse it (exit $code): ${said:-it printed nothing}" \
      "a debug build reads that variable, so this is a release build or one that ignores its test settings — pass '--build release' for it" ;;
esac
rm -rf "$scale_root"

echo "$label: surface smoke test passed${expect_version:+ for $expect_version}"
