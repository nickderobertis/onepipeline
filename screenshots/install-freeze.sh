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

# llmlint: ignore-file[changed_behavior_has_e2e] the capture's every step is a
# third-party tool `just check` does not install (`freeze`, `screencomp`), so a journey
# could only assert against stubs of those two. What it produces is checked instead: the
# visual-docs workflow re-derives the committed baseline on every pull request.
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
version="$(sed -n 's/^freeze-version *:= *"\([^"]*\)".*/\1/p' "$repo_root/justfile")"
# A release number, and nothing else: this value is interpolated into a URL and
# into the archive's filename, so a line that had grown a comment, a range or a
# path would reach the network as part of one.
if ! printf '%s' "$version" | grep -Eq '^[0-9]+\.[0-9]+\.[0-9]+$'; then
  echo "install-freeze: the justfile's 'freeze-version' line reads '$version'," >&2
  echo "                which is not a MAJOR.MINOR.PATCH release. That line is the" >&2
  echo "                only place this repository pins the renderer — set it to a" >&2
  echo "                published freeze release and re-run." >&2
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

release="https://github.com/charmbracelet/freeze/releases/download/v${version}"
archive="freeze_${version}_Linux_${asset}.tar.gz"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

# The archive and the checksums the release publishes beside it. What is about
# to be extracted and then **executed** comes off the network, so it is checked
# against the release's own digest before anything unpacks it: a truncated
# download or a substituted asset otherwise becomes a renderer this repository
# hashes its whole visual baseline against.
if ! curl -fsSL "$release/$archive" -o "$tmp/$archive" ||
  ! curl -fsSL "$release/checksums.txt" -o "$tmp/checksums.txt"; then
  echo "install-freeze: could not download freeze $version or its checksums from" >&2
  echo "                $release. Check the release exists and that this machine" >&2
  echo "                can reach github.com; on a host with Go and no egress to" >&2
  echo "                releases, 'just screenshots-tools' builds it instead." >&2
  exit 1
fi
# The exact filename field, never a substring of it: the release also publishes
# `<archive>.sbom.json`, whose line a substring match picks up too, and
# `sha256sum --check` then fails on a file this script never downloaded.
awk -v want="$archive" '$2 == want || $2 == "*" want' \
  "$tmp/checksums.txt" >"$tmp/expected.txt"
if [ ! -s "$tmp/expected.txt" ]; then
  echo "install-freeze: the freeze $version release publishes no checksum for" >&2
  echo "                $archive, so the download cannot be verified. Check the" >&2
  echo "                release at $release and correct 'freeze-version' in the" >&2
  echo "                justfile." >&2
  exit 1
fi
if ! (cd "$tmp" && sha256sum --check --status expected.txt); then
  echo "install-freeze: the downloaded $archive does not match the checksum the" >&2
  echo "                freeze release publishes for it. Re-run; if it fails" >&2
  echo "                again, do not install it — the asset or the transfer is" >&2
  echo "                wrong, and this binary renders every committed shot." >&2
  exit 1
fi

if ! tar -xzf "$tmp/$archive" -C "$tmp"; then
  echo "install-freeze: $archive verified against its checksum but did not" >&2
  echo "                unpack (tar's message is above), so the release's" >&2
  echo "                archive format has changed. Check the release at" >&2
  echo "                $release and update this script." >&2
  exit 1
fi
binary="$(find "$tmp" -type f -name freeze -perm -u+x | head -1)"
if [ -z "$binary" ]; then
  echo "install-freeze: $archive carries no 'freeze' executable, so the release's" >&2
  echo "                layout has changed. Check the release at $release and" >&2
  echo "                update this script." >&2
  exit 1
fi
if ! install -m 0755 "$binary" "$into/freeze"; then
  echo "install-freeze: could not install the renderer into $into (the message" >&2
  echo "                above is the system's). Pass a directory you can write" >&2
  echo "                as the first argument and make sure it is on PATH." >&2
  exit 1
fi
if ! "$into/freeze" --version >&2; then
  echo "install-freeze: $into/freeze installed but will not run on this host," >&2
  echo "                so the capture has no renderer. Check that the asset" >&2
  echo "                matches this machine's architecture ($(uname -m)); if it" >&2
  echo "                does, build it from source with 'just screenshots-tools'." >&2
  exit 1
fi
