#!/usr/bin/env bash
# After a test step has failed: say why any of its tests went unrun, then re-run
# each test it failed a fixed number of times against the build that already
# exists, and report how many of those re-runs failed.
#
#   rerun-failed.sh --log FILE --times N --known-flakes FILE -- RUNNER...
#
# FILE is the failed step's output. RUNNER is the nextest command that step ran
# its suite with, minus its filter (`cargo nextest run --locked`). Each re-run is
# that command narrowed to the failed tests, so cargo finds the step's build
# fresh and compiles nothing. A re-run that did compile says so.
#
# This only reports. It never re-runs a test until it passes and never excuses
# a failure. The report (each re-run's result and the summary) goes to stdout,
# and what is wrong goes to stderr. Exit status:
#   0  the log names no test failure (the step failed somewhere else)
#   1  the log names a failed or unrun test, whatever the re-runs show, because
#      the run it reports on failed
#   2  a usage error, an unreadable input, or a malformed known-flake list
set -uo pipefail

usage() {
  echo "usage: rerun-failed.sh --log FILE --times N --known-flakes FILE -- RUNNER..." >&2
  exit 2
}
refuse() {
  echo "rerun-failed: $*" >&2
  exit 2
}

log="" times="" flakes=""
while [ "$#" -gt 0 ]; do
  case "$1" in
    --log) [ "$#" -ge 2 ] || usage; log="$2"; shift 2 ;;
    --times) [ "$#" -ge 2 ] || usage; times="$2"; shift 2 ;;
    --known-flakes) [ "$#" -ge 2 ] || usage; flakes="$2"; shift 2 ;;
    --) shift; break ;;
    *) echo "rerun-failed: unknown argument '$1'" >&2; usage ;;
  esac
done
[ -n "$log" ] && [ -n "$times" ] && [ -n "$flakes" ] && [ "$#" -gt 0 ] || usage
case "$times" in
  [1-9] | 10) ;;
  *) refuse "--times must be a whole number from 1 to 10, not '$times'" ;;
esac
[ -e "$log" ] || refuse "cannot read the failed step's log at '$log'; capture the step's output there (ci.yml tees it)"
[ -f "$log" ] && [ -r "$log" ] \
  || refuse "cannot read '$log' as text; pass the file the failed step's output was teed into"
[ -f "$flakes" ] && [ -r "$flakes" ] \
  || refuse "cannot read the known-flake list at '$flakes' as a file; restore it from git (scripts/known-flakes.txt) or pass the right path"
runner=("$@")

# Binary ids and test names come out of a log and go into a nextest filter, so
# only the characters Rust paths and nextest binary ids are made of get through.
id_shape='^[A-Za-z0-9_:./-]+$'
issue_shape='^https://github\.com/[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+/issues/[0-9]+$'

flake_ids=() flake_urls=()
lineno=0
while IFS= read -r line || [ -n "$line" ]; do
  lineno=$((lineno + 1))
  line="${line%%#*}"
  line="${line//$'\r'/}"
  read -r -a fields <<<"$line" || true
  [ "${#fields[@]}" -eq 0 ] && continue
  if [ "${#fields[@]}" -ne 3 ] || ! [[ ${fields[0]} =~ $id_shape ]] \
    || ! [[ ${fields[1]} =~ $id_shape ]] || ! [[ ${fields[2]} =~ $issue_shape ]]; then
    refuse "$flakes:$lineno is not '<binary-id> <test-name> <issue-url>' with a GitHub issue URL; fix or remove it: $line"
  fi
  flake_ids+=("${fields[0]} ${fields[1]}")
  flake_urls+=("${fields[2]}")
done <"$flakes" || refuse "reading the known-flake list '$flakes' failed (the error is above); restore it from git (scripts/known-flakes.txt)"

work="$(mktemp -d)" && [ -d "$work" ] \
  || refuse "cannot make a scratch directory with mktemp -d (its error is above); point TMPDIR at a writable directory"
trap 'rm -rf "$work"' EXIT

esc="$(printf '\033')"
plain() { sed "s/${esc}\[[0-9;]*[A-Za-z]//g" "$1" | tr -d '\r'; }
plain "$log" >"$work/step.log" \
  || refuse "reading '$log' failed (the error is above); pass the file the failed step's output was teed into"

# A status line: `   FAIL [   0.012s] (  3/120) <binary-id> <test-name>`. The
# failing spellings are the ones this repository's configuration can produce,
# and tests/nextest_runs.rs drives real nextest to each of them: a failure, a
# retried failure, a timeout, and an abort (a signal name on Unix, ABORT on
# Windows). The passing ones too: a pass, a leaky pass, and a retried pass
# (`TRY 2 PASS`; the re-runs print no final `FLAKY` line). Leaks pass here (no
# `leak-timeout` result is set), so LEAK-FAIL cannot occur.
status_line() {
  echo "^[[:space:]]*(TRY [0-9]+ )?($1) \[[[:space:]]*[0-9.]+s\] (\([[:space:]]*[0-9]+/[[:space:]]*[0-9]+\) )?([^[:space:]]+) ([^[:space:]]+)[[:space:]]*\$"
}
failed_re="$(status_line 'FAIL|TIMEOUT|ABORT|SIG[A-Z0-9]+')"
passed_re="$(status_line 'PASS|LEAK')"
tests_matching() { sed -nE "s#$1#\\4 \\5#p" "$2" | sort -u; }

say() { echo "rerun-failed: $*"; }
warn() { echo "rerun-failed: $*" >&2; }

