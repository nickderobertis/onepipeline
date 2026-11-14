#!/usr/bin/env bash
# Measurement only (spike-launch-inventory, never lands): a cargo target runner that
# records every process start under the test it runs, as the operating system sees
# it, for scripts/launch-inventory.mjs to check the launch trace against. Linux:
# `strace -f` of execve and the clones that start processes, one file per test
# process under $ONEPIPELINE_OS_TRACE_DIR. Inert (it execs the binary directly)
# when that variable is unset or the binary is not the e2e tier's.
set -euo pipefail
binary="$1"
case "$(basename "$binary")" in
  e2e-*) ;;
  *) exec "$@" ;;
esac
dir="${ONEPIPELINE_OS_TRACE_DIR:-}"
if [ -z "$dir" ] || ! command -v strace >/dev/null; then exec "$@"; fi
mkdir -p "$dir"
name="${NEXTEST_TEST_NAME:-untested}"
exec strace -f -qq --seccomp-bpf -e trace=execve,clone,clone3,fork,vfork -e signal=none -s 256 \
  -o "$dir/${name//::/__}.$$.strace" -- "$@"
