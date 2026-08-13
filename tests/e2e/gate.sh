# A repository gate command, for the journeys that need a gate to do something
# other than pass. Written into a world's own scratch by `harness::gate_script`
# and run by `onevcs` in the session's worktree.
#
# The worktree sits at `$ONEVCS_HOME/<identity>/runs/<token>/worktree`, so the
# session's own token is the name of the directory above it — which is how the
# last two verbs address the stream this session is writing without anything
# telling them one.
set -u

case "${1-}" in
  wait-for)
    # Unbounded on purpose: the journey holding this gate is what releases it,
    # and a gate that gave up on its own would settle a publication the test
    # believes it is still holding.
    until [ -f "$2" ]; do
      sleep 0.05
    done
    ;;
  break-streams)
    rm -rf "$ONEVCS_HOME/streams"
    : >"$ONEVCS_HOME/streams"
    ;;
  append-future-event)
    token=$(basename "$(dirname "$(pwd)")")
    printf '%s\n' '{"from":"a newer onevcs"}' >>"$ONEVCS_HOME/streams/$token.ndjson"
    ;;
  *)
    echo "unknown gate command ${1-}" >&2
    exit 64
    ;;
esac
