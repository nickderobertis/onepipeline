#!/usr/bin/env bash
# One measurement of whether Nx loses a plain llmlint target's output.
#
#   spike/nx-output-loss/measure.sh \
#     --arrangement onepipeline|ai-orchestrator \
#     --invoke nx|nxsh|nxsh-show \
#     --pty default|false|true \
#     --terminal tty|notty|ci \
#     --load on|off \
#     --mode fail|replay \
#     --runs N [--scratch DIR] [--record FILE]
#
# It runs `nx run <project>:<target>` N times, where the target's command is
# `llmlint --diff --diff-base $LLMLINT_DIFF_BASE_SHA` and `llmlint` is the stand-in
# beside this script, first on PATH. It prints how many runs lost the stand-in's
# stdout line, how many lost its stderr line, and how many runs there were.
#
# Axes:
#   --arrangement  onepipeline: a copy of this checkout's HEAD with its own
#                  lockfile's Nx (^23.1.1) and its own scripts/nx.sh.
#                  ai-orchestrator: a scratch clone of its `main` with its own
#                  lockfile's Nx (23.1.0) and its own scripts/nx.sh.
#   --invoke       nx: node_modules/.bin/nx directly. nxsh: the repository's
#                  scripts/nx.sh as its recipes call it (onepipeline: log mode,
#                  output only on failure; ai-orchestrator: its redaction pipe).
#                  nxsh-show: onepipeline's scripts/nx.sh with
#                  ONEPIPELINE_NX_SHOW_OUTPUT=1, or ai-orchestrator's with
#                  AI_ORCHESTRATOR_NX_SHOW_OUTPUT=1.
#   --pty          the target's run-commands `usePty` option: absent, false, true.
#   --terminal     tty: under a pseudo-terminal (util-linux `script`), as a
#                  developer at a terminal; notty: stdout/stderr are files, stdin
#                  /dev/null, CI unset; ci: as notty with CI=true and
#                  GITHUB_ACTIONS=true, as a GitHub Actions step.
#   --load         on: one busy loop per core (nproc) runs beside every run.
#   --mode         fail: the stand-in prints one line to stdout and one to stderr
#                  and exits at once, alternating exit 1 (findings) and 2
#                  (refusal) run by run; a run is lost when either line is missing
#                  from what the invoking command printed. replay: one warm-up run
#                  stores a ~226KB passing output, then every measured run must be
#                  a cache hit, and is lost when the replay's last line is missing.
#
# Exits 0 when the measurement was taken (whatever it found), non-zero when it
# could not be. Kept on a preserved spike branch; not part of any gate.
set -euo pipefail

here="$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd)"
repo="$(git -C "$here" rev-parse --show-toplevel)"

arrangement=onepipeline invoke=nx pty=default terminal=notty load=off mode=fail runs=50
scratch="${ONEPIPELINE_NODE_SCRATCH_DIR:-${TMPDIR:-/tmp}}/nxloss"
record=""
while (($#)); do
  case "$1" in
  --arrangement) arrangement=$2 ;;
  --invoke) invoke=$2 ;;
  --pty) pty=$2 ;;
  --terminal) terminal=$2 ;;
  --load) load=$2 ;;
  --mode) mode=$2 ;;
  --runs) runs=$2 ;;
  --scratch) scratch=$2 ;;
  --record) record=$2 ;;
  -h | --help) sed -n '2,/^set -euo/p' "$0" | sed '$d; s/^# \{0,1\}//'; exit 0 ;;
  *) echo "measure: unknown argument '$1'; see --help" >&2; exit 64 ;;
  esac
  shift 2
done
case "$arrangement" in onepipeline | ai-orchestrator) ;; *) echo "measure: --arrangement must be onepipeline or ai-orchestrator" >&2; exit 64 ;; esac
case "$invoke" in nx | nxsh | nxsh-show) ;; *) echo "measure: --invoke must be nx, nxsh or nxsh-show" >&2; exit 64 ;; esac
case "$pty" in default | false | true) ;; *) echo "measure: --pty must be default, false or true" >&2; exit 64 ;; esac
case "$terminal" in tty | notty | ci) ;; *) echo "measure: --terminal must be tty, notty or ci" >&2; exit 64 ;; esac
case "$load" in on | off) ;; *) echo "measure: --load must be on or off" >&2; exit 64 ;; esac
case "$mode" in fail | replay) ;; *) echo "measure: --mode must be fail or replay" >&2; exit 64 ;; esac
[[ "$runs" =~ ^[1-9][0-9]*$ ]] || { echo "measure: --runs must be a positive integer" >&2; exit 64; }
mkdir -p "$scratch"
scratch="$(cd "$scratch" && pwd)"

target=lint-plain
[ "$pty" = default ] || target="lint-plain-pty-$pty"

