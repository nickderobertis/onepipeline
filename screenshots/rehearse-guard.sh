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

remote="${1:-origin}"
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
url="$(git remote get-url "$remote" 2>/dev/null || echo "$remote")"

echo "rehearse-guard: running $hook as git would, for $ref -> $remote" >&2
echo "rehearse-guard: ${local_sha} over ${remote_sha}" >&2

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

if [ "$status" -eq 0 ]; then
  echo "rehearse-guard: the guard would let this push through." >&2
else
  echo "rehearse-guard: the guard would BLOCK this push (exit $status)." >&2
fi
exit "$status"
