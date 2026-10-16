#!/usr/bin/env bash
# THROWAWAY: re-capture every before/after screenshot of the spike-visual report.
#   spike/visual/capture.sh [OUT_DIR]
# Befores run the unchanged base's real wrapper (BASE, default 9e69fc50) in a temporary
# worktree; afters run this branch's plain Nx target. Both use the stand-in llmlint in
# spike/visual/bin, so no model is called. Needs freeze 0.2.2, node_modules, just.
set -euo pipefail
repo=$(cd "$(dirname "$0")/../.." && pwd)
base=${BASE:-9e69fc5031ec315eb67ce95f9b12fe4196654666}
work=$(mktemp -d); out=${1:-$work/out}; mkdir -p "$out"
bin="$repo/spike/visual/bin"
basewt="$work/base"
git -C "$repo" worktree add --detach "$basewt" "$base" >/dev/null 2>&1
trap 'git -C "$repo" worktree remove --force "$basewt" >/dev/null 2>&1; [ "$out" = "$work/out" ] && find "$work" -mindepth 1 -maxdepth 1 ! -name out -exec rm -rf {} + || rm -rf "$work"' EXIT
ln -s "$repo/node_modules" "$basewt/node_modules"
# The base wrapper prepends $HOME/.local/bin, so the stand-in goes there in a fake HOME.
mkdir -p "$work/home/.local/bin"; ln -s "$bin/llmlint" "$work/home/.local/bin/llmlint"
sha=$(git -C "$repo" rev-parse origin/main)

shot() { # name dir display-command command...
  local name=$1 dir=$2 shown=$3; shift 3; local txt="$work/$name.txt" code=0
  { echo "\$ $shown"; (cd "$dir" && "$@") 2>&1 || code=$?; echo "[exit status $code]"; } \
    | sed "s#$work#/home/you#g; s#$repo#/home/you/onepipeline#g" > "$txt" || true
  freeze "$txt" --language ansi --font.file "$repo/screenshots/fonts/JetBrainsMono-Regular.ttf" \
    --font.family "JetBrains Mono" --font.size 14 --window --background "#0d1117" \
    --padding 20,30 --margin 0 --border.radius 8 --width 1155 --wrap 130 \
    -o "$out/$name.png" </dev/null >/dev/null
  echo "wrote $out/$name.png" >&2
}
common=(env FORCE_COLOR=1 CARGO_HOME="${CARGO_HOME:-$HOME/.cargo}" RUSTUP_HOME="${RUSTUP_HOME:-$HOME/.rustup}" PATH="$bin:$PATH")
lint="just lint-llm-diff origin/main"
for side in before after; do
  if [ $side = before ]; then dir=$basewt; envs=("${common[@]}" HOME="$work/home")
  else dir=$repo; envs=("${common[@]}"); fi
  # A fresh untracked nonce in the tree, so the first pass is always a cache miss.
  echo "$RANDOM$RANDOM pass" > "$dir/spike-visual.nonce"
  shot $side-pass "$dir" "$lint" "${envs[@]}" SPIKE_LLMLINT_MODE=pass just lint-llm-diff origin/main
  shot $side-replay "$dir" "$lint" "${envs[@]}" SPIKE_LLMLINT_MODE=pass just lint-llm-diff origin/main
  # A run with findings is a different tree: a new nonce makes it the miss it would be.
  echo "$RANDOM$RANDOM findings" > "$dir/spike-visual.nonce"
  shot $side-findings "$dir" "$lint" "${envs[@]}" SPIKE_LLMLINT_MODE=fail just lint-llm-diff origin/main
  shot $side-oversize "$dir" "llmlint --diff --diff-base $sha" \
    "${envs[@]}" SPIKE_LLMLINT_MODE=oversize-$side llmlint --diff --diff-base "$sha"
  rm -f "$dir/spike-visual.nonce"
done
echo "screenshots in $out" >&2
