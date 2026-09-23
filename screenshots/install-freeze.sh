#!/usr/bin/env bash
# Install the pinned `freeze` from its prebuilt release. What CI runs;
# `just screenshots-tools` installs the same version through Go instead, for a
# developer who has one.
#
# **The version is read out of the justfile's `freeze-version` line**, which is
# the one place this repository names it. A second pin here is exactly the drift
# this script exists not to have: the renderer's version decides the bytes of
# every shot, so a CI capture taken with a different one fails the committed
# baseline for a reason no diff shows.
#
# The asset has to match the runner's own architecture, which is why this is a
# script rather than a hard-coded URL in a workflow: screencomp fans one capture
# job out per lane in `[capture].arches`, and an `arm64` lane's runner needs an
# `arm64` binary.
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
version="$(sed -n 's/^freeze-version *:= *"\([^"]*\)".*/\1/p' "$repo_root/justfile")"
if [ -z "$version" ]; then
  echo "install-freeze: the justfile no longer carries a 'freeze-version' line," >&2
  echo "                which is the only place this repository pins the renderer." >&2
  echo "                Restore it there (and nowhere else), then re-run." >&2
  exit 1
fi

case "$(uname -m)" in
x86_64 | amd64) asset="x86_64" ;;
arm64 | aarch64) asset="arm64" ;;
*)
  echo "install-freeze: freeze publishes no prebuilt binary for $(uname -m)." >&2
  echo "                Install it from source instead — 'just screenshots-tools'," >&2
  echo "                which needs Go — or capture on a lane declared in" >&2
  echo "                [capture].arches of screencomp.toml." >&2
  exit 1
  ;;
esac

into="${1:-/usr/local/bin}"
if [ ! -d "$into" ] || [ ! -w "$into" ]; then
  echo "install-freeze: $into is not a writable directory to install into." >&2
  echo "                Pass one as the first argument, e.g." >&2
  echo "                'bash screenshots/install-freeze.sh \"$HOME/.local/bin\"'," >&2
  echo "                and make sure it is on PATH." >&2
  exit 1
fi

url="https://github.com/charmbracelet/freeze/releases/download/v${version}/freeze_${version}_Linux_${asset}.tar.gz"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
curl -fsSL "$url" -o "$tmp/freeze.tar.gz"
tar -xzf "$tmp/freeze.tar.gz" -C "$tmp"
install -m 0755 "$(find "$tmp" -type f -name freeze -perm -u+x | head -1)" "$into/freeze"
freeze --version >&2