# nextest names a reason ("due to test failure") whenever it cancels. A run it
# ends with tests unrun and no reason is the case this exists to make visible,
# so that one names nextest's own exit status.
unrun=0
status="$(sed -nE 's/^nextest-run: nextest exited with status ([0-9]+)$/\1/p' "$work/step.log" | tail -n 1)"
while IFS= read -r warning; do
  unrun=1
  counts="$(sed -nE 's/.*warning: ([0-9]+\/[0-9]+) tests? (were|was) not run.*/\1/p' <<<"$warning")"
  case "$warning" in
    *" due to "*) warn "nextest left $counts tests not run ${warning#* not run }" ;;
    *)
      if [ -n "$status" ]; then
        warn "nextest exited with status $status and left $counts tests not run, and gave no reason: no test failed, timed out or was cancelled. See the 'priority' note in .config/nextest.toml for the one cause known here; a run that strands tests for another reason needs that cause found and fixed."
      else
        warn "nextest left $counts tests not run and gave no reason, and its exit status is not in this log (run the suite through scripts/nextest-run.sh to record it). See the 'priority' note in .config/nextest.toml for the one cause known here."
      fi
      ;;
  esac
done < <(grep -E 'warning: [0-9]+/[0-9]+ tests? (were|was) not run' "$work/step.log")

failed=()
while IFS= read -r test; do
  [ -n "$test" ] || continue
  read -r binary name <<<"$test"
  if ! [[ $binary =~ $id_shape ]] || ! [[ $name =~ $id_shape ]]; then
    warn "not re-running '$test': it has characters a nextest filter is not built from here. If nextest now names tests that way, widen id_shape in scripts/rerun-failed.sh."
    continue
  fi
  failed+=("$test")
done < <(tests_matching "$failed_re" "$work/step.log")

if [ "${#failed[@]}" -eq 0 ]; then
  if [ "$unrun" -eq 1 ] || grep -qE '^error: test run failed' "$work/step.log"; then
    warn "the test run failed and names no failed test, so there is nothing to re-run. The job's verdict stays failed: what ended the run is in the step's output above, and a line above names it when it is tests left unrun."
    exit 1
  fi
  say "$log names no failed test, so there is nothing to re-run (the step failed outside the test run; its own output above says where)."
  exit 0
fi

say "the test run failed. Re-running each of its ${#failed[@]} failed test(s) $times time(s) against this build to tell a new flake from a regression."
say "This step only reports: the job's verdict stays failed whatever the re-runs show."

filter=""
fails=()
unran=()
for test in "${failed[@]}"; do
  read -r binary name <<<"$test"
  filter="${filter:+$filter or }(binary_id(=$binary) and test(=$name))"
  fails+=(0)
  unran+=(0)
done

grouped() { [ "${GITHUB_ACTIONS:-}" = "true" ]; }
for run in $(seq 1 "$times"); do
  out="$work/rerun-$run.log"
  grouped && echo "::group::re-run $run of $times: nextest output"
  CARGO_TERM_QUIET=false "${runner[@]}" --no-fail-fast --status-level pass \
    --final-status-level none -E "$filter" >"$out.stdout" 2>"$out.stderr"
  exited=$?
  cat "$out.stdout"
  cat "$out.stderr" >&2
  grouped && echo "::endgroup::"
  cat "$out.stdout" "$out.stderr" >"$out"
  plain "$out" >"$out.plain"
  rebuilt="$(sed -nE 's/^[[:space:]]*Compiling ([^ ]+).*/\1/p' "$out.plain" | tr '\n' ' ')"
  if [ -n "$rebuilt" ]; then
    warn "re-run $run of $times compiled ${rebuilt}first, so it ran a new build of those rather than the failed step's. Re-run with the command the step built with (just rerun-failed after test-quick, just rerun-failed-coverage after the instrumented gate)."
  fi
  tests_matching "$failed_re" "$out.plain" >"$out.failed"
  tests_matching "$passed_re" "$out.plain" >"$out.passed"
  i=0
  for test in "${failed[@]}"; do
    if grep -qxF "$test" "$out.failed"; then
      fails[i]=$((fails[i] + 1))
      say "re-run $run of $times: $test FAILED"
    elif grep -qxF "$test" "$out.passed"; then
      say "re-run $run of $times: $test passed"
    else
      unran[i]=$((unran[i] + 1))
      say "re-run $run of $times: $test did not run (the re-run exited with status $exited; its output above says why, and ${runner[*]} -E '$filter' repeats it)"
    fi
    i=$((i + 1))
  done
done

say "summary: how often each failed test failed again on this build"
i=0
for test in "${failed[@]}"; do
  read -r binary name <<<"$test"
  count="${fails[i]}"
  ran=$((times - unran[i]))
  if [ "$ran" -eq 0 ]; then
    reading="did not run in any re-run, so this says nothing about it: fix what the re-runs' output above names, then run ${runner[*]} -E 'binary_id(=$binary) and test(=$name)'"
  elif [ "$count" -eq "$ran" ]; then
    reading="fails every time on this build, which reads as a regression: reproduce it with cargo nextest run -E 'binary_id(=$binary) and test(=$name)' and fix it"
  elif [ "$count" -eq 0 ]; then
    reading="did not fail again, which reads as a flake: fix it, or open an issue and list it in scripts/known-flakes.txt"
  else
    reading="fails some of the time on this build, which reads as a flake: fix it, or open an issue and list it in scripts/known-flakes.txt"
  fi
  line="  $test: failed $count of $times re-runs"
  [ "${unran[i]}" -gt 0 ] && line="$line (${unran[i]} did not run)"
  line="$line; $reading"
  j=0
  for known in ${flake_ids[@]+"${flake_ids[@]}"}; do
    if [ "$known" = "$test" ]; then
      line="$line (known flake, issue ${flake_urls[j]})"
    fi
    j=$((j + 1))
  done
  echo "$line"
  i=$((i + 1))
done
warn "the test step failed, and that verdict stands."
exit 1
