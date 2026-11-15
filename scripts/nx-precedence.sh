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
# its own, which the lowest priority would slow.
#
# A process belongs to the task its `NX_TASK_TARGET_PROJECT` and
# `NX_TASK_TARGET_TARGET` name, and to this Nx run when its `NX_INVOCATION_ROOT_PID`
# is this task's: the variables Nx gives every task's environment, which the
# processes a task starts inherit. They are read from /proc, so outside Linux, or
# outside an Nx task, COMMAND simply runs. Nothing is ever given a higher priority,
# which would need privileges.
#
# Output: once COMMAND ends, one line naming the tasks it ran ahead of. A process
# that could not be listed or lowered is named on stderr when it happens, once per
# cause, and COMMAND goes on at the priority it had.
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
      [[ "${2:-}" =~ ^[^[:space:]:]+:[^[:space:]:]+$ ]] \
        || usage "--keep needs one PROJECT:TARGET, not '${2:-}'"
      keep+="$2 "
      shift 2
      ;;
    --) shift; break ;;
    *) usage "unknown argument '$1'" ;;
  esac
done
[ "$#" -gt 0 ] || usage "name the command to run after --"

own="${NX_TASK_TARGET_PROJECT:-}:${NX_TASK_TARGET_TARGET:-}"
run="${NX_INVOCATION_ROOT_PID:-}"
if [ "$own" = ":" ] || [ -z "$run" ] || [ ! -r /proc/self/environ ]; then
  exec "$@"
fi

# The task a process runs for, as PROJECT:TARGET, read from its environment;
# empty for a process that is no task's, has already gone, or whose environment
# could not be read whole (a process mid-exec), which is then left alone.
task_of() {
  local entry project="" target=""
  while IFS= read -r -d '' entry; do
    case "$entry" in
      NX_TASK_TARGET_PROJECT=*) project="${entry#*=}" ;;
      NX_TASK_TARGET_TARGET=*) target="${entry#*=}" ;;
    esac
  done 2>/dev/null <"/proc/$1/environ" || true
  [ -z "$project" ] || [ -z "$target" ] || printf '%s:%s' "$project" "$target"
}

lowered="$(mktemp)"
trap 'rm -f "$lowered"' EXIT

yield() {
  local warned=" " listing candidates pid nice task lower tasks out alive
  local -a pids environs
  warn() {
    case "$warned" in *" $1 "*) return 0 ;; esac
    warned+="$1 "
    echo "nx-precedence: $2" >&2
    echo "ACTION: $3" >&2
  }
  while kill -0 "$$" 2>/dev/null; do
    if ! listing="$(ps -e -o pid=,ni= 2>&1)"; then
      warn ps "cannot list processes (${listing//$'\n'/ }), so the tasks beside $own keep their priority until it can" \
        "put procps' ps on PATH"
      sleep 1
      continue
    fi
    # This run's processes not yet at the lowest priority: one `grep` reads every
    # candidate's environment for the run's root.
    environs=()
    while read -r pid nice; do
      [ "$nice" -lt 19 ] 2>/dev/null && [ "$pid" != "$$" ] && environs+=("/proc/$pid/environ")
    done <<<"$listing"
    candidates=""
    if [ "${#environs[@]}" -gt 0 ]; then
      candidates="$(grep -lzx "NX_INVOCATION_ROOT_PID=$run" "${environs[@]}" 2>/dev/null | cut -d/ -f3)" || true
    fi
    lower=""
    tasks=""
    for pid in $candidates; do
      task="$(task_of "$pid")"
      case "$task" in
        "" | "$own") ;;
        *) case "$keep" in
             *" $task "*) ;;
             *) lower+=" $pid"; tasks+="$task"$'\n' ;;
           esac ;;
      esac
    done
    if [ -n "$lower" ]; then
      read -ra pids <<<"$lower"
      # A process that ended since it was listed refuses too, so only one still
      # running above the lowest priority counts as a failure.
      if out="$(renice -n 19 -p "${pids[@]}" 2>&1 >/dev/null)"; then
        printf '%s' "$tasks" >>"$lowered"
      else
        alive="$(IFS=,; ps -o ni= -p "${pids[*]}" 2>/dev/null | awk '$1 + 0 < 19' || true)"
        if [ -z "$alive" ]; then
          printf '%s' "$tasks" >>"$lowered"
        else
          warn renice "cannot lower the tasks beside $own (${out//$'\n'/ })" \
            "run it where util-linux's renice may change this user's own processes"
        fi
      fi
      if ! command -v ionice >/dev/null 2>&1; then
        warn ionice "found no ionice, so the tasks beside $own keep their I/O priority" \
          "put util-linux's ionice on PATH"
      elif ! ionice -c 3 -p "${pids[@]}" >/dev/null 2>&1; then
        for pid in "${pids[@]}"; do
          out="$(ionice -c 3 -p "$pid" 2>&1 >/dev/null)" || [ ! -d "/proc/$pid" ] \
            || warn ionice "cannot move the tasks beside $own to the idle I/O class (${out//$'\n'/ })" \
              "run it where util-linux's ionice may change this user's own processes"
        done
      fi
    fi
    sleep 1
  done
}

yield </dev/null >/dev/null &
yielder=$!
status=0
"$@" || status=$?
kill "$yielder" 2>/dev/null || true
wait "$yielder" 2>/dev/null || true
ahead="$(sort -u "$lowered" | paste -sd ' ' -)"
kept_names="${keep# }"
kept_names="${kept_names% }"
echo "nx-precedence: $own ran ahead of ${ahead:-no other task}${kept_names:+, keeping $kept_names}" >&2
exit "$status"
