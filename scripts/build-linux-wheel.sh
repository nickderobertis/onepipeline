#!/usr/bin/env bash
# Build the `onepipeline-cli` wheel for one Linux target inside the manylinux
# image the release publishes it from.
#
# This is the one definition of a Linux wheel build: release.yml's Linux
# `build-wheels` legs and ci.yml's `wheel` legs both run it through `just
# wheel-linux`, so a pull request builds under the image, prerequisites and
# maturin invocation a release does.
#
# The prerequisites: `openssl-sys` is built with its vendored feature (the linked
# onetaskgraph stores reach `reqwest` with `native-tls-vendored`), which compiles
# OpenSSL from source, and OpenSSL 3's `Configure` loads Perl modules the
# image's CentOS 7 Perl leaves out.
#
# The build runs as root in the container, because installing those modules
# does; what it writes into the checkout is handed back to the invoking user on
# the way out, pass or fail. A target whose architecture is not the host's runs
# under whatever emulation Docker has — the release's legs are native.
#
# Usage:
#   build-linux-wheel.sh --target TRIPLE [--out DIR]
#
# DIR is relative to the repository root, which is what the container mounts,
# and must resolve inside it. Exit 0: the wheel is in DIR. Exit 2: the arguments
# were refused. Exit 1: the build or its environment failed, named on stderr.
set -euo pipefail

usage="run 'build-linux-wheel.sh --target x86_64-unknown-linux-gnu|aarch64-unknown-linux-gnu [--out DIR]'"

fail_usage() {
  echo "$1" >&2
  echo "ACTION: $usage" >&2
  exit 2
}

fail() {
  echo "$1" >&2
  echo "ACTION: $2" >&2
  exit 1
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

# The Linux targets release.yml's `build-wheels` matrix builds;
# tests/linux_wheel.rs fails when the two sets differ.
case "$target" in
  x86_64-unknown-linux-gnu) arch=x86_64 platform=linux/amd64 ;;
  aarch64-unknown-linux-gnu) arch=aarch64 platform=linux/arm64 ;;
  "") fail_usage "--target is required" ;;
  *) fail_usage "no manylinux build is defined for $target" ;;
esac

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd -P)" \
  || fail "cannot resolve the repository root from ${BASH_SOURCE[0]}" \
    "run this from a checkout whose directories are readable"
# Resolved through any symlink, because the container sees only the checkout: a
# path that leaves it would be written somewhere the host never looks.
case "$out" in
  /*) fail_usage "--out $out is absolute; name a directory relative to the repository root" ;;
esac
out_abs="$(realpath -m -- "$root/$out")" \
  || fail_usage "--out $out does not resolve to a path under $root"
case "$out_abs" in
  "$root"/?*) ;;
  *) fail_usage "--out $out resolves to $out_abs, outside the repository at $root" ;;
esac
out_rel="${out_abs#"$root"/}"

command -v docker >/dev/null \
  || fail "docker not found: the wheel is built inside the release's manylinux image" \
    "install Docker, then re-run"

# Exactly one exact release, which is how rust-toolchain.toml pins it; anything
# else would reach rustup as whatever the line happened to hold.
channel="$(sed -n 's/^channel = "\(.*\)"$/\1/p' "$root/rust-toolchain.toml")"
[[ "$channel" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] \
  || fail "rust-toolchain.toml's channel is '$channel', not one exact release" \
    "restore its single [toolchain] channel = \"<major>.<minor>.<patch>\" line"

# The image maturin-action's `manylinux: auto` selected for these targets on the
# release's native runners, and the maturin every release leg pins —
# tests/linux_wheel.rs holds this to release.yml's `maturin-version`.
image="quay.io/pypa/manylinux2014_$arch"
maturin_version="1.14.1"

# Both are created here rather than in the container, which would create any
# missing parent as root.
cargo_target="target/wheel-$target"
mkdir -p -- "$out_abs" "$root/$cargo_target" \
  || fail "could not create $out_abs and $root/$cargo_target" \
    "make both writable by $(id -un), or pass another --out"

# Runs in the container, from the checkout's root, and names nothing outside
# the image's PATH and the checkout. The EXIT trap hands the build's files back
# whatever the build did, and a hand-back that fails fails the run: a checkout
# left with root-owned files breaks the next build on this host.
read -r -d '' inside <<'SCRIPT' || true
step="" action=""
at() { step="$1" action="$2"; }
give_back() {
  status=$?
  if [ "$status" -ne 0 ]; then
    echo "the build stopped $step" >&2
    echo "ACTION: $action" >&2
  fi
  if ! chown -R "$HOST_OWNER" "$CARGO_TARGET_DIR" "$OUT"; then
    echo "could not return $CARGO_TARGET_DIR and $OUT to $HOST_OWNER" >&2
    echo "ACTION: chown -R $HOST_OWNER them from a root container, as this build does, before building here again" >&2
    [ "$status" -ne 0 ] || status=1
  fi
  exit "$status"
}
trap give_back EXIT
at "installing OpenSSL's Perl prerequisites with yum" \
  "check that the image's yum repositories answer, then re-run"
yum install -y -q perl-IPC-Cmd perl-Time-Piece
at "installing rustup" "check that https://sh.rustup.rs answers, then re-run"
curl --proto "=https" --tlsv1.2 -sSf https://sh.rustup.rs \
  | sh -s -- -y -q --profile minimal --default-toolchain none
export PATH="$HOME/.cargo/bin:$PATH" RUSTUP_TOOLCHAIN="$CHANNEL"
at "installing Rust $CHANNEL for $TARGET" \
  "check that $CHANNEL, rust-toolchain.toml's channel, is a published release, then re-run"
rustup toolchain install "$CHANNEL" --profile minimal --target "$TARGET"
at "installing maturin $MATURIN_VERSION" "check that PyPI serves maturin $MATURIN_VERSION, then re-run"
python3.12 -m pip install -q "maturin==$MATURIN_VERSION"
at "compiling the wheel" \
  "fix the compile error above; 'just wheel-linux $TARGET' reproduces it in this image"
python3.12 -m maturin build --release --locked \
  --target "$TARGET" --compatibility manylinux2014 --out "$OUT"
SCRIPT

# The checkout is read-only in the container; only the two directories the
# build writes are mounted writable over it.
docker run --rm --platform "$platform" \
  -v "$root":/io:ro -v "$root/$cargo_target":"/io/$cargo_target" -v "$out_abs":"/io/$out_rel" -w /io \
  -e TARGET="$target" -e OUT="$out_rel" -e CHANNEL="$channel" -e MATURIN_VERSION="$maturin_version" \
  -e HOST_OWNER="$(id -u):$(id -g)" \
  -e CARGO_TARGET_DIR="$cargo_target" \
  -e RUSTFLAGS="-D warnings" \
  "$image" bash -euo pipefail -c "$inside" \
  || fail "the $target wheel did not build in $image" \
    "follow the ACTION above; with none, Docker could not start $image — check that it can pull from quay.io"

echo "built the $target wheel into $out_rel/"
