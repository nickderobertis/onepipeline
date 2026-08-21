# The body of a repository's own `pre-push` hook, for the journeys that need the
# merge path to do something other than let a push through. Written into a
# world's own scratch by `harness::hook_script` and run by **git** — not by
# `onevcs`, which runs no verification tier of its own — at the push that
# publishes the change.
#
# git runs a hook at the top of the tree the push is made from, and which tree
# that is follows the publication policy: a `change-*` policy pushes the branch
# from the session's own worktree, and `local-direct` pushes a squash from a
# scratch worktree under the same run root. Both sit under a directory named
# after the session token, but at different depths — so the last two verbs find
# the stream this session is writing by walking up from here to the first
# ancestor the state root holds one for, rather than by counting directories.
#
# Verbs, and the exit codes they answer with:
#
#   wait-for PATH        block until PATH exists            0
#   break-streams        leave a file where the session     0
#                        stream directory was
#   append-future-event  append a line no build of          0
#                        `onevcs` can read
#   anything else        refuse, naming the verbs           64
#   a verb that could not do what it names                  1
#
# 64 is `sysexits.h`'s EX_USAGE, which is what the compiled program this
# replaced answered a command line it did not speak with — kept, because a hook
# that succeeded on a verb nobody implemented would let a journey assert against
# a merge path that never ran. 1 is separate from it for the same reason in the
# other direction: a write the filesystem refused is not a caller who typed the
# wrong thing, and a hook that reported either as success would leave a journey
# asserting on state nothing produced. git refuses the push on either, which is
# the merge path saying no.
set -u

fail() {
  echo "pre-push: $1" >&2
  echo "pre-push: the verbs are: wait-for PATH | break-streams | append-future-event" >&2
  exit 64
}

# The state root `onevcs` was given. Refused loudly rather than defaulted: unset,
# the two stream verbs below would reach for the operator's own `~/.onevcs`, and
# the journey asserting on this session's stream would be asserting on somebody
# else's. Checked in the caller's own shell rather than inside a substitution,
# because an `exit` inside one ends the subshell and nothing else.
require_home() {
  if [ -z "${ONEVCS_HOME-}" ]; then
    fail "ONEVCS_HOME is unset, so there is no session stream to reach; set it to the state root this world gave onevcs, the way World::cmd does"
  fi
}

# A verb that could not do what it names. Distinct from `fail`, which is the
# caller's mistake: this one is the host's, and neither is a push the merge path
# accepted.
broke() {
  echo "pre-push: $1" >&2
  echo "pre-push: the host refused the write, not the caller: check that ONEVCS_HOME is on a writable mount and that no other process is holding this session's stream" >&2
  exit 1
}

# Exactly the arguments the verb takes, and no more. An argument this script
# ignores is one the caller believed it was steering the hook with, and a hook
# that ran anyway would answer a journey that never happened.
takes() {
  if [ "$1" -ne "$2" ]; then
    fail "$3"
  fi
}

# The session stream this push belongs to, found by walking up from the tree git
# is running the hook in.
#
# The layout is checked rather than assumed at every step: what makes an ancestor
# the session's is that the state root already holds a stream named after it, so
# a hook run anywhere else finds none and refuses rather than naming a file no
# session is writing — a line appended where nothing reads it, and a journey that
# passes having proved nothing.
session_stream() {
  here=$(pwd)
  dir=$here
  while [ -n "$dir" ]; do
    candidate="$ONEVCS_HOME/streams/$(basename "$dir").ndjson"
    if [ -f "$candidate" ]; then
      printf '%s\n' "$candidate"
      return 0
    fi
    parent=$(dirname "$dir")
    if [ "$parent" = "$dir" ]; then
      break
    fi
    dir=$parent
  done
  return 1
}

case "${1-}" in
  wait-for)
    takes "$#" 2 "wait-for takes the path to wait for, and nothing else"
    if [ -z "$2" ]; then
      fail "wait-for takes the path to wait for"
    fi
    # Unbounded on purpose: the journey holding this push is what releases it,
    # and a hook that gave up on its own would settle a publication the test
    # believes it is still holding.
    until [ -f "$2" ]; do
      sleep 0.05
    done
    ;;
  break-streams)
    takes "$#" 1 "break-streams takes no arguments"
    require_home
    # `ONEVCS_HOME` is this script's external input and the next line removes a
    # tree under it, so being *set* is not enough to act on: a variable holding
    # a root, a home directory, or a typo would have this delete a `streams`
    # somewhere nobody meant. What makes it the state root is that `onevcs` has
    # already written the store this verb exists to break — so the store is what
    # is checked, and a path that does not hold one is refused rather than
    # destroyed.
    if [ ! -d "$ONEVCS_HOME/streams" ]; then
      fail "ONEVCS_HOME=$ONEVCS_HOME holds no streams directory, so it is not the state root onevcs wrote; there is nothing here to break"
    fi
    rm -rf "$ONEVCS_HOME/streams"
    if ! : >"$ONEVCS_HOME/streams"; then
      broke "cannot leave a file where $ONEVCS_HOME/streams was"
    fi
    ;;
  append-future-event)
    takes "$#" 1 "append-future-event takes no arguments"
    require_home
    if ! stream=$(session_stream); then
      fail "append-future-event runs in a tree under a session's run root; no ancestor of $(pwd) names a stream under $ONEVCS_HOME/streams"
    fi
    if ! printf '%s\n' '{"from":"a newer onevcs"}' >>"$stream"; then
      broke "cannot append to $stream"
    fi
    ;;
  *)
    fail "unknown command '${1-}'"
    ;;
esac
