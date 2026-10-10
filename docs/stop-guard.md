# `onepipeline stop-guard`

The stop guard is one general command that answers whether a session's turn
may end: it asks `onepipeline unwatched` about the session and answers with one
verdict. It is not a feature of any harness. A harness's stop hook is a *wiring*
of it, stated per harness below, and the decision is made in the command alone.

It exists because a manager forgets to arm `watch`, and dispatched work then
sits for hours with nothing looking at it. Prose is remembered by the model or
it is not; a stop hook runs whether or not anybody remembered, and may refuse to
let the turn end.

## The contract

```
onepipeline stop-guard [--session <ID>] [--continuation] [--format <FORMAT>] [--source <COMMAND>]... [--source-timeout <SECONDS>] [--wake-budget <SECONDS>]
```

**Input** — the session whose stop this is, and whether the stop continues a
block the guard itself made. Either as the flags, or as one JSON object on
standard input when `--session` is absent:

```
{"session": "<ID>", "continuation": true}
```

`continuation` may be omitted and is `false`. No other field is read, and an
object carrying one this verb does not name is treated as unreadable.

**The session is the input's, never the environment's.** A dispatched worker
inherits its manager's `ONEPIPELINE_LAUNCHER_SESSION`, and the worker's own
session owns no run — which is what keeps the guard silent inside a dispatch.
So the guard never falls through to the environment: a blank or absent session
is answered `none`, not answered about whoever the process happens to be
running under. `--session ""` is that same nothing.

**Output** — one object on standard output, and exit `0` always:

