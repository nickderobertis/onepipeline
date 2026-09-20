# `onepipeline stop-guard`

The stop guard is one general command that answers whether a session's turn
may end: it asks [`onepipeline unwatched`](contract-divergences.md#68-nothing-on-disk-says-a-run-is-being-watched-so-nothing-can-ask-which-run-is-not--open)
about the session and answers with one verdict. It is not a feature of any
harness. A harness's stop hook is a *wiring* of it, stated per harness below,
and the decision is made in the command alone.

Why it exists is the measured failure entry 68 records: a manager forgets to arm
`watch`, and dispatched work sits for hours with nothing looking at it. Prose is
remembered by the model or it is not; a stop hook runs whether or not anybody
remembered, and may refuse to let the turn end.

## The contract

```
onepipeline stop-guard [--session <ID>] [--continuation] [--format <FORMAT>]
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
| block | `{"verdict":"block","reason":"<report>"}` | A run the session owns is **proven** unwatched. `reason` is `onepipeline unwatched`'s own lines, byte for byte: one per run, naming it, its standing word, why nothing counts as watching it, and the `onepipeline watch <run>` that does. |
| warn | `{"verdict":"warn","message":"<one sentence>"}` | Something that is not evidence: the runs root could not be read, the question was refused, the engine answered with an error, or the guard's own memory could not be read or written. The sentence names what could not be answered and the exact command to ask it by hand — `onepipeline unwatched --session <ID>` — and, where runs *are* unwatched over a memory the guard could not keep, names them and the `onepipeline watch <run>` for each. |
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
safe to make.

**Cost** is `unwatched`'s: proportional to the run roots under the runs root,
and no run's merged event store is read.

**Renderings.** `--format neutral` is the contract above and the default.
`--format claude-code` and `--format codex` read the harness's own `Stop`
payload off standard input and render the same verdict in the harness's
decision shape. They are presentation over the one verdict — the decision path
is the same — and they are the whole of the harness-specific text in this crate.

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

Disable hooks in Codex with `[features] hooks = false` in `config.toml`; there is
then no mechanism that can refuse a stop, and a manager arms `onepipeline watch
<run>` by hand, asking `onepipeline unwatched --session <ID>` which runs need
one.

## Any other harness

Feed the neutral contract: hand the verb the session and the continuation as
flags or as the one object above, and map the verdict to whatever the harness
reads — `block` to its refusal with `reason` as the text, `warn` to its
user-facing message, `none` to its "proceed". The decision stays in the command;
the adapter only renames fields.