# The plain target, three times over: absent, false and true `usePty`. Cached as the
# plan's target is, keyed on the tree and the base; NXLOSS_MODE is a key too, so a
# failing run can never be answered by the replay mode's stored pass.
add_targets() { # <project.json>
  node - "$1" <<'EOF'
const fs = require("fs");
const file = process.argv[2];
const project = JSON.parse(fs.readFileSync(file, "utf8"));
const base = (extra) => ({
  command: "llmlint --diff --diff-base $LLMLINT_DIFF_BASE_SHA",
  options: { cwd: ".", ...extra },
  cache: true,
  inputs: [
    "{workspaceRoot}/**/*",
    { env: "LLMLINT_DIFF_BASE_SHA" },
    { env: "NXLOSS_MODE" },
  ],
});
project.targets["lint-plain"] = base({});
project.targets["lint-plain-pty-false"] = base({ usePty: false });
project.targets["lint-plain-pty-true"] = base({ usePty: true });
fs.writeFileSync(file, JSON.stringify(project, null, 2) + "\n");
// A developer's first interactive run answers Nx's usage-data prompt once and Nx
// persists the answer in nx.json; without it a run under a terminal blocks on it.
const nxFile = require("path").join(require("path").dirname(file), "nx.json");
const nx = JSON.parse(fs.readFileSync(nxFile, "utf8"));
if (typeof nx.analytics !== "boolean") nx.analytics = false;
fs.writeFileSync(nxFile, JSON.stringify(nx, null, 2) + "\n");
EOF
}

prepare() {
  local ws=$1
  [ -f "$ws/.nxloss-ready" ] && return 0
  rm -rf "$ws"
  mkdir -p "$ws"
  case "$arrangement" in
  onepipeline)
    git -C "$repo" archive HEAD | tar -x -C "$ws"
    (cd "$ws" && npm ci --silent --no-audit --no-fund >&2)
    ;;
  ai-orchestrator)
    git clone -q --depth 1 --branch main https://github.com/nickderobertis/ai-orchestrator "$ws"
    (cd "$ws" && bun install --frozen-lockfile >/dev/null && uv venv -q .venv)
    ;;
  esac
  add_targets "$ws/project.json"
  (
    cd "$ws"
    [ -d .git ] || git init -q
    git add -A project.json nx.json
    [ "$arrangement" = onepipeline ] && git add -A .
    git -c user.name=nxloss -c user.email=nxloss@localhost commit -qm "nxloss: plain llmlint targets" --no-verify
  )
  touch "$ws/.nxloss-ready"
}

ws="$scratch/ws-$arrangement"
prepare "$ws"
project=onepipeline
[ "$arrangement" = ai-orchestrator ] && project=workspace
nx_version="$(node -p "require('$ws/node_modules/nx/package.json').version")"
base_sha="$(git -C "$ws" rev-parse HEAD)"

cmd=()
case "$invoke" in
nx) cmd=(node_modules/.bin/nx run "$project:$target") ;;
nxsh | nxsh-show) cmd=(bash scripts/nx.sh run "$project:$target") ;;
esac

# The environment every run shares. ai-orchestrator's nx.sh keys its Nx cache under
# XDG_CACHE_HOME by remote URL, which would otherwise be this host's real cache.
run_env=(
  "PATH=$here:$PATH"
  "LLMLINT_DIFF_BASE_SHA=$base_sha"
  "XDG_CACHE_HOME=$scratch/xdg-cache"
  "NX_DAEMON=false"
)
unset_env=(-u CI -u GITHUB_ACTIONS -u ONEPIPELINE_NX_SHOW_OUTPUT -u AI_ORCHESTRATOR_NX_SHOW_OUTPUT -u NX_TUI)
case "$terminal" in
ci) run_env+=("CI=true" "GITHUB_ACTIONS=true") ;;
tty) unset_env+=(-u CLAUDECODE -u AI_AGENT -u CLAUDE_CODE_ENTRYPOINT) ;;
esac
if [ "$invoke" = nxsh-show ]; then
  run_env+=("ONEPIPELINE_NX_SHOW_OUTPUT=1" "AI_ORCHESTRATOR_NX_SHOW_OUTPUT=1")
elif [ "$mode" = replay ]; then
  # A replay can only be read when the wrapper prints what Nx printed on success.
  run_env+=("AI_ORCHESTRATOR_NX_SHOW_OUTPUT=1")
fi

config="arrangement=$arrangement invoke=$invoke pty=$pty terminal=$terminal load=$load mode=$mode nx=$nx_version"
slug="$(printf '%s' "$config" | tr ' =' '_-')"
lostdir="$scratch/lost/$slug"
rm -rf "$lostdir"
mkdir -p "$lostdir"

