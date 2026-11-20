#!/bin/sh
# The linker `.cargo/config.toml` names for Linux GNU targets: `cc`, told to link
# a dev or test build's output with the toolchain's own rust-lld, and handed
# every other link — release, or a toolchain shipping no rust-lld — unchanged.
#
# The profile is read off the output path because nothing else rustc hands a
# linker names it: cargo writes the dev and test profiles' crates, build scripts
# and examples under `<target>/[<triple>/]debug/`, and every other profile under
# its own name. rustc puts `<sysroot>/lib/rustlib/<host>/bin` on the linker's
# PATH, and its `gcc-ld/ld.lld` runs the `rust-lld` beside it, which is what
# rustc's own `-Clink-self-contained=+linker` selects on nightly.

out=
previous=
for arg in "$@"; do
  [ "$previous" = -o ] && out=$arg
  previous=$arg
done

case "$out" in
  */debug/deps/* | */debug/build/* | */debug/examples/*) ;;
  *) exec cc "$@" ;;
esac

IFS=:
for dir in $PATH; do
  if [ -x "$dir/rust-lld" ] && [ -x "$dir/gcc-ld/ld.lld" ]; then
    unset IFS
    exec cc -fuse-ld=lld "-B$dir/gcc-ld" "$@"
  fi
done
unset IFS
exec cc "$@"
