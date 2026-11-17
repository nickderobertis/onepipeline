#!/usr/bin/env bash
# Measurement only (spike-launch-inventory, never lands): a cargo target runner that
# records every process start under the test it runs, as the operating system sees
# it, for scripts/launch-inventory.mjs to check the launch trace against. Linux:
# `strace -f` of execve and the clones that start processes, one file per test
# process under $ONEPIPELINE_OS_TRACE_DIR. It execs the binary directly when that
# variable is unset or the binary is not the e2e tier's, and refuses when tracing is
# asked for and strace is missing, rather than running the test untraced.
set -euo pipefail
[ "$#" -ge 1 ] || {
  echo "launch-os-trace: name the binary to run, as cargo's target runner does: launch-os-trace.sh BINARY [ARG...]" >&2
  exit 2
}
binary="$1"
case "$(basename "$binary")" in
  e2e-*) ;;
  *) exec "$@" ;;
esac
dir="${ONEPIPELINE_OS_TRACE_DIR:-}"
[ -n "$dir" ] || exec "$@"
command -v strace >/dev/null || {
  echo "launch-os-trace: ONEPIPELINE_OS_TRACE_DIR asks for an strace record, and strace is not installed: sudo apt-get install -y strace, or unset ONEPIPELINE_OS_TRACE_DIR" >&2
  exit 2
}
mkdir -p "$dir" || {
  echo "launch-os-trace: cannot create $dir for the strace record: point ONEPIPELINE_OS_TRACE_DIR at a writable directory" >&2
  exit 2
}
name="${NEXTEST_TEST_NAME:-untested}"
# A test's path is identifiers joined by `::`; anything else names no file under $dir.
[[ "$name" =~ ^[A-Za-z0-9_]+(::[A-Za-z0-9_]+)*$ ]] || name=untested
# llmlint: ignore[tool_output_is_signal] This measurement-only runner deliberately execs strace so the observed process tree and signal/exit behaviour are strace's own; a wrapper adding its own failure message changes that measured boundary. strace's stderr is forwarded unchanged, and the setup refusals above already name missing-tool and unwritable-directory repairs. This spike never lands (manager ruling a1).
exec strace -f -qq --seccomp-bpf -e trace=execve,clone,clone3,fork,vfork -e signal=none -s 256 \
  -o "$dir/${name//::/__}.$$.strace" -- "$@"