# One run: stdout to $1.out, stderr to $1.err (merged into .out under a terminal).
run_once() { # <prefix> <extra env...>
  local prefix=$1
  shift
  if [ "$terminal" = tty ]; then
    local line
    line="$(printf '%q ' env "${unset_env[@]}" "${run_env[@]}" "$@" "${cmd[@]}")"
    (cd "$ws" && timeout 300 script -qefc "$line" /dev/null >"$prefix.out" 2>"$prefix.err" </dev/null)
  else
    (cd "$ws" && timeout 300 env "${unset_env[@]}" "${run_env[@]}" "$@" "${cmd[@]}" >"$prefix.out" 2>"$prefix.err" </dev/null)
  fi
}

loaders=()
stop_load() {
  local pid
  for pid in "${loaders[@]}"; do kill "$pid" 2>/dev/null || true; done
  for pid in "${loaders[@]}"; do wait "$pid" 2>/dev/null || true; done
  loaders=()
}
trap stop_load EXIT
if [ "$load" = on ]; then
  for _ in $(seq "$(nproc)"); do
    (while :; do :; done) &
    loaders+=("$!")
  done
fi

work="$scratch/work.$$"
mkdir -p "$work"
if [ "$mode" = replay ]; then
  # The warm-up stores the pass; it is not counted.
  status=0
  run_once "$work/warmup" NXLOSS_MODE=pass NXLOSS_NONCE=replay || status=$?
  if ((status != 0)); then
    echo "measure: the replay warm-up failed (exit $status); see $work/warmup.out" >&2
    exit 1
  fi
fi

lost_out=0 lost_err=0 lost_any=0 timed_out=0 not_failed=0 not_hit=0 min_bytes="" started=$SECONDS
for i in $(seq "$runs"); do
  nonce="$$-$i-$RANDOM$RANDOM"
  prefix="$work/run"
  status=0
  if [ "$mode" = fail ]; then
    code=$((i % 2 == 1 ? 1 : 2))
    run_once "$prefix" NXLOSS_MODE=fail NXLOSS_NONCE="$nonce" NXLOSS_EXIT="$code" || status=$?
    seen="$(cat "$prefix.out" "$prefix.err")"
    miss_out=0 miss_err=0
    grep -qF "NXLOSS-STDOUT $nonce " <<<"$seen" || miss_out=1
    grep -qF "NXLOSS-STDERR $nonce " <<<"$seen" || miss_err=1
    ((status != 0)) || not_failed=$((not_failed + 1))
  else
    run_once "$prefix" NXLOSS_MODE=pass NXLOSS_NONCE=replay || status=$?
    seen="$(cat "$prefix.out" "$prefix.err")"
    bytes=$(cat "$prefix.out" "$prefix.err" | wc -c)
    [ -z "$min_bytes" ] || ((bytes >= min_bytes)) || min_bytes=$bytes
    [ -n "$min_bytes" ] || min_bytes=$bytes
    miss_out=0 miss_err=0
    grep -qF "NXLOSS-REPLAY-END" <<<"$seen" || miss_out=1
    grep -qF "debug: judge call 2 " <<<"$seen" || miss_err=1
    grep -qE "local cache|existing outputs match the cache" <<<"$seen" || not_hit=$((not_hit + 1))
    ((status == 0)) || not_failed=$((not_failed + 1))
  fi
  ((status != 124)) || timed_out=$((timed_out + 1))
  lost_out=$((lost_out + miss_out))
  lost_err=$((lost_err + miss_err))
  if ((miss_out || miss_err)); then
    lost_any=$((lost_any + 1))
    cp "$prefix.out" "$lostdir/run-$i.out"
    cp "$prefix.err" "$lostdir/run-$i.err"
  fi
done
elapsed=$((SECONDS - started))
stop_load
rm -rf "$work"

echo "config: $config"
if [ "$mode" = fail ]; then
  echo "runs=$runs lost_stdout=$lost_out lost_stderr=$lost_err lost_any=$lost_any exited_zero=$not_failed timed_out=$timed_out seconds=$elapsed"
else
  echo "runs=$runs lost_stdout=$lost_out lost_stderr=$lost_err lost_any=$lost_any not_cache_hit=$not_hit exited_nonzero=$not_failed min_bytes=$min_bytes timed_out=$timed_out seconds=$elapsed"
fi
((lost_any == 0)) || echo "lost runs' output: $lostdir"
if [ -n "$record" ]; then
  printf '%s\t%s\n' "$config" "runs=$runs lost_stdout=$lost_out lost_stderr=$lost_err lost_any=$lost_any exited_unexpectedly=$not_failed not_cache_hit=$not_hit min_bytes=${min_bytes:-} timed_out=$timed_out seconds=$elapsed" >>"$record"
fi
