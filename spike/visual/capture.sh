#!/usr/bin/env bash
# THROWAWAY (spike-visual, stop-guard-flow-coverage-2026-10-10): the one command
# that re-captures every before/after screenshot of the spike report.
#
#   spike/visual/capture.sh [OUT_DIR]     # default: a fresh mktemp -d, printed
#
# Before = the real `onepipeline` built from this tree, whose src/ and manifests
# are refused unless identical to BASE (the unchanged base this branch was cut
# from) — except `watch`, whose before is the deployed gallery's capture
# (origin/gh-pages x86_64/watch.svg) when its digest matches
# shots/baseline/x86_64.json, else a fresh `screenshots/capture.sh` on this tree.
# After = spike/visual/mock-onepipeline, which delegates everything it does not
# mock to that same base binary.
# Every shot: freeze with screenshots/capture.py's exact settings (118 columns,
# 1054 px window, 14 pt vendored JetBrains Mono) to SVG, then rasterized to PNG
# at 2x by the Playwright chrome-headless-shell, gallery SVG included.
set -euo pipefail
repo="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$repo"
BASE="${BASE:-6d0c29a}"
out="${1:-$(mktemp -d -t spike-visual-XXXXXX)}"
mkdir -p "$out"; out="$(cd "$out" && pwd)"
chrome="${CHROME:-$(ls -d "$HOME"/.cache/ms-playwright/chromium_headless_shell-*/*/chrome-headless-shell | sort | tail -1)}"
session="9b2e41c7-5d1a-4f0e-8c3b-2a6f0d7e1b54"

git diff --quiet "$BASE" HEAD -- src crates Cargo.toml Cargo.lock build.rs \
  || { echo "capture: this tree's engine differs from BASE $BASE; the before would not be the base" >&2; exit 1; }
cargo build --release --locked --bin onepipeline >&2
real="$repo/target/release/onepipeline"

work="$(mktemp -d -t spike-visual-work-XXXXXX)"; trap 'rm -rf "$work"' EXIT
mkdir -p "$work/before-bin" "$work/after-bin" "$work/runs" "$work/state"
ln -s "$real" "$work/before-bin/onepipeline"
ln -s "$repo/spike/visual/mock-onepipeline" "$work/after-bin/onepipeline"

# Fixture: the session's only run, settled (the recorded run root, its driver
# pid rewritten to one no platform issues) and closed by a real acknowledgement.
cp -r tests/recorded/run-root/onemessagebus-repair-2 "$work/runs/"
sed -i -E "s/\"pid\": [0-9]+,/\"pid\": 2147483647,/; s/\"session\": \"[^\"]+\"/\"session\": \"$session\"/" \
  "$work/runs/onemessagebus-repair-2/launch.json"
for v in $(env | grep -oE '^ONE[A-Z]*_[A-Z_]*' ); do unset "$v"; done
export ONEPIPELINE_RUNS_DIR="$work/runs" XDG_STATE_HOME="$work/state" REAL_ONEPIPELINE="$real"
export ONEPIPELINE_ONEAGENTGRAPH_BIN=/nonexistent/oneagentgraph
"$real" unwatched --session "$session" --acknowledge onemessagebus-repair-2 \
  --reason "the draft run settled and was read" >/dev/null

render() { # render <text file> <png>
  local svg="${2%.png}.svg"
  freeze "$1" --language ansi --font.file screenshots/fonts/JetBrainsMono-Regular.ttf \
    --font.family "JetBrains Mono" --font.size 14 --window --background "#0d1117" \
    --padding 20,30 --margin 0 --border.radius 8 --width 1054 --wrap 118 -o "$svg" </dev/null >/dev/null
  rasterize "$svg" "$2"; rm -f "$svg"
}
rasterize() { # rasterize <svg> <png>
  local w h
  w=$(grep -oE '<svg width="[0-9.]+"' "$1" | grep -oE '[0-9]+' | head -1)
  h=$(grep -oE '<svg width="[0-9.]+" height="[0-9.]+"' "$1" | grep -oE 'height="[0-9]+' | grep -oE '[0-9]+')
  "$chrome" --headless --no-sandbox --hide-scrollbars --force-device-scale-factor=2 \
    --default-background-color=00000000 --window-size="$w,$h" --screenshot="$2" "file://$1" >/dev/null 2>&1
}
shot() { # shot <before|after> <name> <command...>: "$ cmd", both streams, exit status
  local side="$1" name="$2"; shift 2
  local txt="$work/$side-$name.txt" code=0 said
  said=$(PATH="$work/$side-bin:$PATH" "$@" 2>&1) || code=$?
  { echo "\$ $*"; [ -n "$said" ] && echo "$said"; echo "[exit status $code]"; } > "$txt"
  render "$txt" "$out/$side-$name.png"
}

for side in before after; do
  shot "$side" stop-guard onepipeline stop-guard --format neutral --session "$session" --wake-budget 2100
  shot "$side" unwatched onepipeline unwatched --session "$session"
done

# Watching: before is the gallery's `watch` scene (`watch tracked-release
# --tick-interval 5 --timeout none --until node=docs`); after is the mock's
# `watch --flow plan-stop-guard --tick-interval 5 --timeout none` (the manager's
# ruling: same verb and flags, the run argument replaced by `--flow`, no
# `--until node=` because a flow has no node),
# rendered the way capture.py renders that scene (its human stderr lines only).
want=$(python3 -c 'import json;print([s["hash"] for s in json.load(open("shots/baseline/x86_64.json"))["shots"] if s["name"]=="watch"][0])')
git fetch -q origin gh-pages
got=$(git show FETCH_HEAD:x86_64/captures.json | python3 -c 'import json,sys;print([s["hash"] for s in json.load(sys.stdin)["shots"] if s["name"]=="watch"][0])')
if [ "$want" = "$got" ]; then
  echo "capture: watch before = gallery capture (gh-pages $(git rev-parse --short FETCH_HEAD), digest $got)" >&2
  git show FETCH_HEAD:x86_64/watch.svg > "$work/before-watch.svg"
else
  echo "capture: gallery watch is stale ($got != $want); capturing on this (base) tree" >&2
  SHOTS_OUT="$work/shots" screenshots/capture.sh >&2
  cp "$work/shots/watch.svg" "$work/before-watch.svg"
fi
rasterize "$work/before-watch.svg" "$out/before-watch.png"
PATH="$work/after-bin:$PATH" onepipeline watch --flow plan-stop-guard --tick-interval 5 --timeout none 2> "$work/after-watch.txt" >/dev/null || true
render "$work/after-watch.txt" "$out/after-watch.png"

ls -1 "$out"/*.png
echo "capture: wrote six screenshots to $out" >&2
