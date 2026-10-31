#!/usr/bin/env bash
# The end-user install proofs ci.yml's test jobs run, each a crate Nx target so
# it runs beside the test tiers in the job's one Nx invocation (scripts/ci-job.sh)
# and builds into the job's one target directory:
#
#   install-proof.sh path        `cargo install --path .` of this revision
#                                (`onepipeline:install-smoke`)
#   install-proof.sh documented  README.md's `cargo install --git` one-liner, run
#                                as given (`onepipeline:install-documented`)
#   install-proof.sh wheel       the x86_64 Linux wheel, built by the script the
#                                release builds with (`onepipeline:wheel-smoke`)
#
# Each installs into a root under `target/` rather than the user's own, puts that
# root first on PATH and runs `scripts/smoke-published.sh` over what it installed:
# the same assertions every release runs over every artifact it publishes.
#
# Every build here denies warnings, as every build the gate makes does, so the
# release-profile dependencies the stop-verdict tier compiles
# (`just _stop-verdict-build`) and the two installs are one set of units in one
# target directory rather than three. A warning an end user's build would only
# print is already a failure of `just lint`, so this fails nothing an install
# would pass that the gate does not fail first.
#
# Exit 0: installed and smoke-tested. 1: the install or its smoke failed. 2: a
# refused call.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT" || {
  echo "install-proof: cannot enter the repository root $ROOT" >&2
  echo "ACTION: run this from a checkout whose directories are readable" >&2
  exit 1
}

# A path cargo reads, which on Windows is cargo's own spelling of it: Git Bash
# rewrites a POSIX path it passes as an argument but not one it passes through
# the environment, and native cargo reads `/d/a/...` as `\d\a\...` on the
# current drive. Everywhere else the path is already cargo's.
native() {
  if command -v cygpath >/dev/null 2>&1; then cygpath -m "$1"; else printf '%s' "$1"; fi
}

export RUSTFLAGS="-D warnings"
CARGO_TARGET_DIR="$(native "$ROOT/target")"
export CARGO_TARGET_DIR
platform="$(uname -s)/$(uname -m)"

# The root an install writes into, emptied first so the smoke can only find what
# this run installed.
fresh_root() {
  local root="$ROOT/target/install-proof/$1"
  rm -rf -- "$root"
  mkdir -p "$root"
  printf '%s' "$root"
}

[ "$#" -eq 1 ] || {
  echo "install-proof: takes exactly one argument, the proof, not $#" >&2
  echo "ACTION: run 'scripts/install-proof.sh path|documented|wheel'" >&2
  exit 2
}

# llmlint: ignore-block[changed_behavior_has_e2e] each proof's work is a release build, a Docker image or the network: the test
# jobs run all three for real on every pull request that reaches the crate, which
# is their end-to-end proof, and an offline journey would either compile the release
# binary each run or stand in for cargo and Docker and prove a fake. What can be
# proven without them, the refusals, is in tests/ci_jobs/proofs.test.mjs.
case "${1:-}" in
  path)
    root="$(fresh_root path)"
    # On a pull request the change is not on the registry yet, so `--path .` is
    # the only way to install *this* revision; it is the code path of the
    # documented `cargo install onepipeline`, less the registry fetch.
    cargo install --path . --locked --quiet --root "$(native "$root")"
    PATH="$root/bin:$PATH" bash scripts/smoke-published.sh --label "cargo install on $platform"
    # The `host-view-time` budget times the binary users run, and this is the
    # build of it from this revision, so it is measured against that very binary
    # and adds no build of its own. Linux only, where the fixture's live
    # dispatches prove their procfs start tokens.
    if [ "$(uname -s)" = Linux ]; then
      just host-view-budget "$root/bin/onepipeline" "ci-job install-smoke ($platform): cargo install --path . --locked"
    fi
    ;;
  documented)
    # The README is the one source of the command: this proves the command users
    # are told to type, read out of README.md, never a copy of it.
    documented="$(grep -m1 '^cargo install --git ' README.md || true)"
    if [ -z "$documented" ]; then
      echo "install-proof: README.md's Install section no longer gives a 'cargo install --git' one-liner, so there is nothing to prove" >&2
      echo "ACTION: restore the one-liner in README.md, or drop the install-documented target" >&2
      exit 1
    fi
    # README.md is committed, but it is still file-derived input: accept only a
    # plain argv of unambiguous characters, then run it as an argv. Nothing in
    # that file is ever handed to the shell to interpret.
    if [[ $documented =~ [^A-Za-z0-9._:/@=+[:space:]-] ]]; then
      echo "install-proof: README.md's install one-liner carries characters this will not run: $documented" >&2
      echo "ACTION: make the README's one-liner a plain argv" >&2
      exit 1
    fi
    read -r -a argv <<<"$documented"
    root="$(fresh_root documented)"
    echo "install-proof: running README.md's command: ${argv[*]}"
    # The root is cargo's environment rather than an added argument, so the argv
    # run is exactly the README's.
    CARGO_INSTALL_ROOT="$(native "$root")" "${argv[@]}"
    # A `--git` install with no revision resolves the remote's default branch, so
    # the smoke is the installed revision's own copy of the assertions rather
    # than this checkout's: they assert the binary's whole command list, which a
    # branch adding a verb would fail on a binary that cannot have it yet. The
    # revision is the one cargo recorded installing, not the branch read again.
    installed="$(sed -n 's/^"onepipeline [^ ]* (git+[^#]*#\([0-9a-f]\{40\}\))" = .*/\1/p' "$root/.crates.toml")"
    [ -n "$installed" ] || {
      echo "install-proof: cargo recorded no git revision for onepipeline in $root/.crates.toml" >&2
      echo "ACTION: check that README.md's one-liner installs onepipeline from a git URL" >&2
      exit 1
    }
    smoke="$root/smoke-published.sh"
    git fetch --quiet --depth=1 origin "$installed" \
      && git show "$installed:scripts/smoke-published.sh" >"$smoke" || {
      echo "install-proof: could not read scripts/smoke-published.sh at the installed revision $installed from origin (git's reason is above)" >&2
      echo "ACTION: check that origin is the repository README.md's one-liner installs from and that it serves $installed, then re-run" >&2
      exit 1
    }
    PATH="$root/bin:$PATH" bash "$smoke" --label "cargo install --git on $platform"
    ;;
  wheel)
    [ "$(uname -s)" = Linux ] || {
      echo "install-proof: the wheel proof builds the x86_64 Linux wheel, which needs a Linux host" >&2
      echo "ACTION: run it on Linux (ci.yml's test-linux job does)" >&2
      exit 2
    }
    root="$(fresh_root wheel)"
    # `just wheel-linux` is the script release.yml's build-linux job builds with,
    # for the same target, so the packaging is proven on every pull request
    # rather than first at a release. It builds in that image, into its own
    # target directory, and takes its output relative to the checkout.
    just wheel-linux x86_64-unknown-linux-gnu "${root#"$ROOT"/}/dist"
    python3 -m venv "$root/venv"
    "$root/venv/bin/python" -m pip install --quiet --no-index --find-links "$root/dist" onepipeline-cli
    PATH="$root/venv/bin:$PATH" bash scripts/smoke-published.sh --label "the wheel built from this revision"
    ;;
  *)
    echo "install-proof: name the proof: path, documented or wheel" >&2
    echo "ACTION: run 'scripts/install-proof.sh path|documented|wheel'" >&2
    exit 2
    ;;
esac
# llmlint: ignore-end[changed_behavior_has_e2e]
