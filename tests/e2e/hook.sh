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
#                        …or refuse the push once the        1
#                        ceiling below expires
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

# The state root itself, established before anything under it is removed.
#
# `ONEVCS_HOME` is this script's external input and `break-streams` removes a
# tree under it recursively, so *set* is not enough to act on, and neither is a
# lone `streams` directory: an operator's home, a filesystem root, or a typo that
# happened to hold one would pass that and have this delete somewhere nobody
# meant. What identifies the real root is the whole store `onevcs::home` lays
# out — the registry document beside the locks, sessions, streams, and
# workspaces directories it keeps everything under — together with the one thing
# that makes it *this* push's root rather than some other run's: it holds the
# stream the session making this push is writing, which is the very stream this
# verb exists to take away. A path that is not all of that is refused, loudly,
# rather than destroyed — and refused rather than skipped, because a hook that
# quietly did nothing would leave the journey asserting on a stream nothing ever
# broke.
require_state_root() {
  require_home
  if [ ! -f "$ONEVCS_HOME/registry.json" ]; then
    fail "ONEVCS_HOME=$ONEVCS_HOME holds no registry.json, so it is not a state root onevcs wrote; refusing to remove anything under it. Point ONEVCS_HOME at the state root this world gave onevcs, the way World::cmd does"
  fi
  for held in locks sessions streams workspaces; do
    if [ ! -d "$ONEVCS_HOME/$held" ]; then
      fail "ONEVCS_HOME=$ONEVCS_HOME holds no $held directory, so it is not a state root onevcs wrote; refusing to remove anything under it. Point ONEVCS_HOME at the state root this world gave onevcs, the way World::cmd does"
    fi
  done
  if ! session_stream >/dev/null; then
    fail "ONEVCS_HOME=$ONEVCS_HOME holds no stream for any ancestor of $(pwd), so it is not the state root of the session making this push; refusing to remove $ONEVCS_HOME/streams. Run this verb from the session's own tree, under the ONEVCS_HOME that session was given"
  fi
}

# How long `wait-for` waits for its rendezvous before it refuses the push.
#
# **Bounded, and the bound is what makes a held push reachable on every
# platform.** Unbounded, a hold nobody releases — a journey that panicked past
# its release, on a host with no portable way to reap the hook git left waiting —
# takes the whole job with it: the leg burns its budget and answers nothing.
# Bounded, that same abandonment refuses the push, the journey's assertions about
# a publication still in flight stop holding, and the leg goes red naming itself.
# A red journey is the better answer on both counts — it says what is wrong and
# it costs minutes rather than a runner — which is why this gives up rather than
# waits, though a ceiling that fired early would settle a publication a journey
# believes it is still holding.
#
# 300 seconds is picked to make that trade-off never arrive in a healthy run: the
# journeys that hold a publication hold it for well under five seconds, and no
# job's budget is anywhere near five minutes.
#
# The environment carries it so the journey that proves the expiry can reach it
# without waiting the ceiling out; every other journey runs on the default. It is
# external input, so it is refused rather than defaulted or clamped when it is not
# a number of seconds between 1 and an hour: `0` would expire every hold before it
# began, which is the silent opposite of what a journey asking for one means, and
# an hour is already far past every hold this suite takes. A leading zero is
# refused with them because `hook.bat` counts this out with `set /a`, which reads
# one as octal, and anything longer than the ceiling's own four digits is refused
# before it is compared as a number at all — arithmetic here is the host's word
# size and 32-bit over there, so a value nothing turned down would land as a
# deadline no clock can represent.
wait_ceiling() {
  seconds=300
  if [ -n "${ONEPIPELINE_FAKE_HOOK_WAIT_SECONDS-}" ]; then
    seconds=$ONEPIPELINE_FAKE_HOOK_WAIT_SECONDS
  fi
  refused="ONEPIPELINE_FAKE_HOOK_WAIT_SECONDS holds '$seconds', which is not a number of seconds between 1 and 3600. Unset it to wait the 300-second default, or set it to a whole number in that range with no leading zero"
  case "$seconds" in
    '' | 0* | *[!0-9]* | ?????*)
      fail "$refused"
      ;;
  esac
  if [ "$seconds" -gt 3600 ]; then
    fail "$refused"
  fi
}

# A `wait-for` whose ceiling ran out, which is a verb that could not do what it
# names: the push is refused, and the journey holding it fails on the assertions
# that no longer hold. See `wait_ceiling` for why the wait ends at all.
expired() {
  echo "pre-push: nothing wrote $1 within the ceiling of $seconds seconds: the held push expired" >&2
  echo "pre-push: nothing released this push; write that path to let it through, or raise ONEPIPELINE_FAKE_HOOK_WAIT_SECONDS, which carries the ceiling and is 300 seconds by default" >&2
  exit 1
}

case "${1-}" in
  wait-for)
    takes "$#" 2 "wait-for takes the path to wait for, and nothing else"
    if [ -z "$2" ]; then
      fail "wait-for takes the path to wait for"
    fi
    wait_ceiling
    deadline=$(( $(date +%s) + seconds ))
    until [ -f "$2" ]; do
      if [ "$(date +%s)" -ge "$deadline" ]; then
        expired "$2"
      fi
      sleep 0.05
    done
    ;;
  break-streams)
    takes "$#" 1 "break-streams takes no arguments"
    require_state_root
    if ! rm -rf "$ONEVCS_HOME/streams"; then
      broke "cannot remove $ONEVCS_HOME/streams"
    fi
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
