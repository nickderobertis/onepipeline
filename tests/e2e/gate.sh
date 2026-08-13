# A repository gate command, for the journeys that need a gate to do something
# other than pass. Written into a world's own scratch by `harness::gate_script`
# and run by `onevcs` in the session's worktree.
#
# The worktree sits at `$ONEVCS_HOME/<identity>/runs/<token>/worktree`, so the
# session's own token is the name of the directory above it — which is how the
# last two verbs address the stream this session is writing without anything
# telling them one.
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
# 64 is `sysexits.h`'s EX_USAGE, which is what the compiled gate this replaced
# answered a command line it did not speak with — kept, because a gate that
# succeeded on a verb nobody implemented would let a journey assert against a
# gate that never ran. 1 is separate from it for the same reason in the other
# direction: a write the filesystem refused is not a caller who typed the wrong
# thing, and a gate that reported either as success would leave a journey
# asserting on state nothing produced.
set -u

fail() {
  echo "gate: $1" >&2
  echo "gate: the verbs are: wait-for PATH | break-streams | append-future-event" >&2
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
# caller's mistake: this one is the host's, and neither is a gate that passed.
broke() {
  echo "gate: $1" >&2
  echo "gate: the host refused the write, not the caller: check that ONEVCS_HOME is on a writable mount and that no other process is holding this session's stream" >&2
  exit 1
}

# Exactly the arguments the verb takes, and no more. An argument this script
# ignores is one the caller believed it was steering the gate with, and a gate
# that ran anyway would answer a journey that never happened.
takes() {
  if [ "$1" -ne "$2" ]; then
    fail "$3"
  fi
}

case "${1-}" in
  wait-for)
    takes "$#" 2 "wait-for takes the path to wait for, and nothing else"
    if [ -z "$2" ]; then
      fail "wait-for takes the path to wait for"
    fi
    # Unbounded on purpose: the journey holding this gate is what releases it,
    # and a gate that gave up on its own would settle a publication the test
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
    # The working directory is `onevcs`'s to choose, so the layout this reads a
    # token out of is checked rather than assumed: run anywhere else, the
    # derivation would name some other directory's basename and this would
    # create a stream file no session is writing — a line appended where nothing
    # reads it, and a journey that passes having proved nothing.
    here=$(pwd)
    if [ "$(basename "$here")" != "worktree" ]; then
      fail "append-future-event runs in a session worktree; this is $here"
    fi
    stream="$ONEVCS_HOME/streams/$(basename "$(dirname "$here")").ndjson"
    if [ ! -f "$stream" ]; then
      fail "no session stream at $stream"
    fi
    if ! printf '%s\n' '{"from":"a newer onevcs"}' >>"$stream"; then
      broke "cannot append to $stream"
    fi
    ;;
  *)
    fail "unknown command '${1-}'"
    ;;
esac
