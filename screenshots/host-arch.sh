#!/usr/bin/env bash
# Print this host's screencomp capture lane: the normalised CPU architecture that
# names `shots/current/<arch>/` and `shots/baseline/<arch>.json`.
#
# One place, so the three consumers cannot disagree about what a lane is called:
# the capture, the local pre-push guard, and `just screenshots-bless`. The names
# match `[capture].arches` in screencomp.toml, which is what screencomp fans CI
# out over.
set -euo pipefail

arch="$(uname -m)"
case "$arch" in
x86_64 | amd64) arch="x86_64" ;;
arm64 | aarch64) arch="arm64" ;;
esac
printf '%s\n' "$arch"
