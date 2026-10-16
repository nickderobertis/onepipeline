#!/usr/bin/env bash
# Minimal reproduction of both Nx output losses, for the draft nrwl/nx issue: an
# empty Nx workspace at a given version, nothing else.
#
#   spike/nx-output-loss/nx-repro.sh NX_VERSION [RUNS] [DIR]
#
# 1. A failing task (`echo` to stdout and stderr, then `exit 1`) run RUNS times
#    with `nx run` whose stdout/stderr are files (no terminal), with one busy
#    loop per core beside it; prints how many runs lost each line.
#    (Each marker is split by quotes in its command so that Nx's echo of the
#    command line cannot itself satisfy the check.)
# 2. A cached task whose output is ~226KB, replayed RUNS times through a pipe
#    (`nx run ... | cat`) and once to a file; prints the bytes each received.
set -euo pipefail
version="${1:?usage: nx-repro.sh NX_VERSION [RUNS] [DIR]}"
runs="${2:-50}"
dir="${3:-${ONEPIPELINE_NODE_SCRATCH_DIR:-${TMPDIR:-/tmp}}/nx-repro-$version}"

if [ ! -f "$dir/.ready" ]; then
  rm -rf "$dir"
  mkdir -p "$dir"
  cd "$dir"
  cat >package.json <<EOF
{ "name": "nx-repro", "private": true, "devDependencies": { "nx": "$version" } }
EOF
  cat >nx.json <<'EOF'
{ "analytics": false, "targetDefaults": { "big": { "cache": true } } }
EOF
  cat >project.json <<'EOF'
{
  "name": "repro",
  "targets": {
    "fail": { "command": "echo OUT-''LINE; echo ERR-''LINE >&2; exit 1" },
    "big": { "command": "head -c 226000 /dev/zero | tr '\\0' x; echo; echo LAST-''LINE" }
  }
}
EOF
  git init -q && git add -A && git -c user.name=r -c user.email=r@localhost commit -qm init
  npm install --silent --no-audit --no-fund >/dev/null
  touch .ready
fi
cd "$dir"
export NX_DAEMON=false
unset CI GITHUB_ACTIONS
# Run as a person or a CI job would: Nx 23.3+ switches to its `summary` output
# style (no task output inline) when these say an AI agent is driving it.
unset CLAUDECODE CLAUDE_CODE AI_AGENT REPL_ID CODEX_THREAD_ID CURSOR_TRACE_ID GEMINI_CLI OPENCODE AGENT
echo "nx $(node -p "require('./node_modules/nx/package.json').version")"

loaders=()
trap 'for p in "${loaders[@]}"; do kill "$p" 2>/dev/null || true; done' EXIT
for _ in $(seq "$(nproc)"); do (while :; do :; done) & loaders+=("$!"); done
lost_out=0 lost_err=0
for _ in $(seq "$runs"); do
  ./node_modules/.bin/nx run repro:fail >out.txt 2>err.txt </dev/null || true
  grep -q OUT-LINE out.txt err.txt || lost_out=$((lost_out + 1))
  grep -q ERR-LINE out.txt err.txt || lost_err=$((lost_err + 1))
done
for p in "${loaders[@]}"; do kill "$p" 2>/dev/null || true; done
loaders=()
echo "failing task, under load: runs=$runs lost_stdout_line=$lost_out lost_stderr_line=$lost_err"

./node_modules/.bin/nx run repro:big >/dev/null 2>&1 </dev/null # store it in the cache
short=0 min=""
for _ in $(seq "$runs"); do
  bytes=$(./node_modules/.bin/nx run repro:big 2>&1 </dev/null | cat | wc -c)
  [ -n "$min" ] && ((bytes >= min)) || min=$bytes
  ./node_modules/.bin/nx run repro:big 2>&1 </dev/null | grep -q LAST-LINE || short=$((short + 1))
done
file_bytes=$(./node_modules/.bin/nx run repro:big >file.txt 2>&1 </dev/null; wc -c <file.txt)
echo "cached replay through a pipe: runs=$runs missing_last_line=$short min_bytes=$min; to a file: bytes=$file_bytes"
