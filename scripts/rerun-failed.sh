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
# a failure. It exits 1 whenever the log names a failed or unrun test, whatever
# the re-runs show, because the run it reports on failed. It exits 0 only when
# the log names no test failure (the step failed somewhere else), and 2 for a
# usage error or a malformed known-flake list.
set -uo pipefail

usage() {
  echo "usage: rerun-failed.sh --log FILE --times N --known-flakes FILE -- RUNNER..." >&2
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
  *) echo "rerun-failed: --times must be a whole number from 1 to 10, not '$times'" >&2; exit 2 ;;
esac
[ -f "$log" ] || { echo "rerun-failed: no log at '$log'; the failed step's output was not captured" >&2; exit 2; }
[ -f "$flakes" ] || { echo "rerun-failed: no known-flake list at '$flakes'" >&2; exit 2; }
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
    echo "rerun-failed: $flakes:$lineno is not '<binary-id> <test-name> <issue-url>' with a GitHub issue URL: $line" >&2
    exit 2
  fi
  flake_ids+=("${fields[0]} ${fields[1]}")
  flake_urls+=("${fields[2]}")
done <"$flakes"

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

# The log as plain text: no colour escapes, no carriage returns.
esc="$(printf '\033')"
plain() { sed "s/${esc}\[[0-9;]*[A-Za-z]//g" "$1" | tr -d '\r'; }
plain "$log" >"$work/step.log"

# A status line: `   FAIL [   0.012s] (  3/120) <binary-id> <test-name>`.
status_line() {
  echo "^[[:space:]]*(TRY [0-9]+ )?($1) \[[[:space:]]*[0-9.]+s\] (\([[:space:]]*[0-9]+/[[:space:]]*[0-9]+\) )?([^[:space:]]+) ([^[:space:]]+)[[:space:]]*\$"
}
failed_re="$(status_line 'FAIL|TIMEOUT|LEAK-FAIL|XFAIL|ABORT|SIG[A-Z0-9]+|SIG [0-9]+')"
passed_re="$(status_line 'PASS|LEAK|FLAKY [0-9]+/[0-9]+')"
tests_matching() { sed -nE "s#$1#\\4 \\5#p" "$2" | sort -u; }

say() { echo "rerun-failed: $*"; }

# Why tests went unrun. nextest names a reason ("due to test failure") whenever
# it cancels; a run it ends with tests unrun and no reason is the case this
# exists to make visible, so that one names nextest's own exit status.
unrun=0
status="$(sed -nE 's/^nextest-run: nextest exited with status ([0-9]+)$/\1/p' "$work/step.log" | tail -n 1)"
while IFS= read -r warning; do
  unrun=1
  counts="$(sed -nE 's/.*warning: ([0-9]+\/[0-9]+) tests? (were|was) not run.*/\1/p' <<<"$warning")"
  case "$warning" in
    *" due to "*) say "nextest left $counts tests not run ${warning#* not run }" ;;
    *)
      if [ -n "$status" ]; then
        say "nextest exited with status $status and left $counts tests not run, and gave no reason: no test failed, timed out or was cancelled. See the 'priority' note in .config/nextest.toml for the one cause known here."
      else
        say "nextest left $counts tests not run and gave no reason, and its exit status is not in this log. See the 'priority' note in .config/nextest.toml for the one cause known here."
      fi
      ;;
  esac
done < <(grep -E 'warning: [0-9]+/[0-9]+ tests? (were|was) not run' "$work/step.log")

failed=()
while IFS= read -r test; do
  [ -n "$test" ] || continue
  read -r binary name <<<"$test"
  if ! [[ $binary =~ $id_shape ]] || ! [[ $name =~ $id_shape ]]; then
    say "skipping a failed-test line this cannot turn into a filter: $test"
    continue
  fi
  failed+=("$test")
done < <(tests_matching "$failed_re" "$work/step.log")

if [ "${#failed[@]}" -eq 0 ]; then
  if [ "$unrun" -eq 1 ] || grep -qE '^error: test run failed' "$work/step.log"; then
    say "the test run failed, and no test in it failed, so there is nothing to re-run. The job's verdict stays failed."
    exit 1
  fi
  say "$log names no failed test, so there is nothing to re-run (the step failed outside the test run)."
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
    --final-status-level none -E "$filter" 2>&1 | tee "$out"
  grouped && echo "::endgroup::"
  plain "$out" >"$out.plain"
  rebuilt="$(sed -nE 's/^[[:space:]]*Compiling ([^ ]+).*/\1/p' "$out.plain" | tr '\n' ' ')"
  if [ -n "$rebuilt" ]; then
    say "re-run $run of $times compiled ${rebuilt}before running, so it ran a fresh build of those, not the failed step's"
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
      say "re-run $run of $times: $test did not run"
    fi
    i=$((i + 1))
  done
done

say "summary: how often each failed test failed again on this build"
i=0
for test in "${failed[@]}"; do
  count="${fails[i]}"
  ran=$((times - unran[i]))
  if [ "$ran" -eq 0 ]; then
    reading="did not run in any re-run, so this says nothing about it"
  elif [ "$count" -eq "$ran" ]; then
    reading="fails every time on this build, which reads as a regression"
  elif [ "$count" -eq 0 ]; then
    reading="did not fail again, which reads as a flake"
  else
    reading="fails some of the time on this build, which reads as a flake"
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
say "the test step failed, and that verdict stands."
exit 1
