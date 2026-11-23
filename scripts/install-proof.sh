#!/usr/bin/env bash
# The end-user install proofs ci.yml's jobs run, each an Nx target so it runs
# beside the test tiers in the job's one Nx invocation (scripts/ci-job.sh, which
# also says which job selects which):
#
#   install-proof.sh path        `cargo install --path .` of this revision
#                                (`onepipeline:install-smoke`)
#   install-proof.sh documented  README.md's `cargo install --git` one-liner, run
#                                as given (`onepipeline:install-documented`)
#   install-proof.sh wheel       the x86_64 Linux wheel, built by the script the
#                                release builds with (`onepipeline-packaging:wheel-smoke`)
#
# Each installs into a root under `target/` rather than the user's own, puts that
# root first on PATH and runs `scripts/smoke-published.sh` over what it installed:
# the same assertions every release runs over every artifact it publishes.
#
# `path` builds into the job's one target directory, and every build here denies
# warnings, as every build the gate makes does, so the release-profile dependencies
# the stop-verdict tier compiles (`just _stop-verdict-build`) and that install are
# one set of units rather than two. A warning an end user's build would only print
# is already a failure of `just lint`, so this fails nothing an install would pass
# that the gate does not fail first. `documented` builds another revision, so it
# builds in a directory of its own (below).
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

# A directory as PATH holds it, which on Windows is Git Bash's own spelling: a
# root this script reached as `E:/...` would otherwise split at its drive colon.
on_path() {
  if command -v cygpath >/dev/null 2>&1; then cygpath -u "$1"; else printf '%s' "$1"; fi
}

export RUSTFLAGS="-D warnings"
CARGO_TARGET_DIR="$(native "$ROOT/target")"
export CARGO_TARGET_DIR
platform="$(uname -s)/$(uname -m)"

# The root an install writes into, emptied first so the smoke can only find what
# this run installed.
installed_into() {
  [ -x "$1/bin/onepipeline" ] || [ -x "$1/bin/onepipeline.exe" ] || {
    echo "install-proof: cargo reported success but $1/bin holds no onepipeline; the root holds:" >&2
    ls -laR "$1" >&2 || true
    echo "ACTION: compare where cargo installed (its output above, without --quiet) with $1/bin" >&2
    exit 1
  }
}

# test-linux's `install-smoke` times `host-view-time`'s reads, which a release
# compile beside them starves (packaging/project.json's wheel-smoke records 192 ms
# against the 100 ms line). An Nx edge would order the documented install after
# it, but would also select `install-smoke` wherever the documented install runs
# alone, so `scripts/ci-job.sh` asks for the order instead, only in a job that runs
# both: with ONEPIPELINE_INSTALL_ORDER=documented-after-path, `documented` waits
# for the marker `path` leaves when it exits, its timing included.
PATH_FINISHED="$ROOT/target/install-proof/path.finished"
order="${ONEPIPELINE_INSTALL_ORDER:-}"
case "$order" in
  "" | documented-after-path) ;;
  *)
    echo "install-proof: ONEPIPELINE_INSTALL_ORDER='$order' is not an order this knows" >&2
    echo "ACTION: leave it unset, or set it to 'documented-after-path' as scripts/ci-job.sh does" >&2
    exit 2
    ;;
esac

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

# llmlint: ignore-block[changed_behavior_has_e2e] each proof's work is a release build, a Docker image or the network: CI's jobs
# run all three for real (scripts/ci-job.sh says when), which is their end-to-end
# proof, and an offline journey would either compile the release binary each run or
# stand in for cargo and Docker and prove a fake. What can be proven without them,
# the refusals and the directory each install builds in, is in
# tests/ci_jobs/proofs.test.mjs, the latter against a cargo that only records.
case "${1:-}" in
  path)
    # Left however this exits, so a documented install waiting on it never waits
    # past a failure.
    rm -f -- "$PATH_FINISHED"
    trap 'mkdir -p "${PATH_FINISHED%/*}" && touch "$PATH_FINISHED"' EXIT
    root="$(fresh_root path)"
    # On a pull request the change is not on the registry yet, so `--path .` is
    # the only way to install *this* revision; it is the code path of the
    # documented `cargo install onepipeline`, less the registry fetch.
    cargo install --path . --locked --root "$(native "$root")"
    installed_into "$root"
    PATH="$(on_path "$root/bin"):$PATH" bash scripts/smoke-published.sh --label "cargo install on $platform"
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
    if [ "$order" = documented-after-path ]; then
      echo "install-proof: waiting for install-smoke to finish, host-view-time's reads included"
      # Bounded, so a smoke that died without leaving its marker fails this proof
      # rather than holding the job to its timeout. The bound is not exercised by a
      # test, which would wait out the hour; the wait itself is (proofs.test.mjs).
      waited=0
      until [ -f "$PATH_FINISHED" ]; do
        if [ "$waited" -ge 3600 ]; then
          echo "install-proof: install-smoke left no $PATH_FINISHED within an hour, so the documented install did not start" >&2
          echo "ACTION: read onepipeline:install-smoke's output above for why it never finished, fix that, then re-run the job" >&2
          exit 1
        fi
        sleep 1
        waited=$((waited + 1))
      done
    fi
    root="$(fresh_root documented)"
    # The default branch's build in the job's target directory rewrote the release
    # units, and the target/release/onepipeline, that the job's other release builds
    # share (onepipeline#1814), so it builds under the runner's temporary directory,
    # which no cache saves, and that directory goes when this exits.
    temp="$(on_path "${RUNNER_TEMP:-${TMPDIR:-/tmp}}")"
    build="$(mktemp -d "$temp/install-documented.XXXXXX")" || {
      echo "install-proof: could not create the documented install's build directory under $temp (mktemp's reason is above)" >&2
      echo "ACTION: point RUNNER_TEMP or TMPDIR at a writable directory, then re-run" >&2
      exit 1
    }
    trap 'rm -rf -- "$build"' EXIT
    echo "install-proof: running README.md's command: ${argv[*]}"
    # The root and the build directory are cargo's environment rather than added
    # arguments, so the argv run is exactly the README's.
    CARGO_TARGET_DIR="$(native "$build")" CARGO_INSTALL_ROOT="$(native "$root")" "${argv[@]}"
    installed_into "$root"
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
    PATH="$(on_path "$root/bin"):$PATH" bash "$smoke" --label "cargo install --git on $platform"
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
    PATH="$(on_path "$root/venv/bin"):$PATH" bash scripts/smoke-published.sh --label "the wheel built from this revision"
    ;;
  *)
    echo "install-proof: name the proof: path, documented or wheel" >&2
    echo "ACTION: run 'scripts/install-proof.sh path|documented|wheel'" >&2
    exit 2
    ;;
esac
# llmlint: ignore-end[changed_behavior_has_e2e]
