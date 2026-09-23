# Recorded planner-channel directories

Whole `runs/<run>/channel/` directories as `onepipeline` 0.28.2's channel code
wrote them — this crate's own recordings, and the one recording `onemessagebus`
held that is not byte for byte one of them (see
`onemessagebus-repair-2-pending-bus/`). `tests/channel_layout.rs` reads each one
through this crate's `planner-channel` layout and re-applies its history into an
empty directory, byte for byte; `tests/release_channel/` (`just release-compat`) holds a channel
this build writes to the live 0.28.2 wheel and the other way round; and
`tests/e2e/recorded_channel.rs` substitutes each one into the
recorded run root beside it, drives this build's `status`, `runs`, `results` and
`next` over it, re-derives its `queue.json`, and holds every answer and every
channel byte to what the 0.28.2 binary answered over the same directory. Those
answers are checked in under `../answers/`, and `scripts/record-channel-answers.sh`
is how they were captured. `tests/layout_document.rs` offers every record each one
holds through a bus of the compiled-in layout and one of the published
`schemas/planner-channel.json`, and holds the two directories they write to one
another.

Every file here is byte for byte what was on disk. Nothing was edited or
redacted. The journeys edit one thing in their *copy* of the recorded run root,
and only there: whether that run's driver is gone is proved only on the host its
launch record names, by asking that host's process table about its pid. So the
world tells every command it runs on the recording host and gives its copy of
the launch record a pid no platform issues. The run then reads as finished on
any host, by the same answer the recording host gave. `src/channel.rs` was byte-identical from v0.28.0 through the tree this
node started from (its last change, `bbee552`, first shipped in v0.28.0), so each
directory is exactly what the 0.28.2 release writes.

Only the files of the channel layout are copied, where the run has them — the
files `onepipeline::channel::layout::FILES` names, which the write-side journey fails
on a channel file it does not name. Each run's channel
directory also held an empty `handover/` directory. That is the ownership
handover gate, not the channel layout, so it is left out.

## `domain-driven-modularity-2/` (recorded)

The complete channel of the finished run `domain-driven-modularity-2` on this
host (`/home/nick/projects/ai-orchestrator/runs/domain-driven-modularity-2`),
copied on 2026-09-14. The run started 2026-09-12T19:31:24Z, and its channel was
last written at 2026-09-13 04:10 local time. All seven files.

- **A check-in superseded by a newer one.** Check-in `0` was still waiting when
  check-in `1` was queued, and was replaced by it without ever being claimed.
- **Surfaces answered by replies.** Surfaces `4`, `6` and `11` were each claimed
  into the pending slot and released by the reply that answered them.
- **Replies carrying both a verdict and edits.** Replies `0`–`7` carry commands
  beside their verdict; reply `8` is verdict-only.
- **Surfaces abandoned by one serving session and attended by a later one of the
  same asker.** Surfaces `4`, `10` and `13` were each abandoned when their
  `channel serve` session ended, and taken back by the next session of the same
  asker. Surfaces `5`, `6`, `11` and `14` were abandoned by those sessions and
  never attended.
- **A reply claimed to a cursor short of the log's end.** `replies.jsonl` holds 9
  replies (ids 0–8), and `replies-cursor.json` holds `7`, so replies 7 and 8 were
  never claimed.
- **Commands-only replies with their recorded outcomes.** `commands.jsonl` holds 7
  envelopes (ids 0–6) and `commands-cursor.json` holds `7`. Envelopes `1` and `5`
  (the monitor's `finding`s) and `2`, `3` and `6` (the planner's `note`s) carry no
  verdict, so no reply on `replies.jsonl` carries them. `command-outcomes.jsonl`
  answers all 7.
- `queue.json`: nothing waiting, nothing pending, `next_id` 17, stamped and sealed.

## `onemessagebus-repair-2/` (recorded)

The complete channel of the finished run `onemessagebus-repair-2` on this host
(`/home/nick/projects/ai-orchestrator/runs/onemessagebus-repair-2`), written on
2026-09-13 between 02:02 and 02:43 local time and copied on 2026-09-14. It has no
command queue, so it has four files: `surfaces.jsonl` (3 surfaces, each queued
and claimed, surface `1` answered), `queue.json` (nothing waiting or pending,
`next_id` 3), `replies.jsonl` (one verdict-only reply) and `replies-cursor.json`
(`1`).

## `onemessagebus-repair-2-pending/` (produced by the 0.28.2 binary)

**A blocking surface claimed and still pending** is a state no finished run on
this host holds: every 0.28-format channel here ended with its pending slot empty
or holding an abandoned surface. So it was produced by the 0.28.2 release itself,
the `onepipeline` binary of the `onepipeline-cli` 0.28.2 wheel run with
`uv tool run --offline --from onepipeline-cli==0.28.2`, over a scratch copy of the
recorded `onemessagebus-repair-2` run root and channel:

