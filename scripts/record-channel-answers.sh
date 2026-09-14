#!/usr/bin/env bash
# Capture what a published `onepipeline` answers over the recorded channel
# directories, into `tests/recorded/answers/`.
#
# `tests/e2e/recorded_channel.rs` holds this build to those answers. It captures
# them itself, running the same steps in the same world, when
# ONEPIPELINE_RECORD_CHANNEL_ANSWERS_WITH names the executable to capture with.
# This script finds that executable in the published `onepipeline-cli` wheel and
# runs those journeys and no others.
#
# Usage: scripts/record-channel-answers.sh [RELEASE]   (default 0.28.2)
set -euo pipefail

cd "$(dirname "$0")/.."

release="${1:-0.28.2}"
if ! program=$(uv tool run --from "onepipeline-cli==$release" sh -c 'command -v onepipeline'); then
    echo "record-channel-answers: onepipeline-cli $release could not be installed through uv; check the version exists on PyPI and uv can reach it" >&2
    exit 2
fi
reported=$("$program" --version)
if [ "$reported" != "onepipeline $release" ]; then
    echo "record-channel-answers: $program reports '$reported', not onepipeline $release; nothing was captured. Clear uv's cached copy with 'uv cache clean onepipeline-cli' and run this again, or pass the release that executable is as RELEASE" >&2
    exit 2
fi

ONEPIPELINE_RECORD_CHANNEL_ANSWERS_WITH="$program" \
    just test-e2e 'test(/^recorded_channel::/)'
echo "record-channel-answers: captured $reported's answers under tests/recorded/answers/"
