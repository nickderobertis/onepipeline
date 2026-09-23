#!/usr/bin/env bash
# Print this host's screencomp capture lane: the normalised CPU architecture
# that names `shots/current/<arch>/` and `shots/baseline/<arch>.json`.
#
# One place, so the three consumers can never disagree about what a lane is
# called: the capture (scripts/screenshots.py), the local pre-push guard
# (.githooks/pre-push), and `just screenshots-bless`. The names match
# `[capture].arches` in screencomp.toml, which is what screencomp fans CI out
# over.
#
# llmlint: ignore-file[new_code_lands_in_a_project] the visual-docs capture is
# informational machinery outside every Nx target the gate reaches, and outside
# the crate on purpose (the 95% coverage floor is measured over the crate).
set -euo pipefail

arch="$(uname -m)"
case "$arch" in
x86_64 | amd64) arch="x86_64" ;;
arm64 | aarch64) arch="arm64" ;;
esac
printf '%s\n' "$arch"