1. `channel serve onemessagebus-repair-2`, with
   `ONEPIPELINE_CHANNEL_ASKER=fixture-listener`,
   `ONEPIPELINE_REPLY_TIMEOUT_SECONDS=1` and
   `ONEPIPELINE_SERVE_SESSION_SECONDS=3`, was fed one blocking frame
   (`{"kind":"planner-question","message":"Recorded for the onepipeline channel
   fixtures: a blocking question raised through channel serve, left pending by
   next.","blocking":true}`) while its stream stayed open past the session bound.
   It queued surface `3` under that asker, timed out waiting for a reply, and
   ended on its session bound, which is the ending that marks nothing abandoned.
2. `next onemessagebus-repair-2` claimed surface `3` into the pending slot.

The four files are the copy after those two commands, unedited. `surfaces.jsonl`
is the recorded log with surface `3`'s `queued` and `claimed` records appended,
and `queue.json` holds surface `3` pending, not abandoned, with `next_id` 4.

## `onemessagebus-repair-2-pending-bus/` (produced by the 0.28.2 binary, recorded in the bus)

The same state as `onemessagebus-repair-2-pending/`, produced the same way by the
same 0.28.2 binary, and recorded in `onemessagebus` rather than here: it was
`crates/onemessagebus-agent/tests/recorded/channel/onemessagebus-repair-2-pending/`
at that repository's commit `07b2f2c`, the last the bus carried the
`planner-channel` layout at (onemessagebus#126 moved the layout into this crate,
and the bus's fixtures came with it). Its two other recorded directories,
`domain-driven-modularity-2/` and `onemessagebus-repair-2/`, were byte for byte
the copies above and are not repeated. This one differs from the copy above in
the frame the serving session was fed — its message reads `Recorded for the
onemessagebus channel fixtures: …` — and so in surface `3`'s `queued_at` and the
projection's `accounted` and `seal`, so it is held beside it as a second recording
of that state. Unedited, as every directory here is.

## `waiting-updates/` (produced by the 0.28.2 binary)

**Updates nobody has read**, which no finished run on this host holds either: a
finished run's check-ins were read. Produced on 2026-09-14 by the 0.28.2 release's
`onepipeline` binary, run from uv's cached copy of the `onepipeline-cli` 0.28.2
wheel, over a scratch copy of the recorded `onemessagebus-repair-2` run root with
an empty `channel/`, with `ONEPIPELINE_RUNS_DIR` naming that scratch runs root:

1. `surface onemessagebus-repair-2 --kind check-in --message first`
2. `surface onemessagebus-repair-2 --kind check-in --message second`, which
   replaced the first while it was still waiting
3. `surface onemessagebus-repair-2 --kind finding --message "a finding"`

The two files are the copy after those three commands, unedited: `surfaces.jsonl`
holds the three `queued` records, and `queue.json` holds check-in `1` and finding
`2` waiting, nothing pending, `next_id` 3. Every view counts the two as unread;
how long they have been unread is the clock's, so the answers blank that age.

## `../answers/written-channel.json` (written by the 0.28.2 binary)

Not a recorded directory but a channel **written from empty**, so that what this
build writes is held to what 0.28.2 writes rather than only what it reads.
`recorded_channel::a_channel_this_build_writes_is_byte_for_byte_the_channel_the_release_writes`
has each binary launch its own run, `written`, whose one node the `oneagentgraph`
double holds open, so that binary's driver is live and reconciles the command
queue for the whole journey. Over it the journey takes, in order: two check-ins,
the second replacing the first, and a finding; `runs`, for the unread count; a
blocking question `channel serve` raises under
`ONEPIPELINE_CHANNEL_ASKER=written-listener`; three `next`s; a reply carrying a
verdict and a `finding` command, whose verdict reaches that server and whose
command the driver applies; a commands-only reply the driver applies; a second
session of the same asker that waits out a one-second window and ends, abandoning
its question; a third that attends it and ends on its session bound; a verdict no
listener reads, leaving the reply cursor short of the log; `runs` again; and
`stop`.

The answer file holds each step's exit, both unread lines, and every channel file
0.28.2 wrote across those steps — `surfaces.jsonl`, `queue.json`,
`replies.jsonl`, `replies-cursor.json`, `commands.jsonl`,
`commands-cursor.json` and `command-outcomes.jsonl` — captured by
`scripts/record-channel-answers.sh` with the 0.28.2 binary. The instants
(`queued_at`, `at`), what the projection derives from the log's bytes
(`accounted`, `seal`), every `correlation` and how long an update has been
unread are normalized in place, as the manager ruled for this comparison, and
nothing else is.

# Recorded run root

## `../run-root/onemessagebus-repair-2/`

The run root of the finished run `onemessagebus-repair-2` (above): its
`launch.json`, `plan.json`, `checkpoint.json`, `events.jsonl`, `summary.json` and
`result.json`, copied from this host unedited. The verbs whose answers are
recorded open a run root rather than a bare channel directory, so every recorded
channel above is read inside a scratch copy of this root, substituted for its
`channel/`.
