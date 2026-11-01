#!/usr/bin/env bash
# `just gate`'s timing wrapper: runs the gate's whole body (the command given), records its
# start and end to .gate-time.json at the repository root (gitignored), then checks the
# `host-variable` budget onepipeline-gate-time against that record.
#
#   bash scripts/gate-time.sh <command> [args...]
#   bash scripts/gate-time.sh --check     # check the last recorded gate again, running nothing
#
# A development machine's timing varies too much to refuse on, so an over reading (exit 1 from
# onebudgetspec) prints a warning naming the budget and the gate still passes; an invalid
# budgets file (2) or a failed measurement (3) fails it. A body that fails fails the gate with
# its own status, and no budget is checked: a failed gate has no wall clock worth reporting.
# CI does not run this: its jobs are held to their own `timeout-minutes`.
set -uo pipefail

[ $# -gt 0 ] || { echo "usage: gate-time.sh <command> [args...] | --check" >&2; exit 2; }

root="$(cd "$(dirname "$0")/.." && pwd)"
record="$root/.gate-time.json"
stamp() { date -u +%Y-%m-%dT%H:%M:%SZ; }

check_budget() {
  local checker budget=0
  # The checker the justfile pins, which `just bootstrap` installs and `just budgets` runs.
  checker="$(cd "$root" && just --evaluate onebudgetspec-root)/bin/onebudgetspec"
  [ -x "$checker" ] || checker="$checker.exe"
  if [ ! -x "$checker" ]; then
    echo "gate-time: the repository-pinned onebudgetspec is missing, so onepipeline-gate-time" \
      "cannot be checked; run \`just bootstrap\` and \`just gate\` again." >&2
    exit 3
  fi
  (cd "$root" && "$checker" check budgets.yaml --label host-variable) || budget=$?
  case "$budget" in
    0) ;;
    1)
      echo "gate-time: warning: onepipeline-gate-time is over its budget (reading above). The gate" \
        "still passes: a development machine's timing varies too much to refuse on." >&2
      ;;
    *)
      echo "gate-time: checking onepipeline-gate-time failed (onebudgetspec exit $budget); fix" \
        "budgets.yaml or the record and re-run \`just gate\`." >&2
      exit "$budget"
      ;;
  esac
}

if [ "$1" = --check ] && [ $# -eq 1 ]; then
  check_budget
  exit 0
fi

record_failed() {
  echo "gate-time: cannot $1 $record: ${2:-it is still there}" >&2
  echo "gate-time: next: make .gate-time.json at the repository root a writable file (or" \
    "remove whatever is there) and run \`just gate\` again." >&2
  exit 2
}

# A record left by an earlier gate never stands in for this one, even if this one is killed,
# so it goes before the body starts and the gate refuses to run when it cannot.
if ! error="$(rm -f "$record" 2>&1)" || [ -e "$record" ]; then
  record_failed "remove the earlier record" "$error"
fi
started="$(stamp)"
"$@"
status=$?
ended="$(stamp)"
if ! error="$( { printf '{"schema_version":1,"invocation":"%s-%s","started_at":"%s","ended_at":"%s","exit":%d}\n' \
  "$$" "$started" "$started" "$ended" "$status" >"$record.$$"; } 2>&1)"; then
  record_failed "write the record beside" "$error"
fi
if ! error="$(mv "$record.$$" "$record" 2>&1)"; then
  rm -f "$record.$$"
  record_failed "replace" "$error"
fi

if [ "$status" -ne 0 ]; then
  echo "gate-time: the gate failed (exit $status), so its wall clock is not checked; fix the" \
    "failure reported above and run \`just gate\` again." >&2
  exit "$status"
fi

check_budget
