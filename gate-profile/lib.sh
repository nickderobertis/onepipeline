# Sourced by the shims and by nx-hook.sh; never run. Shared helpers for `just gate-profile`.

# The genuine tool `$1` a shim execs. Under `just gate-profile` that is the path run.mjs resolved
# (real-tools.mjs) when the run started, before any test could change PATH, and handed down as
# ONEPIPELINE_GATE_PROFILE_REAL_<TOOL>: it is never looked up again. A test that puts its own fake
# of the tool first on PATH, and whose fake execs the tool it finds behind itself — this shim —
# then still reaches the genuine tool, where a lookup on the current PATH would find the fake and
# exec it, and the fake the shim, forever. A resolved path that is a shim, or nothing runnable, is
# refused rather than exec'd. Only with nothing resolved (a shim run outside `just gate-profile`)
# is the tool looked up: the first on PATH outside every shim directory, each of which carries the
# marker, so a profiled run nested inside another skips both runs' shims.
gate_profile_real_tool() {
  local variable dir dirs resolved
  variable=ONEPIPELINE_GATE_PROFILE_REAL_${1^^}
  variable=${variable//[^A-Z0-9_]/_}
  resolved=${!variable-}
  if [ -n "$resolved" ]; then
    if [[ $resolved != /* ]] || [ ! -x "$resolved" ] || [ -d "$resolved" ] || gate_profile_is_shim "$resolved"; then
      echo "gate-profile: $variable=$resolved is not the real $1 (an absolute path to it, outside the profiling shims); it is set only by \`just gate-profile\`, so unset it and run that instead" >&2
      return 1
    fi
    printf '%s\n' "$resolved"
    return 0
  fi
  IFS=: read -ra dirs <<<"$PATH"
  for dir in "${dirs[@]}"; do
    [ -n "$dir" ] && [ -x "$dir/$1" ] && [ ! -d "$dir/$1" ] && ! gate_profile_is_shim "$dir/$1" || continue
    printf '%s\n' "$dir/$1"
    return 0
  done
  echo "gate-profile: no $1 on PATH outside the profiling shims; install it, or run without \`just gate-profile\`" >&2
  return 1
}

# Whether `$1` is one of the profiling shims: it, or the file it links to, sits in a directory
# carrying the marker. `$1` is a path with a directory part, as every caller's is.
gate_profile_is_shim() {
  local target
  [ -e "${1%/*}/.gate-profile-shims" ] && return 0
  [ -L "$1" ] || return 1
  target=$(readlink -f -- "$1") || return 1
  [ -e "${target%/*}/.gate-profile-shims" ]
}

# A JSON string literal of `$1`. Records hold recipe names and argv, so quotes, backslashes and
# the usual whitespace escapes are what occur; any other control character is dropped.
gate_profile_json_string() {
  local s=$1
  s=${s//\\/\\\\}
  s=${s//\"/\\\"}
  s=${s//$'\n'/\\n}
  s=${s//$'\t'/\\t}
  s=${s//$'\r'/\\r}
  printf '"%s"' "${s//[$'\001'-$'\037']/}"
}

# A JSON array of the arguments.
gate_profile_json_array() {
  local out="" arg
  for arg in "$@"; do out+="${out:+,}$(gate_profile_json_string "$arg")"; done
  printf '[%s]' "$out"
}

# Sets gate_profile_cpu to this shell's children's CPU so far, "user,system" in milliseconds, from
# bash's `times`: its second line totals every waited-for descendant. Called directly, never in a
# command substitution, whose subshell's children's times start again at zero; and it forks
# nothing, so the figures it brackets carry none of its own cost.
gate_profile_children_cpu() {
  local _ user system
  times >"$1"
  { read -r _ && read -r user system; } <"$1"
  gate_profile_ms "$user"
  user=$gate_profile_ms_value
  gate_profile_ms "$system"
  gate_profile_cpu="$user,$gate_profile_ms_value"
}

# Sets gate_profile_ms_value from `1m2.345s` to `62345`.
gate_profile_ms() {
  local minutes=${1%%m*} seconds=${1#*m}
  seconds=${seconds%s}
  gate_profile_ms_value=$((10#$minutes * 60000 + 10#${seconds%.*} * 1000 + 10#${seconds#*.}))
}

# Whether this process runs at the profiled run's own level rather than inside one of its Nx
# tasks: Nx gives each task its hash, and the run recorded the one (or none) it started under.
gate_profile_at_top() {
  [ "${NX_TASK_HASH-}" = "${ONEPIPELINE_GATE_PROFILE_TOP-}" ]
}

# Says on stderr that this run's profile could not be recorded, and exits 3. It also notes that in
# the run's faults file, created by run.mjs before the recipe starts: a shim inside an Nx task
# reaches run.mjs only as its task's failure, which would otherwise read as the recipe failing.
gate_profile_unrecorded() {
  echo "gate-profile: $1" >&2
  if [ -n "${ONEPIPELINE_GATE_PROFILE_FAULTS-}" ]; then
    printf '%s\n' "$1" >>"$ONEPIPELINE_GATE_PROFILE_FAULTS" || true
  fi
  exit 3
}

# Refuses a run directory that is not one: a stale variable must fail loudly, not write elsewhere.
gate_profile_check_dir() {
  [ -d "$ONEPIPELINE_GATE_PROFILE_DIR" ] && return 0
  echo "gate-profile: ONEPIPELINE_GATE_PROFILE_DIR=$ONEPIPELINE_GATE_PROFILE_DIR is not a directory; it is set only by \`just gate-profile\`, so unset it and run that instead" >&2
  return 2
}
