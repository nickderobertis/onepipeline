#!/usr/bin/env bash
# Run one Nx task's command ahead of the other tasks its Nx run starts beside it.
#
#   nx-precedence.sh [--keep PROJECT:TARGET]... -- COMMAND [ARG...]
#
# The e2e tier's coverage run no longer waits for the runner to empty
# (`tests/e2e/project.json`), and its journeys are timed against their own
# deadlines, so the work beside it may use only the CPU it leaves idle. While
# COMMAND runs, every process of another task in the same Nx run is moved to the
# lowest CPU priority (nice 19) and the idle I/O class, once a second, so a task
# that starts later is caught within that second and what it spawns afterwards
# inherits the priority. A `--keep` task is left where it is: it times something of
# its own, which the lowest priority would slow. Each task is named on stderr the
# first time it is lowered, so a job's log says what ran beside this one.
#
# A process belongs to the task its `NX_TASK_TARGET_PROJECT` and
# `NX_TASK_TARGET_TARGET` name, the variables Nx gives every task's environment, and
# to this Nx run when it descends from the process that started this task. Both are
# read from /proc, so outside Linux, or outside an Nx task, COMMAND simply runs.
# Nothing is ever given a higher priority, which would need privileges.
#
# Exit: COMMAND's status; 2 when called wrongly.
set -euo pipefail

usage() {
  echo "nx-precedence: $1" >&2
  echo "ACTION: run 'scripts/nx-precedence.sh [--keep PROJECT:TARGET]... -- COMMAND [ARG...]'" >&2
  exit 2
}

keep=" "
while [ "$#" -gt 0 ]; do
  case "$1" in
    --keep)
      case "${2:-}" in
        ?*:?*) keep+="$2 "; shift 2 ;;
        *) usage "--keep needs a PROJECT:TARGET, not '${2:-}'" ;;
      esac
      ;;
    --) shift; break ;;
    *) usage "unknown argument '$1'" ;;
  esac
done
[ "$#" -gt 0 ] || usage "name the command to run after --"

own="${NX_TASK_TARGET_PROJECT:-}:${NX_TASK_TARGET_TARGET:-}"
if [ "$own" = ":" ] || [ ! -r /proc/self/environ ]; then
  exec "$@"
fi

# The task a process runs for, as PROJECT:TARGET, read from its environment;
# empty for a process that is no task's or has already gone.
task_of() {
  local entry project="" target=""
  while IFS= read -r -d '' entry; do
    case "$entry" in
      NX_TASK_TARGET_PROJECT=*) project="${entry#*=}" ;;
      NX_TASK_TARGET_TARGET=*) target="${entry#*=}" ;;
    esac
  done 2>/dev/null <"/proc/$1/environ" || true
  [ -z "$project" ] || printf '%s:%s' "$project" "$target"
}

parent_of() {
  local stat state parent
  stat="$(cat "/proc/$1/stat" 2>/dev/null)" || return 1
  # After the command name, which may itself hold spaces and parentheses.
  read -r state parent _ <<<"${stat##*) }"
  [ -n "$state" ] && printf '%s' "$parent"
}

# Climb out of this task's own processes; the first that is not one of them
# started the task, and every task of the run descends from it.
task_root=$$
while :; do
  parent="$(parent_of "$task_root")" || exec "$@"
  [ "$parent" -gt 1 ] || exec "$@"
  [ "$(task_of "$parent")" = "$own" ] || break
  task_root="$parent"
done
run_root="$parent"

yield() {
  local kept=" " named=" " candidates pid task lower
  while kill -0 "$$" 2>/dev/null; do
    # Processes under the run's root, outside this task and the kept tasks found so
    # far, not yet at the lowest priority.
    candidates="$(ps -e -o pid=,ppid=,ni= | awk -v root="$run_root" -v skip="$task_root$kept" '
      { parent[$1] = $2; nice[$1] = $3 }
      END {
        n = split(skip, list, " ")
        for (i = 1; i <= n; i++) skipped[list[i]] = 1
        for (p in parent) {
          if (nice[p] + 0 >= 19 || p == root) continue
          for (q = p; ; q = parent[q]) {
            if (q in skipped) break
            if (parent[q] == root) { print p; break }
            if (!(parent[q] in parent) || parent[q] <= 1) break
          }
        }
      }')" || candidates=""
    lower=""
    for pid in $candidates; do
      task="$(task_of "$pid")"
      case "$task" in
        "" | "$own") ;;
        *) case "$keep" in
             *" $task "*) kept+="$pid " ;;
             *) lower+=" $pid"
                case "$named" in
                  *" $task "*) ;;
                  *) named+="$task "
                     echo "nx-precedence: $task runs at the lowest priority beside $own" >&2 ;;
                esac ;;
           esac ;;
      esac
    done
    if [ -n "$lower" ]; then
      # shellcheck disable=SC2086 # the pids are separate words by construction
      renice -n 19 -p $lower >/dev/null 2>&1 || true
      # shellcheck disable=SC2086
      ! command -v ionice >/dev/null 2>&1 || ionice -c 3 -p $lower >/dev/null 2>&1 || true
    fi
    sleep 1
  done
}

kept_names="${keep# }"
kept_names="${kept_names% }"
echo "nx-precedence: $own takes the CPU before the other tasks of its Nx run${kept_names:+ but $kept_names}" >&2
yield </dev/null >/dev/null &
yielder=$!
status=0
"$@" || status=$?
kill "$yielder" 2>/dev/null || true
wait "$yielder" 2>/dev/null || true
exit "$status"
