#!/usr/bin/env bash
# Build the `onepipeline-cli` wheel for one Linux target inside the manylinux
# image the release publishes it from.
#
# This is the one definition of a Linux wheel build: release.yml's Linux
# `build-wheels` legs and ci.yml's `wheel` check both run it through `just
# wheel-linux`, so a pull request builds under exactly the image, prerequisites
# and maturin invocation a release does. Before it, the check built on the bare
# runner, where the full Perl OpenSSL's `Configure` needs is present, while the
# release built in manylinux2014, where it is not — so v0.50.0 shipped no wheel
# while the required `wheel` check had passed.
#
# The prerequisites are that difference. `openssl-sys` is built with its vendored
# feature (the linked onetaskgraph stores reach `reqwest` with
# `native-tls-vendored`), which compiles OpenSSL from source, and OpenSSL 3's
# `Configure` loads Perl modules the image's CentOS 7 Perl leaves out.
#
# The build runs as root in the container, because installing those modules
# does; what it writes into the checkout is handed back to the invoking user on
# the way out, pass or fail. A target whose architecture is not the host's runs
# under whatever emulation Docker has — the release's legs are native.
#
# Usage:
#   build-linux-wheel.sh --target TRIPLE [--out DIR]
#
# DIR is relative to the repository root, which is what the container mounts.
set -euo pipefail

usage="run 'build-linux-wheel.sh --target x86_64-unknown-linux-gnu|aarch64-unknown-linux-gnu [--out DIR]'"

fail_usage() {
  echo "$1" >&2
  echo "ACTION: $usage" >&2
  exit 2
}

target=""
out="dist"
while [ $# -gt 0 ]; do
  case "$1" in
    --target | --out)
      [ $# -ge 2 ] || fail_usage "$1 needs a value"
      if [ "$1" = --target ]; then target="$2"; else out="$2"; fi
      shift 2
      ;;
    *) fail_usage "unknown argument: $1" ;;
  esac
done

case "$target" in
  x86_64-unknown-linux-gnu) arch=x86_64 platform=linux/amd64 ;;
  aarch64-unknown-linux-gnu) arch=aarch64 platform=linux/arm64 ;;
  "") fail_usage "--target is required" ;;
  *) fail_usage "no manylinux build is defined for $target" ;;
esac
case "$out" in
  /* | ../* | *'/../'* | ..) fail_usage "--out must be a directory inside the repository, relative to its root" ;;
esac

command -v docker >/dev/null || {
  echo "docker not found: the wheel is built inside the release's manylinux image" >&2
  echo "ACTION: install Docker, then re-run" >&2
  exit 1
}

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
channel="$(sed -n 's/^channel = "\(.*\)"$/\1/p' "$root/rust-toolchain.toml")"
[ -n "$channel" ] || {
  echo "no toolchain channel in rust-toolchain.toml" >&2
  exit 1
}

# The image maturin-action's `manylinux: auto` selected for these targets on the
# release's native runners, and the maturin the release pinned.
image="quay.io/pypa/manylinux2014_$arch"
maturin_version="1.14.1"

mkdir -p "$root/$out"

docker run --rm --platform "$platform" \
  -v "$root":/io -w /io \
  -e TARGET="$target" -e OUT="$out" -e CHANNEL="$channel" -e MATURIN_VERSION="$maturin_version" \
  -e HOST_OWNER="$(id -u):$(id -g)" \
  -e CARGO_TARGET_DIR="/io/target/wheel-$target" \
  -e CARGO_TERM_COLOR="${CARGO_TERM_COLOR:-auto}" \
  -e RUSTFLAGS="-D warnings" \
  "$image" bash -euo pipefail -c '
    trap "chown -R \"\$HOST_OWNER\" \"\$CARGO_TARGET_DIR\" \"/io/\$OUT\" 2>/dev/null || true" EXIT
    yum install -y -q perl-IPC-Cmd perl-Time-Piece
    curl --proto "=https" --tlsv1.2 -sSf https://sh.rustup.rs \
      | sh -s -- -y -q --profile minimal --default-toolchain none
    export PATH="$HOME/.cargo/bin:$PATH" RUSTUP_TOOLCHAIN="$CHANNEL"
    rustup toolchain install "$CHANNEL" --profile minimal --target "$TARGET"
    /opt/python/cp312-cp312/bin/python -m pip install -q "maturin==$MATURIN_VERSION"
    /opt/python/cp312-cp312/bin/maturin build --release --locked \
      --target "$TARGET" --compatibility manylinux2014 --out "$OUT"
  '
