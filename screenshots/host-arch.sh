#!/usr/bin/env bash
# The one place a lane name is derived from `uname -m`, so the capture, the
# pre-push guard and `just screenshots-bless` cannot disagree about which
# `shots/baseline/<arch>.json` this host owns. The spellings match
# `[capture].arches` in screencomp.toml, which is what CI fans out over.

# llmlint: ignore-file[changed_behavior_has_e2e] the capture is informational machinery
# whose every step is a third-party tool this repository deliberately does not install
# for `just check` — `freeze` renders the scenes and `screencomp` classifies them — so
# an offline journey of it could only assert against stubs of those two, which tests the
# stubs. What this machinery produces is checked instead, and more strictly than a
# journey would: the committed digest baseline is re-derived by
# `.github/workflows/visual-docs.yml` on every pull request, and a capture that stopped
# working, changed a byte, or lost a scene is a red check there.
set -euo pipefail

arch="$(uname -m)"
case "$arch" in
x86_64 | amd64) arch="x86_64" ;;
arm64 | aarch64) arch="arm64" ;;
esac
printf '%s\n' "$arch"
