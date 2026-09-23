#!/usr/bin/env bash
# Run the pre-push visual guard the way git runs it, without pushing anything.
#
# The guard is the one caller of the capture that runs inside a **git hook**, and
# a hook's environment is not a shell's: git exports `GIT_DIR` and `GIT_WORK_TREE`
# naming the repository being pushed, and writes the refs on the hook's stdin. A
# defect that only appears under those is not reachable by running `just
# screenshots`, so it is found either here or by a rejected push.
#
# It invokes `.githooks/pre-push` itself rather than reimplementing it, so what
# is rehearsed is the hook that will run. It pushes nothing and writes nothing
# this repository does not already ignore.
#
# llmlint: ignore-file[changed_behavior_has_e2e] this script's whole body is the
# invocation of a git hook whose every step is a third-party tool this repository
# neither installs in `just check` nor ships — `screencomp` and `freeze` — over a capture
# that takes about a minute of real subprocess work. A journey over it could only assert
# against stubs of those two, which is a test of the stubs; the hook it runs carries the
# same directive for the same reason.
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root"

# The remote to rehearse against. It reaches `git remote get-url` as an argument
# and is interpolated into a remote-tracking ref, so it is held to a remote's
# name and to *this* clone's remotes before either — an option-shaped or
# path-shaped value would otherwise change what those commands mean.
remote="${1:-origin}"
if ! printf '%s\n' "$remote" | grep -qE '^[A-Za-z0-9][A-Za-z0-9._-]*$' \
   || ! git remote | grep -qxF "$remote"; then
  echo "rehearse-guard: '$remote' is not a remote of this clone. Name one of:" >&2
  git remote | sed 's/^/                /' >&2
  exit 1
fi
hook=".githooks/pre-push"
[ -x "$hook" ] || {
  echo "rehearse-guard: $hook is not executable, so git would not run it either." >&2
  echo "                Restore it, or 'chmod +x $hook'." >&2
  exit 1
}

# What git would name on the hook's stdin for `git push <remote>` of this branch.
# A detached HEAD has no branch to push, which is a state to report rather than
# to invent a ref for.
ref="$(git symbolic-ref -q HEAD || true)"
[ -n "$ref" ] || {
  echo "rehearse-guard: HEAD is detached, so there is no branch git would push." >&2
  echo "                Check out the branch you mean to rehearse and re-run." >&2
  exit 1
}
local_sha="$(git rev-parse HEAD)"
# The remote side's current tip, as git reports it to the hook: the tracked
# remote-tracking ref if this clone has one, and forty zeroes for a branch the
# remote does not carry yet — which is what a session branch's first push sends,
# and the case the guard resolves through `origin/HEAD`.
remote_sha="$(git rev-parse -q --verify "refs/remotes/${remote}/${ref#refs/heads/}" \
  || printf '0000000000000000000000000000000000000000')"
url="$(git remote get-url "$remote")"

# The environment git gives a hook. `GIT_DIR` is the point of the exercise: it
# beats `-C` and beats discovery, so anything the capture runs git for sees this
# repository unless the capture clears it.
#
# `set +e` around it because a blocked push is this script's *answer*, not its
# failure: under `set -e` the refusal would exit here and never be reported.
set +e
printf '%s %s %s %s\n' "$ref" "$local_sha" "$ref" "$remote_sha" \
  | env GIT_DIR="$(git rev-parse --absolute-git-dir)" \
        GIT_WORK_TREE="$repo_root" \
        GIT_PREFIX="" \
        bash "$hook" "$remote" "$url"
status=$?
set -e

# Quiet on success, like the capture it wraps: the hook has already said the
# shots are unchanged, and the exit status is the answer either way.
[ "$status" -eq 0 ] \
  || echo "rehearse-guard: the guard would BLOCK this push (exit $status)." >&2
exit "$status"
