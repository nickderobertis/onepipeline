#!/usr/bin/env bash
# The one place a lane name is derived from `uname -m`, so the capture, the
# pre-push guard and `just screenshots-bless` cannot disagree about which
# `shots/baseline/<arch>.json` this host owns. The spellings match
# `[capture].arches` in screencomp.toml, which is what CI fans out over.

# llmlint: ignore-file[changed_behavior_has_e2e] the capture's every step is a
# third-party tool `just check` does not install (`freeze`, `screencomp`), so a journey
# could only assert against stubs of those two. What it produces is checked instead: the
# visual-docs workflow re-derives the committed baseline on every pull request.
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