| Verdict | Object | When |
| --- | --- | --- |
| block | `{"verdict":"block","reason":"<report>"}` | A run the session owns is **proven** unwatched, or a [declared source](#declared-sources) refuses the stop or could not be consulted. With no source declared, `reason` is `onepipeline unwatched`'s own lines, byte for byte: one per run, naming it, its standing word, why nothing counts as watching it, and the `onepipeline watch <run>` that does. With sources declared it is the [combination](#declared-sources). |
| warn | `{"verdict":"warn","message":"<one sentence>"}` | Something that is not evidence: a run whose watch or closure could not be judged under a [wake budget](#the-wake-budget), a wake budget that could not be read, the runs root could not be read, the question was refused, the engine answered with an error, or the guard's own memory could not be read, written or removed. The sentence names what could not be answered and the exact command to ask it by hand — `onepipeline unwatched --session <ID>`, or `onepipeline unwatched --session <ID> --wake-budget <SECONDS>` where the guard was given `--wake-budget` — and, where runs *are* unwatched over a memory the guard could not keep, names them and the `onepipeline watch <run>` for each. |
| none | `{"verdict":"none"}` | Nothing to say: the session owns nothing unwatched, the input could not be read or named no session, or this stop continues a block on a report that has not changed. |

Nothing else is written on standard output. Standard error carries exactly what
`onepipeline unwatched` would have written for the same question — the runs it
could not decide about — and nothing more.

**Block once per condition.** Before a block is made it is recorded, keyed by a
SHA-256 digest of the report, in one file per session at
`${XDG_STATE_HOME:-$HOME/.local/state}/onepipeline/stop-guard/<sha256(session)>`.
A relative `XDG_STATE_HOME` is ignored. On a continuation whose remembered
digest equals the current report's, the verdict is `none`: the manager was told
and did nothing, and a second identical block would hold the session open for
ever. A continuation whose report has *changed* — a second run now unwatched, or
a different one — blocks again. A session with nothing to report has its memory
removed. A memory that cannot be read on a continuation, or cannot be written
before a block, is a `warn`, never a block: the record is what makes a block
safe to make. One that cannot be removed is a `warn` too, because left behind it
would let a later continuation over that same report through unrefused.

**Cost** is `unwatched`'s: proportional to the run roots under the runs root,
and no run's merged event store is read — plus the slowest [declared
source](#declared-sources), bounded by `--source-timeout`.

**Renderings.** `--format neutral` is the contract above and the default.
`--format claude-code` and `--format codex` read the harness's own `Stop`
payload off standard input and render the same verdict in the harness's
decision shape. They are presentation over the one verdict — the decision path
is the same — and they are the whole of the harness-specific text in this crate.

## The wake budget

To have the guard ask whether each run the session owes will **wake it** in
time, rather than whether anything is watching it, export
`ONEPIPELINE_WAKE_BUDGET=<SECONDS>` in the environment the session and its hook
run in, or pass `--wake-budget <SECONDS>` on the hook's command line. Setting the
variable also makes it, less the [wake reserve](#the-wake-reserve),
`onepipeline watch`'s default `--timeout`, so the bare `onepipeline watch <RUN>`
a blocked line names, run from that session, is the watch that clears it. Leave
both unset and nothing changes.

A blocked line names what clears it; a `warn` names a run whose watch or closure
the guard could not judge. A run that has settled is cleared by closing it —
`onepipeline reply <RUN>` with a `complete` verdict, or
`onepipeline unwatched --acknowledge <RUN> --reason <TEXT>`.

## Flows

A **flow** is a launched process a manager must supervise that is not itself a
run: a planning launcher that starts a draft run, reviews it, then starts more
runs, or a deploy or CI wait that starts none. It is owned by the session that
started it, so `unwatched` — and through it this guard — holds that session for
the flow's whole life, including between its runs, and `watch --flow` can wake
the session on anything the flow or its runs need.

```
onepipeline flow run [--name <NAME>] [--session <ID>] [--bus-config <PATH>] -- <PROGRAM> [<ARG>]...
```

- **Session.** `--session`, else `ONEPIPELINE_LAUNCHER_SESSION`. With neither,
  or blank, the program runs with no flow registered, and one line on standard
  error says nothing will hold a session for it.
- **Id.** Minted from `--name` (letters, digits, `.`, `_` and `-`; default: the
  program's file name) as the first free of `<NAME>`, `<NAME>-2`, ... among the
  flows of the runs root every verb resolves (`ONEPIPELINE_RUNS_DIR`, or its
  default). Flow ids and run ids are separate namespaces.
- **What it says.** Before the program starts, exactly one line on standard
  error: `flow <ID>: watch it with: onepipeline watch --flow <ID>`.
- **Environment.** The program's is this process's plus `ONEPIPELINE_FLOW=<ID>`.
  A node dispatch never carries `ONEPIPELINE_FLOW`.
- **Nesting.** Where `ONEPIPELINE_FLOW` already names a live flow of the same
  session under the same runs root, the program runs in that flow: nothing is
  registered and nothing is printed.
- **Signals and status.** SIGINT, SIGTERM and SIGHUP are forwarded to the
  program, and the verb waits for it. It records the program's exit status — a
  signal-terminated program's as 128 + the signal; one that could not be started
  as 127 — and exits with that same status.
- **Liveness.** A flow is **live** while its holder — the `flow run` process — is
  alive, on the rule a watch lease is (its pid and start token). One whose holder
  is gone with no ending recorded has **died**.

**Runs launched in a flow.** `onepipeline start` under `ONEPIPELINE_FLOW` naming
a live flow of the launching session under the same runs root records
`"flow": "<ID>"` in the run's `launch.json`; otherwise the key is absent and the
run belongs to no flow.

**Where flows are kept.** Every flow sits under one directory of the runs root,
`.flows/`, whose layout is this repository's alone. Run-root discovery passes it
over; an engine before flows lists the same runs it always did, and names
`.flows` once as a skipped root with no launch record.

### What a session owes a flow

`onepipeline unwatched` — and so this guard, whose flags and verdicts do not
change — answers, per session, beside its run lines:

- A **live** flow with no qualifying live watch of it: one line, in
  `unwatched`'s own columns with `flow <ID>` where a run id goes, naming the flow,
  `LIVE`, why nothing counts as watching it, and
  `onepipeline watch --flow <ID>` — followed, where the wake budget was given as
  `--wake-budget <SECONDS>`, by ` --timeout <S>`, `<S>` being the budget less the
  [wake reserve](#the-wake-reserve), so the command named is one that qualifies.
  A flow id wider than the run-id column pushes the later columns right exactly as
  a long run id does.
- A **run** launched in a flow counts as watched while that flow is live and a
  qualifying live watch of the flow exists. Once the flow is not live, the run is
  judged on its own.
- A flow that **ended with status 0** is owed nothing.
- A flow that **ended non-zero, or died**, is owed **closure**: one line naming
  the flow, `ENDED` and `it ended with status <N>` or `DIED` and `it died`, and
  `onepipeline unwatched --acknowledge-flow <ID> --reason <TEXT>`, which closes
  it. `--acknowledge-flow` requires `--reason`, refuses a flow that is still live,
  and is recorded beside the flow, never in a run's journal.
- Exit status `6` whenever any run or flow line is reported, and as before
  otherwise.

A **qualifying** flow watch is decided by the wake-budget rule a run watch is
(divergence entry 98): with no budget, any live watch of the flow; under one, a
live watch of this session's that returns on a surface and whose recorded terms
give up within the budget **less the wake reserve**, counted from the instant it
armed. So a `watch --flow` whose `--timeout` equals the budget never qualifies,
and the line saying so names the reserve. The run-watch rule is unchanged.

### `watch --flow`

```
onepipeline watch --flow <ID> [--timeout <SECONDS|none>] [--tick-interval <SECONDS>] [--cursor <CURSOR>] [--until <CONDITION>]... [--log <PATH>]
```

`<RUN>` and `--flow` are mutually exclusive, and `--filter` and `--all` are a
run's alone. The watch records its lease and terms on the flow exactly as a run
watch does on a run, so `unwatched` counts it. It returns on the first of:

| Exit | Condition | When |
| --- | --- | --- |
| `4` | `surface` | A planner surface is waiting on the flow's own channel or on any member run's; the line names the flow or the run. |
| `6` | `node-settled` | A member run's node settled; the line names the run and the node. |
| `8` | `run-joined` | A member run was launched after the watch armed, or is past its cursor; the line names the run. |
| `0` | `ended` | The flow ended with status 0. |
| `9` | `flow-failed` | The flow ended non-zero, or died; the line names the status or the death and the `--acknowledge-flow` command. |
| `5` | `elapsed` | Its timeout elapsed. |

`--until` accepts `surface`, `node-settled` and `run-joined`, all three by
default; the flow's ending always ends the wait. Every ending line carries
`cursor <cursor>` to re-arm from, and each heartbeat carries the unread-surface
count summed over the flow's own channel and its member runs. Each line is written
twice, as a run watch's is: the human line on standard error (or `--log`), and one
JSON record on standard output — `{"watch":"return","flow_id":…,"condition":…,"exit":…,"cursor":…,"unread":{"count":…}}`,
with `run_id`, `node`, `status` or `died` where the condition names one.

### The wake reserve

Under a wake budget a watch must actually *return* within the budget, not only
be configured to. So `watch` — a run's and `--flow` alike — given no `--timeout`
gives up at the wake budget less a fixed **wake reserve of 2 seconds**
(`WAKE_RESERVE_SECONDS`). While it waits it checks its deadline at least every
**500 milliseconds** (`DEADLINE_CHECK_MILLIS`), whatever `--tick-interval` says,
and its exit after the deadline does no work but writing its ending line and
removing its lease.

### The flow's own channel

Every registered flow has a planner channel of its own, laid out exactly as a
run's, under the bus configuration it was registered with (`--bus-config`,
checked as `start` checks its own), and kept with the flow. `onepipeline surface`,
`next`, `reply` and `channel queue` take `--flow <ID>` in place of `<RUN>` — with
it, the one positional `surface` and `reply` take is the file — and the two are
mutually exclusive. A reply to a flow is a verdict alone: a flow has no graph to
edit, so one carrying commands is refused whole. `next --flow` answers `events`
empty. `onepipeline ask` asks on the flow's channel where it names no run and
`ONEPIPELINE_FLOW` is set; a run named through `ONEPIPELINE_RUN_ID` still wins. That
is how a flow that launches no run reports to its manager and asks a blocking
question.

## Declared sources

A host whose "may this turn end" question is wider than `unwatched` declares the
rest of it on the same registration, as one `--source <COMMAND>` per question —
for example `onepipeline stop-guard --format claude-code --source 'just
unfinished'`. With none declared the verb is exactly the contract above.

- **What a source is asked.** On every stop, each declared source is run through
  the platform shell (`sh -c`, or `cmd /C` on Windows) and handed this verb's own
  neutral input on standard input — `{"session":"<ID>","continuation":<bool>}`
  and a newline — naming the session *this verb* is answering about: the
  payload's under `--format claude-code` and `codex`, never the environment's.
  `ONEPIPELINE_LAUNCHER_SESSION` in its environment is set to that same session.
  Those are the bytes to hand it by hand, and a source is drivable that way with
  no guard in front of it. A source must take the whole of it: one that closes
  its standard input before the input was delivered in full has answered
  without knowing whose stop this is, so whatever it answered is not taken and
  it is a source that [could not be consulted](#declared-sources). Input the
  pipe accepted before the source exited counts as delivered, whether or not it
  was read. Its standard error is discarded. Sources are asked
  concurrently with each other and with `unwatched`, and the same command
  declared twice is asked once.
- **What it may answer.** One object on standard output, in the vocabulary this
  verb renders under `--format neutral` — `{"verdict":"block","reason":"…"}`,
  `{"verdict":"warn","message":"…"}` or `{"verdict":"none"}` — and exit `0`. The
  object is read closed: no other field, no field twice, and a `reason` or
  `message` that is not blank. A source never needs to know which `--format` the harness asked for.
- **How answers combine.** The strongest wins: `block` over `warn` over `none`.
  A combined block's `reason` carries every block — the verb's own report first,
  byte for byte, then each refusing source's, in the order declared, each headed
  by the source's command — followed by every warning, because a block's
  rendering has nowhere else to carry one. A combined warn joins every warning.
  Each `--format` renders that one verdict in the shape it documents.
- **Block once per condition, per source.** Each source's block is remembered
  under its own file beside the verb's —
  `<sha256(session)>.<sha256(command)>` in the same directory — before it is made,
  and a continuation over an unchanged report *from that source* is `none` for
  it. So a continuation after a block ends the turn unless some condition moved,
  and then it refuses on what moved alone. A source answering `warn` or `none`
  has its memory removed. A source memory that cannot be read, written or
  removed is a `warn`, exactly as the verb's own is.
- **A source that cannot be consulted blocks.** A source that is missing, cannot
  be started, is not delivered its whole input, exits non-zero, does not answer
  within `--source-timeout` seconds
  (default `10`, at most `3600`; it is then ended, with everything it started —
  and one that exits leaving something behind holding its standard output past
  that deadline is the same unanswered source, with what it left ended too, on
  Unix, where the source runs in a process group of its own), writes nothing,
  or answers outside the vocabulary is a `block` whose reason names the source,
  what went wrong, and the input to hand it by hand. It never reads as `none`.
  Block rather than warn is the safe choice here, and the opposite of the rule
  for the verb's own question, because the host *declared* the source: its
  condition must hold before a turn ends, and a failed consultation leaves it
  unshown — a `warn` would let the turn end with the condition unenforced while
  the host believes it enforced. The per-source memory bounds the cost: an
  unchanged failure refuses one stop, and its continuation ends the turn, so a
  broken source cannot hold a session in a loop. Keep `--source-timeout` inside
  the hook's own `timeout`, or the harness kills the whole hook first.

<!-- llmlint: ignore-block[contracts_have_one_source_or_a_drift_gate] the two harness
sections below restate contracts owned by Claude Code and Codex, neither of which publishes
its Stop hook schema in a form this repository's offline gate can read: Claude Code's is
prose, and Codex's is compiled into its binary. The drift that can be gated is gated —
`tests/e2e/stop_guard.rs` parses the Claude Code `settings.json` entry out of this page and
drives full Stop payloads through it — and "What was verified, and how" at the foot of this
page records the one-time reading of `codex-cli 0.154.0` so a reader can re-run it against
their own Codex. -->
## Claude Code

Claude Code runs `Stop` hooks when the main agent has finished responding,
passes one JSON object on the hook's standard input, and reads its standard
output as the decision. Add this to `.claude/settings.json` (project) or
`~/.claude/settings.json` (user):

```json
{
  "hooks": {
    "Stop": [
      {
        "hooks": [
          {
            "type": "command",
            "command": "onepipeline stop-guard --format claude-code",
            "timeout": 30
          }
        ]
      }
    ]
  }
}
```

- **Command.** `onepipeline` as the hook's process resolves it; a host that
  installs the engine into a project environment names it by path, for example
  `.venv/bin/onepipeline stop-guard --format claude-code`. No shell adapter is
  needed.
- **Timeout.** Seconds before Claude Code cancels the hook; 30 is generous by two
  orders of magnitude for the verb's bound, and a killed hook is a stop that was
  never guarded, so keep one.
- **How the payload reaches the verb.** `--format claude-code` reads the `Stop`
  payload's `session_id` as the session and `stop_hook_active` as the
  continuation — nothing else of the payload is read — so the session asked
  about is the conversation's own, whatever `ONEPIPELINE_LAUNCHER_SESSION` the
  hook's environment carries.
- **What Claude Code does with each verdict.** `block` is rendered
  `{"decision":"block","reason":"<report>"}`: Claude Code refuses to end the
  turn and shows Claude the reason, which names the runs and the `watch` to arm
  on each. `warn` is rendered `{"systemMessage":"<sentence>"}` with no decision
  beside it: the turn ends and the sentence is shown to the person. `none` is
  rendered as **nothing** — no output, exit `0` — and the turn ends. Claude Code
  sets `stop_hook_active` on the stop that follows a block, which is what makes
  the guard block once per condition, and ends the turn itself after eight
  consecutive blocks.

## Codex

Codex CLI (0.154 and later, with the `hooks` feature on, which it is by
default) runs `Stop` hooks when a turn completes, passes one JSON object on the
hook's standard input, and reads its standard output as the decision — its
`Stop` payload and decision are the same shape as Claude Code's, by Codex's own
schema. Add this to `~/.codex/hooks.json` (user) or `<repo>/.codex/hooks.json`
(project; loaded only where the `.codex/` layer is trusted):

<!-- What follows was read off the installed Codex CLI rather than assumed; see
     "What was verified, and how" at the foot of this page. -->

```json
{
  "hooks": {
    "Stop": [
      {
        "hooks": [
          {
            "type": "command",
            "command": "onepipeline stop-guard --format codex",
            "timeout": 30
          }
        ]
      }
    ]
  }
}
```

- **Command.** As for Claude Code: `onepipeline` as the hook's process resolves
  it, or by path.
- **Timeout.** Seconds before Codex cancels the hook (600 when omitted); keep
  one, for the reason above.
- **How the payload reaches the verb.** `--format codex` reads the payload's
  `session_id` as the session and `stop_hook_active` as the continuation, exactly
  as the Claude Code rendering does.
- **What Codex does with each verdict.** `block` is rendered
  `{"decision":"block","reason":"<report>"}`: Codex does not end the turn and
  submits the reason as the continuation prompt. `warn` is rendered
  `{"systemMessage":"<sentence>"}`: the turn ends and the sentence is shown.
  `none` is rendered as nothing, and the turn ends. Codex sets
  `stop_hook_active` on the stop that follows a block.

- **Trust.** Codex reports a hooks file it has not yet had confirmed as
  `trustStatus: "untrusted"`, per file and per content hash, and prompts to
  trust it. Editing the file changes its hash, so a hook is re-confirmed after
  every edit — expect that prompt once when you add this entry, and again
  whenever you change it.
- **An event name Codex does not know is dropped in silence**, with no warning
  and no error, so a typo in `"Stop"` reads exactly like a hook that never
  fires. `codex app-server`'s `hooks/list` (below) is how you tell the two
  apart.

Disable hooks in Codex with `[features] hooks = false` in `config.toml`; there is
then no mechanism that can refuse a stop, and a manager arms `onepipeline watch
<run>` by hand, asking `onepipeline unwatched --session <ID>` which runs need
one.

<!-- llmlint: ignore-end[contracts_have_one_source_or_a_drift_gate] -->
## Any other harness

Feed the neutral contract: hand the verb the session and the continuation as
flags or as the one object above, and map the verdict to whatever the harness
reads — `block` to its refusal with `reason` as the text, `warn` to its
user-facing message, `none` to its "proceed". The decision stays in the command;
the adapter only renames fields.

## What was verified, and how

A wiring page that is wrong is worse than none, so the Codex claims above were
read off the installed CLI (`codex-cli 0.154.0`) rather than taken from its
documentation. What was read, and how, so the next reader can re-check it
against whatever Codex they have:

- **`Stop` is a real event with a real runtime path**, distinct from
  `SubagentStop`: the binary carries a `stop.command.input` schema whose
  `hook_event_name` is `const: "Stop"` and whose required fields include
  `session_id` and `stop_hook_active`, a `stop.command.output` schema whose
  properties are `decision` (`enum: ["block"]`), `reason`, `systemMessage`,
  `continue`, `stopReason` and `suppressOutput`, and the runtime message
  `Stop hook exited with code 2 but did not write a continuation prompt to
  stderr`. The `reason` property's own description reads *"Claude requires
  `reason` when `decision` is `block`"*, which is Codex saying in its own schema
  that this is Claude Code's shape.
- **`Stop` is accepted as a `hooks.json` key, `timeout` is the right spelling,
  and `600` is the default.** Asked over the app server:

  ```
  codex app-server   # then, on stdin: initialize, initialized, hooks/list
  ```

  with a `hooks.json` naming `"Stop"`, `"SessionStart"` and one invented event,
  it answered with the first two parsed (`"eventName": "stop"` and
  `"sessionStart"`) and the invented one absent, with no warning or error. A
  file-level `"timeout": 30` came back as `"timeoutSec": 30`; a file-level
  `"timeoutSec": 45` was ignored and that handler came back at `600`. So
  `timeout` is the key a hooks file uses, `timeoutSec` is only what the app
  server reports it as, and an unknown event name fails silently.
- **The `hooks` feature is on by default**: `codex features list` reports
  `hooks  stable  true`.

What could **not** be verified here is a `Stop` hook firing during a real turn:
this host's Codex account was out of credits, and hooks do not run for a turn
that fails before it starts. So the four bullets above are read off the parser
and the compiled schemas, which is what decides whether the wiring is accepted;
whether Codex then honours `decision: "block"` at runtime is its schema's claim
and not something measured here.
