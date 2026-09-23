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

# An architecture this mapping does not know is refused rather than passed
# through: it would name a lane no baseline exists for, and both callers would
# then compare a fresh capture against nothing.
case "$(uname -m)" in
x86_64 | amd64) printf 'x86_64\n' ;;
arm64 | aarch64) printf 'arm64\n' ;;
*)
  echo "host-arch: this host reports '$(uname -m)', which is not an architecture" >&2
  echo "           screencomp captures are lanes for. Add the mapping here and a" >&2
  echo "           lane in [capture].arches of screencomp.toml, then capture a" >&2
  echo "           baseline for it with 'just screenshots-bless'." >&2
  exit 1
  ;;
esac
