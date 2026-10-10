#!/usr/bin/env bash
# THROWAWAY spike for the host-view-speed design document — not a test, not part
# of `just screenshots`, and nothing it produces is committed. Re-captures the
# before/after pictures of `onepipeline host` into the directory it is given:
#
#   screenshots/spike-host-view/capture.sh OUT_DIR [ONEPIPELINE_BIN]
#
# "Before" is the binary built from this tree (the unchanged base). "After" is
# `after.py`, a mock of the proposed renderer fed from the same hand-made root.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo="$(cd "$here/../.." && pwd)"
out="$(mkdir -p "$1" && cd "$1" && pwd)"
bin="${2:-}"
if [ -z "$bin" ]; then
  cargo build --locked --bin onepipeline --manifest-path "$repo/Cargo.toml" >&2
  bin="$repo/target/debug/onepipeline"
fi

export HOSTNAME=devbox-01
# A directory only this invocation owns, inside the dispatch's scratch dir.
scratch="$(mktemp -d "${ONEPIPELINE_NODE_SCRATCH_DIR:?set ONEPIPELINE_NODE_SCRATCH_DIR to a writable directory}/spike-host-view.XXXXXX")"
root="$scratch/runs"
export ONEPIPELINE_RUNS_DIR="$root"

# One live dispatch: a process this script starts, registered as the executor does.
sleep 600 & live=$!
trap 'kill "$live" 2>/dev/null || true; rm -rf "$scratch"' EXIT
python3 "$here/fixture.py" "$root" "$live"

"$bin" host > "$out/before-text.txt" 2>&1 || true
"$bin" host --json > "$out/before-json.txt" 2>&1 || true
python3 "$here/after.py" text "$out/before-text.txt" > "$out/after-text.txt"
python3 "$here/after.py" json "$out/before-text.txt" > "$out/after-json.txt"

render() { # $1 = prompt line, $2 = captured output, $3 = image
  { printf '\033[32m$\033[0m %s\n' "$1"; cat "$2"; } > "$3.ansi"
  freeze "$3.ansi" --language ansi \
    --font.file "$repo/screenshots/fonts/JetBrainsMono-Regular.ttf" \
    --font.family "JetBrains Mono" --font.size 14 --window --background '#0d1117' \
    --padding 20,30 --margin 0 --border.radius 8 --width 1054 --wrap 118 \
    -o "$3" >/dev/null </dev/null
  rm "$3.ansi"
}
render "onepipeline host" "$out/before-text.txt" "$out/1-before-text-view.png"
render "onepipeline host" "$out/after-text.txt" "$out/1-after-text-view.png"
render "onepipeline host --json" "$out/before-json.txt" "$out/2-before-json.png"
render "onepipeline host --json | jq ." "$out/after-json.txt" "$out/2-after-json.png"
ls "$out"
