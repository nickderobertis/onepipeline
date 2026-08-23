#!/usr/bin/env bash
# One source for the environment this repository's judged tier runs under.
#
# Sourced by both ends of that tier — `scripts/llmlint-judge.sh`, which judges,
# and `scripts/llmlint-fingerprint.sh`, which keys Nx's cache on the judge
# configuration. The sharing is the point: if the two resolved different `llmlint`
# binaries, the key would describe a judge configuration the run never used, and a
# recorded verdict would replay for a diff it was never computed against.
#
# `scripts/setup-llmlint.sh` installs llmlint with `uv tool` into `$HOME/.local/bin`
# and prepends that directory for the rest of the session, so prepending it here is
# what a session that ran setup already has. The inherited PATH is kept behind it
# rather than replaced: a contributor who installed llmlint elsewhere is still
# judged, and both ends still agree, because both take this same order.
#
# llmlint: ignore-file[boundary_inputs_validated] The inherited PATH is the only
# value read here, and it is deliberately kept rather than validated or narrowed:
# an opinion about it here would let the judge and the fingerprint resolve
# different binaries, which is the split key this helper exists to prevent.
set -euo pipefail

llmlint_runtime_env() {
  # Where `scripts/setup-llmlint.sh` installs the toolchain. The two files name one
  # directory, and `npm/test/llmlint-cache.test.mjs` holds them to that.
  [ -n "${HOME:-}" ] || {
    echo "llmlint runtime env: HOME is not set, so the judged tier cannot find the toolchain 'just setup-llmlint' installs; set HOME to the account that ran it and retry" >&2
    return 1
  }
  export PATH="$HOME/.local/bin${PATH:+:$PATH}"
}
