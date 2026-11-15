# AGENTS (oneharness-core)

Subtree rules for the reusable engine — the pure `domain` layer and the `io`
boundary every surface (the CLI, the SDKs, an embedder) drives.

<!-- llmlint: ignore-file[agents_md_durable_and_terse] Everything below this header was moved verbatim from the root AGENTS.md (its "What this binary is" engine rules and its "Adding or changing a harness" checklist), under this change's instruction to move — not rewrite or trim — the text that governs this project, with standalone trimming of AGENTS.md out of its scope. The dated inventories inside it (which harnesses a capability is wired for today) are the original's; a durability pass over this checklist is a change of its own. -->

## What this binary is

- A thin CLI over a registry of **harness adapters**. Each adapter is data: a
  canonical id, a default binary name, an install hint, an output format, and two
  pure functions — build the argv, and best-effort extract the final text.
- **`run` is a library call the CLI wraps, not a verb the CLI owns.** Its whole
  orchestration lives in `oneharness_core::io::run` behind
  `run(&RunRequest, RunControls) -> Result<RunOutcome, _>`, which **returns** the
  report; `src/commands/run.rs` only converts clap's `RunArgs` into `RunRequest`,
  owns stdout (the buffered report, or the streamed events — text lines, or the
  NDJSON protocol under `--format json` — through its `EventSink`), and maps the outcome to an exit code. So **nothing under
  `io::run` may write to stdout** — a `println!` there is a bug in a consumer's
  own contract — and a new `run` flag is three edits, not one: the clap field,
  the `RunRequest` field, and the `From<&RunArgs>` conversion (a field dropped
  there goes silently missing from every run, which is what
  `every_run_flag_reaches_the_engine_request` exists to catch). `--compact` and
  `--format` are deliberately NOT on `RunRequest`: they are about printing,
  which the shell owns. `--format` is one clap type (`cli::Format`) rendered
  through `commands::print_report`, so a verb gains a text view by handing that
  seam a renderer. **The CLI's stdout defaults to `text`** wherever it points
  (never a TTY heuristic); `--format json` is the programmatic contract, and
  `--compact` alone selects it. Each verb carries the pair as ONE
  `cli::StdoutFormat`, parsed at the clap boundary, so `--format text
  --compact` is refused there (exit 2, naming both) before any verb runs —
  nothing has been synced, spawned or interrupted when it is. A streamed run's
  stdout is chosen by `--format` alone, whichever layer turned streaming on
  (`commands::run`'s `StreamView`): text by default like every other stdout,
  NDJSON only under `--format json`, so every in-repo stream reader (the SDKs'
  `always` argv, tests, `oh_stream_assert`) names `--format json`. Text views live in `oneharness_core::domain::render`,
  never in `src/commands`, so an embedder prints what the CLI prints. Number
  live events only through `events::EventStream`: history's closing record
  skips the events already persisted by the `index` it shares with the report.
  Anything reading stdout as JSON says so: the SDKs on
  every call, and every test or script here (the `crates/oneharness-e2e/tests/cli.rs` `run` helper
  passes `--format json`; `run_as_typed` is the bare invocation).
  Nor is any `--no-x` half of a clap-exclusive pair — the request carries the one
  value they resolve to (`stream`/`history` as `Option<bool>`, `--bypass` folded
  into `mode`, `--fork` inside the `Resume` it is meaningless without), because
  a library caller has no clap to make the conflicting state unreachable.
  `RunControls` carries three of the four things a subprocess hop gave a
  consumer for free — the event sink, a `CancelToken` (the only handle that
  reaches a harness tree, since each harness leads its own process group), and
  whether oneharness may take over the host's SIGINT/SIGTERM disposition
  (`signal_cancel`, which the CLI sets and an embedder leaves off, cancelling
  its own token instead). The fourth is that hop's *grouping*, and it is a
  `ProcessSupervisor` on `run_supervised` rather than a fifth `RunControls`
  field: that struct is exhaustively constructible, so a field would break every
  literal already written — as any further side channel would, which is why each
  takes its own entry point. A caller's `pre_exec` must run last to win, and
  teardown follows the group the child is REALLY in, asked of the OS: one the
  caller re-parented it into is the caller's to reap, and oneharness ends only
  the direct child. Warnings still go to the host's stderr, so an embedder
  inherits them. `tests/library.rs` is that surface's drift alarm, one test per
  property: report-back-without-printing (an fd-1 redirect across the call), an
  event observed while the run is demonstrably still streaming, a cancel that
  stops the harness's own descendant — proven from outside the tree — and, for
  the supervisor, both teardown halves and a hand-over from every execution
  model.
- `run` drives the selected harnesses as a **fallback chain by default** —
  `run_mode` resolves to `fallback` at its one site (`io::run`), on every
  surface (library, CLI, SDKs), so `parallel` is the opt-in — each as a
  subprocess with a timeout, and emits one report. A chain of exactly one
  candidate carries a batch or a `--resume`/`--fork` continuation as the
  single-harness run (`fallback: null`) but keeps the chain's exit rule, since
  the driver is an implementation detail and the exit code is not
  (`undriven_chain_of_one` folds into `require_available` at its one site);
  over two or more candidates those are
  refused, naming the default when the mode was unset. `io::process` owns each launcher's whole
  tree (Unix process group; Windows kill-on-close Job Object assigned while the
  child is suspended), applies a brief TERM→KILL grace on Unix, reaps, and bounds
  pipe drain; both buffered and streaming runner paths must go through it so an
  npm wrapper's native child cannot survive or hold inherited pipes open. It also
  owns `resolve_program`, so every spawn — runner and `usage` probe alike — takes
  a bare registry name through the PATHEXT-aware lookup that finds Windows's
  `codex.cmd`; a site that skips it reports an installed harness as
  `program not found`. **Cancellation** goes through that same
  `Finish::Terminate` teardown: `io::cancel` holds a caller-owned `CancelToken`
  plus a process-wide flag raised by `install_signal_cancel` (SIGINT/SIGTERM;
  Windows console-control), which the CLI installs for `run` *after* any stdin
  read. Because the launcher leads its own process group, a signal that killed
  oneharness would orphan a live, billing harness — so both runner loops bound
  their wait/pipe-read by `CANCEL_POLL_SLICE` and re-check the flag. That bound is
  the whole mechanism for a **silent** harness: it emits no line, so `on_line`
  (and therefore `StreamStep::Stop`) is unreachable, and a plain wait to the
  deadline would hold the run for the entire timeout after the caller gave up.
  A cancelled run is `Status::Cancelled` — its own value, never `timeout` (nothing
  was exceeded) and never the streaming consumer-`Stop`'s `ok` — with its captured
  bytes still normalized; queued jobs report cancelled without spawning. Timeout
  status is authoritative, but `commands::run::executed_result` still normalizes
  any complete captured records into text/usage/session/events (skipping a
  truncated JSONL tail), which history then preserves. A **same-prefix batch** is
  the dual shape:
  pass more than one prompt (`--prompt`/`--prompt-file` are repeatable, each one
  whole prompt) and `run` fans **one** harness over the N prompts instead —
  single-harness by nature (a provider cache prefix is per harness/model/tools),
  and not itself a `--resume`/`--fork` continuation (those are a usage error with a
  batch). Pure scheduling lives in `domain::batch` (the `BatchStrategy` waves:
  `speed` = one concurrent wave; `min-tokens` = a one-call warm-up wave then the
  fanned-out rest, with a barrier between). `min-tokens` *reduces tokens* only via a
  **cache-reusing fork** (`HarnessSpec.fork_reuses_cache`, a capability beyond
  `supports_fork`): a static `--system` is NOT reused across separate harness
  processes (Claude Code re-creates a user-supplied `--append-system-prompt` every
  `claude -p`; only its own global prefix gets cross-process cache reads — verified
  live, three experiments). So on a harness whose fork reuses the cache (today
  **claude-code only**) the command layer's `run_fork_batch` runs the warm-up
  (prompt[0], establishing a session that carries `--system`), reads its
  `session_id`, then rewrites the fan-out jobs to `--resume <sid> --fork` (dropping
  `--system`, inherited from the session) so they reuse the warmed prefix.
  **OpenCode is fork-*capable* but `fork_reuses_cache: false`** — its `--fork`
  re-sends the branched conversation cold (fan-out reads no cache, re-writes the
  whole prefix — measured live, so forking it would *raise* tokens). Without a
  cache-reusing fork, `min-tokens` only orders the calls (a stderr warning — no
  reuse). `run_in_waves` covers `speed`/order-only; `run_fork_batch` the fork path.
  Only spawning is I/O, so the warm-then-fan ordering is unit-testable against the
  mock (`MOCK_LOG_FILE` records start/end interleaving; a mock `session_id` drives
  the fork-argv test). Each result carries its own `prompt`; the report's `batch`
  block carries `strategy`/`prompt_count`/`forked`. `--batch-strategy` is a
  per-invocation orchestration knob, so — unlike most `run` flags — it deliberately
  has no config/`ONEHARNESS_*` layer. The live drift alarm that the mode *reduces
  tokens* is `oh_batch_fork_enforce` in `e2e-claude.sh` (the fork fan-out reads the
  warmed prefix and writes less than the warm-up), tied to the `usage` cache counts.
  It is claude-only because claude-code is the only `fork_reuses_cache` harness;
  `e2e-opencode.sh` deliberately omits it (its fork doesn't reuse — see above).
  `list` and `detect` describe and probe
  the registry; `config` shows the effective layered configuration with each
  value's source; `sync` merges the unified policy settings (allow/deny rules,
  hooks, raw `settings` tables) into each harness's **own** config file — project
  by default, or the user-global location under `--global` (hooks only) — so the
  policy also applies without oneharness in the loop. Those five print a text
  view by default and the JSON contract under `--format json`/`--compact`.
  `history` (opt-in via `run --history` / `history` config /
  `ONEHARNESS_HISTORY`, off by default) streams a **standardized cross-harness**
  run history — one normalized record per harness run (the report's signals, no
  raw stdout/stderr) — to `<history_dir>/<project-slug>/<session>.jsonl` (one file
  per run; `history_dir` defaults to the platform state dir). It is its own output
  v0.2 contract with its own `domain::history::SCHEMA_VERSION` (independent of
  the report's): each record has a UUIDv7 `history_id` and validated `labels`
  (`history_labels` / `ONEHARNESS_HISTORY_LABELS` / repeated `--history-label`,
  merged by key with CLI > env > project > user precedence). v0.1 remains
  readable with a deterministic UUIDv5 id and empty labels. A validated input the
  SDKs also validate must be stated so the **Rust runtime check and the hand-written
  `JsonSchema` accept the same values** — the schema is the SDK validators' only
  source, so a gap there ships as an SDK that refuses what the CLI takes. Three
  traps, all live in `domain::history`: bound lengths in **characters** (code
  points — the only unit `maxLength` expresses; never bytes); spell a character
  allow-list as a **forbidden unanchored `not` search**, never an anchored
  `^…$` (Python's `re` `$` also matches before a trailing newline, so `"v\n"`
  passes the Python SDK); and keep `char::is_control` (Cc = C0 + DEL + **C1**) and
  its pattern in step. `HistoryId` accepts only canonical hyphenated text with the
  RFC 4122 variant and a defined version — `Uuid::parse_str` is laxer than the
  pattern promises. The shared `tests/fixtures/sdk-contract-matrix.json` is where
  such a rule gets pinned across Rust/Node/Python at once. The record shape +
  slug + name + timestamp formatting are pure (`domain::history`); the clock
  reads that mint the session id/timestamps and all file writes/reads are I/O
  (`io::history`). The writer is **best-effort** — a store that can't be opened or
  a record that can't be written warns on stderr and disables history for the run,
  never taking the results down (like the mock restore). The session `name` is
  oneharness-derived (a slug of the first prompt, or `--history-name`), NOT from
  the harness — headless harnesses expose only an opaque `session_id` (already
  captured per record), never a readable title; don't fabricate one. The report
  echoes the session file as `history_file` (the programmatic handle). The
  `oneharness history list/show/watch/reindex/clear` verb views/manages the store: a
  text view by default, `--format json` (the contract) for programs; `show` resolves a
  record UUID exactly before its back-compatible session id/name lookup; `watch`
  emits typed JSONL envelopes with label filters and `--after` cursor resume.
  The index is dated and append-only; its layout and which reader reads what
  are declared once in `docs/history-index.md`. **Never add an implicit
  whole-store read** — nothing but an explicit verb may scan. A test writing
  session files by hand runs `history reindex` before reading them back. `clear` is a dry run until `--yes`. History paths are canonicalized
  before writing so `cwd=..` remains discoverable. The **pointer file**
  (`--history-pointer-file`, layered like `history_dir`) is how a consumer finds
  a run's sessions without scanning the store. Three constraints are
  load-bearing. `domain::history::HistoryPointer` is the line's ONE declaration
  (the SDK types generate from it; the README table is test-pinned to it). The
  line is written as the run's id is minted, BEFORE anything spawns, once per
  plan entry the writer is handed — a chain candidate as it is reached, an
  already-`skipped` row too, since every entry closes as its own record — so
  the library path writes it exactly as the CLI does. Each line is one
  `O_APPEND` write with no lock, so concurrent processes never interleave; read
  it only through `io::history::read_pointers`, which skips a torn or foreign
  line rather than failing, and a line without its newline is torn even when it
  parses.
  <!-- llmlint: ignore-block[agents_md_durable_and_terse, no_redundant_instruction_pointers, comments_earn_their_place] Stating these load-bearing constraints here and deferring them to `docs/harness-usage.md` are the only two arrangements, and one rule in this list forbids each; they stay stated, with the pointer intact. `comments_earn_their_place` is listed because the span covers these directive lines too. -->
  `usage` is the pre-flight verb: subscription headroom per identity, on its own
  output contract, parsers pure (`domain::usage`) and probes I/O (`io::usage`).
  Four constraints are load-bearing; every observed payload and exchange behind
  them lives in `docs/harness-usage.md`. A probe must be **zero-turn** — no user
  message, no completed turn — because a pre-flight check that spends quota
  defeats itself. Each parser carries its own drift guard and degrades an
  unrecognized shape to *unknown*, because a confident wrong headroom number is
  silent where a crash is loud. A probe whose answer is **asynchronous** must
  hold the child's stdin open until that answer lands (`StdinAfterRequests`):
  codex's app-server drops an in-flight reply on EOF, which reported a readable
  45%-used window as unreadable for a whole release, and only the live
  `oh_usage_enforce` phase can catch it — a mock that answers inline cannot.
  Claude's null `rate_limits` under `rate_limits_available: true` is a
  **transient** (an expired or refreshing credential), not drift: the pure
  `claude_usage_snapshot_missing` names it, the probe asks again a bounded
  number of times inside its one deadline, and the reason says what the
  payload means — calling it "changed shape" sent a reader after a release
  break that did not exist.
  And the Cursor probe must keep masking
  `CURSOR_API_KEY` from its child: passing it authenticates rather than selects,
  a hazard any future Cursor dispatch also hits.
  <!-- llmlint: ignore-end[agents_md_durable_and_terse, no_redundant_instruction_pointers, comments_earn_their_place] -->
  A control socket ADDRESS is bounded, not merely a path: `sun_path` holds 108
  bytes on Linux and 104 on macOS, so `domain::control::socket_path` abbreviates
  a session name that would not fit (deterministically — `interrupt` is a
  different process resolving the same address) and refuses a store directory
  past the budget as a loud usage error before anything spawns, naming path,
  length and limit. A run whose channel cannot exist must never start; a
  graph-minted name one byte over made every dispatch on a host silently
  uninterruptible. `io::control::bind` re-checks after canonicalizing, which can
  lengthen an address the caller already cleared.
  `run --control` requires `--session` and one live TURN — exactly one harness in
  `parallel`, or a fallback chain of any length (it starts candidates one at a
  time), whose candidates may declare DIFFERENT mechanisms. The mechanism is
  **late-bound**: the socket address is the run's, and the serving candidate
  binds its own as it takes the turn and releases it when the turn ends. So
  `validate_control` may only refuse what is true of a candidate WHATEVER it
  does — no mechanism at all, an inexpressible mode, a format its mechanism pins
  differently — and never over the candidate SET where it means the serving one.
  Mixed mechanisms and a multi-candidate pooled-server chain are therefore both
  accepted: one candidate serves at a time, so one mechanism and one lease are
  live at a time. Every candidate takes the control delivery; only the session
  token stays the anchor's, since it is not portable between identities. Every
  candidate-wise refusal must hold for a candidate in ANY position, so each one
  needs a test with the offending candidate second. An interrupt reaches
  whatever is bound, and is `no_active_turn` when nothing is; a redirection its
  own turn never delivered is dropped there and said, never carried to the next
  candidate. Every violation is a loud usage error. For every control-capable
  harness and every `PermissionMode` that harness supports, a controlled run
  must be under exactly the policy the same mode gives without `--control` (the
  codex `bypass`→`workspaceWrite` bug was one cell of that grid). The way to
  keep it true is to DELIVER the harness's own mapping into the controlled
  launch rather than re-derive a posture for the protocol: copilot's permission
  flags ride the `--acp` argv beside it, goose's `GOOSE_MODE` already rides the
  control child's job env. Only where nothing can be delivered is a posture
  answered on the wire, and then it is the harness's own (`ModeSpec::posture`)
  rather than the spectrum's — which is why crush's ungated `default` is
  unattended under control too. A mode whose ONLY delivery is the harness's own
  config environment cannot reach a turn submitted to a pooled server, so
  opencode's `edit` is a **loud usage error** under `--control` and the approval
  mode stays out of the pool key. That is the feature's one **known gap**, named
  in both places a reader looks — its grid cell
  (`known-gap:mode-env-not-delivered-to-a-pooled-server`)
  and a phase `e2e-control.sh` reports rather than runs. Adding a harness or a mode means adding its cell to `control_mode_parity`. Declare `ControlShape` only after a live interrupt
  through oneharness. Stdin control keeps the child stdin open, then closes it
  on `is_turn_terminal`. Dialogue control owns its JSON-RPC child per dispatch:
  codex ends on `turn/completed`, not the `turn/start` response, and ACP must
  answer `session/request_permission`. The MODEL a driven turn negotiates is
  the CANDIDATE's own (`unit.model`, what the result and record report), never
  the run-level one, and it rides `thread/start`, `thread/resume` AND
  `turn/start`; the server's own answer (`ThreadStartResponse.model`, required;
  `turn/started` carries none) is read back as `observed_model`, and a
  difference is refused as `model_mismatch` BEFORE `turn/start` — a zero-cost
  classified failure naming both models, never a turn billed to the wrong one.
  Its live alarm, `oh_control_model_enforce`, delivers the model through a
  harness-scoped config key, since `--model` is the path that cannot show the
  defect. Dialogue-derived session ids are usable
  only under `--control` (`session_capable_under`) — but a `--session` handle
  under `--control` names the CHANNEL, and whether it also continues a
  CONVERSATION is the mechanism's own question (`ControlShape::carries_session`):
  a driven turn builds no argv, so `HarnessPlan::resume` is never reached and
  the protocol's own `resume_request` is the only route in. Codex has one
  (`thread/resume`, whose response carries the same `thread` field
  `thread/start` does); claude needs none (its frame rides the ordinary `-p`
  run). Over a mechanism with neither a *continue* is a loud usage error and a *create*
  warns that this handle will not continue — never a fresh conversation reported
  as a continuation, which the store cannot tell apart. The token is scoped to
  the session ANCHOR on this route exactly as on the argv one; a chain candidate
  that is not the anchor opens fresh. HTTP control submits turns
  through the pooled server, not the harness CLI: permission requests must be
  answered; opencode is terminal only on idle after admission; and cwd — plus
  opencode's MODEL, which its session-create route takes as a required
  provider+id pair — stays a per-turn value. A per-turn setting the wire has no
  place for is refused, never dropped: an opencode session opened without a
  model runs on whatever the server picks, and live that was a free model
  answering 401 on every turn. Its own config does not decide that — `opencode
  serve` loads a `model` from `OPENCODE_CONFIG_CONTENT` and creates sessions on
  another one anyway. Pool keys exclude all per-turn and per-thread settings.
  Readiness is a question about the PROCESS oneharness launched, never about who
  answers at its address: a TCP port is reserved by binding and letting go, so
  between the reservation and the launch it belongs to whoever asks the kernel
  next, and a run that took any answer could be driven against a stranger's
  server (which is how a hermetic control test read `timeout` at random). So a
  server that EXITED during bring-up is said so at once and relaunched once at a
  fresh address; one that is merely SILENT is reported against the window and
  never relaunched.
  `interrupt --input` carries a **redirection** with the abort. Atomic means
  *committed with the abort, delivered at the turn boundary*, never written
  alongside it: every mechanism drops or queues a message sent into a live turn,
  so the run parks it before the abort goes out, hands it back on any failure,
  and opens the next turn itself — through the same frame/route that opened the
  first one, which is why no declared mechanism has to refuse `--input`. So every
  backend must keep its turn (and stdin) OPEN while a redirection is pending; a
  mechanism whose terminal signal ends the run unconditionally would drop it.
  The interrupt is answered on **delivery**, not on the abort (#1405): it waits
  on a `RedirectDelivery` in `io::control` that only the writer of the
  replacement turn settles `Delivered` (stdin/ACP on the successful write,
  codex on its `turn/start` ack, HTTP on the session's admission); after a
  successful write the message is never handed back. A double records a stdin
  write only once it reads it, so journeys prove the write precedes the answer
  through `ONEHARNESS_INTERNAL_REDIRECT_TRACE` (the run appends
  `REDIRECT_WRITTEN` to it after the write), never through stdout. The
  redirect client has no read timeout: once a write is claimed only the run
  can settle it, which can take as long as the harness withholds its answer. Every path that ends a
  turn or a candidate without that write must settle it as undelivered
  (`no_active_turn` turn ended / `not_running` run gone) — a new backend that
  forgets leaves the supervisor waiting out the 60s deadline. A writer claims
  the ticket BEFORE writing and never writes an unclaimable one, which is what
  keeps a withdrawn message from landing.
  *When* the run learns the aborted turn ended is per mechanism and measured:
  most announce it, but **opencode announces nothing** — its stream just stops,
  so there the served interrupt is the ending and the message goes out as soon as
  the abort lands (`HttpShape::abort_ends_turn_silently`). Interrupting also
  makes the aborted turn's OWN submission fail (opencode answers its held-open
  prompt request with a refusal), and that refusal is not the run's outcome.
  `gate <id>` is the odd one out: the runtime pre-tool gate an
  installed `[[hooks]]` hook invokes, reading a harness's hook event on stdin and
  emitting its native deny verdict on stdout (pure shapes in `domain::gate`). It
  exists to prove a synced hook is *honored* end to end (the per-harness live
  e2e drives a real harness through it), not to be a policy engine — that is the
  sibling `allowlister`'s role, which consumes the `install` library. `mock
  <id>` is its read-write sibling for behavioral test suites (the `skilltest`
  consumer; design in `docs/mock-spy-design.md`): the same hook loop, driven by
  a `--rules` JSON ruleset — rules match on the tool name (`tool`/`tool_regex`),
  the raw event (`event_contains`/`event_regex`), and per-field `input`
  predicates (`equals`/`contains`/`regex` over `tool_input`), all ANDed and
  loud-validated (regexes are the linear-time `regex` crate, compiled at parse
  time) — that can *deny*, *rewrite the tool's input*, or *stub* a shell call
  (declare only the output; oneharness compiles it to a safely-quoted printf
  rewrite — nothing user-authored executes) and appends every
  observed event to a `--spy-file`/`ONEHARNESS_SPY_FILE` JSONL spy log, which
  preserves the *original* pre-rewrite call (the transcript `events` show only
  post-rewrite reality). Decision/verdicts are pure in `domain::mock`; the
  rewrite shape is per-harness registry data (`mock_rewrite`, all verified live
  by `oh_mock_enforce` and/or the `explore-hooks` probe: claude-code and codex
  `claude-nested` — codex's hooks engine needs the run to opt in via a `-c
  features.hooks=true --dangerously-bypass-hook-trust` passthrough — crush
  `crush-flat`, cursor `cursor-permission` (its `preToolUse` event, wired into
  the hook binding for this), opencode via the plugin shim's args merge;
  absent — a loud usage error — for goose, whose protocol can't rewrite, for
  copilot, whose hooks were probe-REFUTED headlessly (zero events under `-p`),
  and for qwen, whose documented `updatedInput` was live-REFUTED — hook fired,
  verdict emitted, original command still ran on all three OSes. Claude's
  documented PostToolUse `updatedToolOutput` replacement was also
  probe-refuted — fired, ignored — so there is no `replace` verb yet; opencode
  after-hook replacement is probe-verified and is where `replace` starts).
  `run --mock-rules <file>` / `run --spy-file <file>` is the single-flag
  ephemeral delivery: per-run argv for claude-code (`--settings` temp file,
  zero mutation), a snapshot-and-restore project-scope install for the rest
  (layers onto existing config via the non-destructive merge; created files
  deleted, created dirs pruned — `io::hooks::HookSnapshot`), codex's opt-in
  flags auto-appended (`MockDelivery` in the registry); qwen/copilot are
  refused loudly (no headless-capable delivery). `oh_mock_enforce` is the live
  drift alarm for both the verdict shape and the ephemeral delivery (it drives
  `run --mock-rules` and asserts zero residue), and it retries once when the
  spy log is empty (an agent refusal — the hook never fired — is flakiness,
  not verdict drift).
- **Structured output** (`run --schema <file>`): constrain each harness's final
  answer to a JSON Schema, validate it (the `jsonschema` crate, pinned
  `default-features = false` so it stays offline), and re-prompt on failure up to
  `--schema-max-retries` (default 2). Two deliveries, per `HarnessSpec.native_schema`:
  *native* where the CLI has a schema flag (only Claude Code's `--json-schema`
  today, value read from `structured_output`), *prompt-based* for the rest (the
  schema is appended to the prompt, the value recovered from the answer text).
  oneharness validates either way, so a native flag the harness ignores is still
  caught. The validate/retry loop lives in the runner as `run_jobs_with` (a pure
  domain closure decides re-runs; the runner owns spawning), so it stays parallel
  across harnesses. Pure logic — schema compile/validate, JSON extraction,
  instruction text, the shared `check` used by both the loop and the report — is
  in `domain::structured`. Like every normalized signal, the structured value is
  **never fabricated**: no extractable JSON is "invalid", not a guess. Codex's
  native `--output-schema` is deliberately *not* wired yet (file-based + ignored
  once tools run, https://github.com/openai/codex/issues/15451); adding it is one
  registry line plus a `build_argv` arm (the pointer comments at the codex
  registry entry and `structured::NativeSchema` say exactly what to change). The
  *per-feature* live e2e (`scripts/e2e-schema.sh` / `just live-schema` /
  `e2e-schema.yml`, helper `oh_schema_enforce`) drives real claude-code through
  `--schema` and asserts a schema-valid round-trip — the drift alarm for the
  native `--json-schema` flag the hermetic suite can only mock. That live check
  is Linux/macOS-only: a JSON Schema is quote-heavy and npm `.cmd` shims mangle
  quote-containing argv via cmd.exe `%*` on Windows (so structured output is
  unreliable against a `.cmd`-shim harness there — documented in the README; the
  hermetic `check` job still covers the Windows argv/validation path). The
  structured-output prompt additions stay **single-line** by convention; this is
  not a spawn constraint — `io::runner` now spawns a multi-line argument against
  a `.cmd`-shim harness by bypassing the shim (`domain::shim::parse_cmd_shim`
  rewrites it to the shim's real target: `node <cli.js>`, or the wrapped `.exe`
  directly, as for claude-code whose bin is `bin/claude.exe`) — but the cmd.exe `%*`
  quote-mangling above is a *separate* limitation the bypass does not touch
  (a quote-heavy schema is single-line, so it never triggers the bypass).
- **A controlled codex turn runs on the shared app-server** (`io::app_server`;
  the pure env/key/frame rules in `domain::app_server`, the deadlines beside
  `DEFAULT_LINGER` in `io::server_pool`) unless `app_server = "per-dispatch"`.
  The pool starts each entry's sweeper by re-running the current executable
  with `ONEHARNESS_INTERNAL_APP_SERVER_SWEEPER`, so a binary driving pooled
  dispatches calls `io::app_server::serve_sweeper_if_requested()` first in
  `main` (the CLI and the `app_server_reuse` example do); without it no
  sweeper starts and dead leases wait for the next pool operation. The sweeper,
  like the server, is the pool's and is started with a scrubbed environment
  (`domain::app_server::sweeper_env`): one carrying the dispatch's was reaped
  with it by launchers that end a tree by an environment stamp (onepipeline's
  `ONEAGENTGRAPH_SCRATCH_DIR`), leaving its server unwatched forever. The server's
  environment is scrubbed, so the mock's server-wide knobs are `CODEX_MOCK_*`
  (`MOCK_CODEX_*` knobs answer to that spelling too) and its per-thread knobs
  are the dispatch's own `MOCK_*`, read off `shell_environment_policy.set`; a
  journey scripting the stdio mock app-server runs under `per-dispatch`
  (`codex_per_dispatch` in `tests/cli.rs`). Its journeys:
  `crates/oneharness-e2e/tests/app_server.rs`. A server that dies before it
  listens is diagnosed from its stderr, kept in the entry's `server.log` and
  quoted in the dispatch's error. Real Codex binds every `--listen` socket
  in one rendezvous directory and exits 1 unless it is the user's at one mode;
  both are stated once, in `domain::app_server::codex_daemon_dir` /
  `CODEX_DAEMON_DIR_MODE`, which the launch (`prepare_daemon_dir`, tightening
  one that is ours) and the mock both read, and `oh_codex_rendezvous_check`
  holds to real Codex live. Never point a
  test at the host's directory: it is shared by every Codex on the machine,
  and a journey uses `ONEHARNESS_INTERNAL_CODEX_DAEMON_DIR` +
  `CODEX_MOCK_DAEMON_DIR` instead. Anything that outlives its
  dispatch spawns through `io::server_pool::detach`, which also marks every
  descriptor above stderr close-on-exec: on macOS `std` marks a new pipe or
  socket close-on-exec only after creating it, so a detached child otherwise
  keeps whatever it caught open — a caller's output pipe held this way never
  reaches end-of-file for whoever reads it. And on Darwin, `setsockopt` on a socket whose peer closed
  fails with EINVAL (`io::websocket::bound_reads`). Never let a waiter rely on
  a kernel signal Darwin does not give: on a Unix-domain stream whose peer
  shut its reading but still holds the socket, a write succeeds there (XNU
  drops the bytes; Linux says EPIPE), so a mock that "refuses a write" must
  END the connection; and a dead server's connection need not read EOF (a
  command it started can hold its end), so a pooled turn asks
  `ServerRecord::is_running` whenever it reads nothing, and `pid_alive` counts
  a zombie as gone on both kernels — on Darwin by the ESRCH `proc_pidinfo`
  gives a pid signal 0 still answers for (XNU's lookup skips an exited process
  unless asked for zombies). A short child the caller only bounds by its own
  deadline (a `--version` probe, the model catalog's refresh helper) goes
  through `Process::spawn_tethered`: a caller SIGKILLed mid-probe enforces no
  deadline, and `Process::spawn`'s own process group is out of reach of any
  signal sent to the caller's. Every Rust test runs under a nextest per-test bound (`.config/nextest.toml`): a hang
  fails as TIMEOUT by name.

## Adding or changing a harness

A new harness is a new entry in the registry
(`crates/oneharness-core/src/domain/harness.rs`) plus its `build_argv`/`extract`
functions — no changes to `run`, the runner, or the report
shape. When you add one:


- Add a `--print-command` assertion in `crates/oneharness-e2e/tests/cli.rs` pinning its exact argv
  (this is the deterministic, network-free proof the adapter is correct).
- Declare its `modes` (`HarnessSpec.modes`): one `ModeSpec` per
  [`PermissionMode`] the CLI can express, each tagged `clean` or `hangs`
  headless, sourced from that CLI's docs/behavior — never guessed. Every harness
  lists `bypass` and `default`. Map each in `build_argv` (or, when the mode is an
  environment variable like Goose's `GOOSE_MODE`, in `ModeSpec.env`), and prefer
  the cleanest non-interactive variant for `default` (e.g. a deny-and-continue,
  not an interactive prompt). A *behavioral* mode a harness can't express
  natively can be synthesized from **enforcement + an instruction**: set
  `ModeSpec.instruction` (prepended to the prompt by the command layer) and pair
  it with the enforcement `build_argv`/`env` provides — this is how Codex's
  `plan` works (read-only sandbox + a plan instruction). Only do this when the
  enforcement half exists (a plan instruction without read-only enforcement
  wouldn't stop the agent acting). Pin the mode→flag mapping with a `build_argv`
  assertion, and update the *Approval modes* table in `README.md`. For each
  no-mutation mode the harness supports (`read-only`, `plan`), add an
  `oh_mode_enforce <id> <mode>` phase to its `e2e-<id>.sh` (a write blocked under
  `--mode <mode>`, allowed under `--mode bypass`) — the live proof the mapping is
  honored and its drift alarm. If the harness's `edit` auto-approves a write that
  its non-edit posture would deny (copilot, qwen), add `oh_edit_enforce <id>` —
  under `--mode edit` a file-tool edit must succeed, the live proof the edit
  mapping is honored. Its "gate shell" half is NOT asserted live: it isn't
  reliably testable, because a model told to write via shell routes around any
  gate through whatever path the harness still allows (copilot auto-approves
  `echo`, opencode delegates to a `task` subagent, qwen's auto-edit ran it) — that
  half stays argv/env-pinned in the `domain::harness` tests. A mode delivered by
  environment (Goose's `GOOSE_MODE`, OpenCode's `OPENCODE_CONFIG_CONTENT`) is
  pinned hermetically via the mock harness's `MOCK_ECHO_ENV` instead. (`auto`
  likewise has no live drift-alarm: a deterministic cross-harness check would
  hinge on the classifier's model-dependent safe/risky split — it stays
  unit-pinned.)
- Update the harness table in `README.md` — including its config-support
  columns (`model`, `system`, bypass, allow/deny rules, hooks, output format,
  `--resume`), which document how each unified setting reaches (or doesn't
  reach) the harness — and the `supports_*` capability fields in the registry.
  Policy settings (`allowed_tools`/`denied_tools`/`hooks`/`settings`) are
  delivered by `oneharness sync` into the harness's own config file (the
  `SyncSpec` in the registry — file path + key paths, sourced from that CLI's
  docs, never guessed). They follow the loud-absence rule: no mapping means a
  parse error for `[harness.<id>]` fields and an `unmapped` entry in the sync
  report (plus a stderr warning) for top-level ones — never a silent drop. The
  sync merge is non-destructive by contract: unrelated keys untouched, lists
  unioned (idempotent re-sync), unparseable files refused and left intact,
  writes atomic. Keep those properties test-pinned when touching it. `--exact`
  (`SyncMode::Exact`) is the one opt-out, and only for the lists at
  `allow_path`/`deny_path`. A target whose rules are not JSON declares it by
  its file name (`SyncSpec::format`: Codex's `*.rules`), is owned whole, and
  translates each rule or reports it in `unmapped_rules` — never widened.
- Declare `supports_resume` / `supports_fork` and map them in `build_argv`,
  sourced from that CLI's headless docs (never guessed). *Resume* is the
  continuation flag (`--resume`, `--session`, or a subcommand like Codex's `exec
  resume <id>`); all current harnesses support it, so `supports_resume` is a
  drift-alarm for a future one that doesn't — when false, the command layer
  rejects `--resume` rather than silently starting fresh. *Fork* (`run --resume
  <id> --fork`) branches a new session from the resumed one and is rare — only
  Claude Code (`--fork-session`) and OpenCode (`--fork`) express it headlessly;
  the rest resume linearly, and `--fork` is a loud usage error for them (`fork`
  implies `resume`, clap-enforced). Pin each mapping with a `build_argv`/`--print-
  command` assertion, and remember the session-id round-trip: if the harness emits
  an id headlessly, teach `signals::extract_session` its field (Codex's
  `thread_id`); if it emits none (Goose, Copilot), the continuation handle is
  caller-supplied (a `--name` / minted UUID) and `session_id` stays `null` — never
  fabricate one. Update the `--resume` column in `README.md`. Also declare
  `session_formats` (every non-empty list implies `supports_resume`): the exact
  output formats that emit the native id, preferred automatic format first; an
  empty list means incapable. `oneharness list` derives `session_capable` from
  this list, so capability can never drift from the transport. The non-empty
  harnesses are exactly the `extract_session` sources — claude-code, opencode,
  codex, cursor, qwen — which is what lets the uniform
  `run --session <name>` handle map a caller-owned name to the harness's native
  token in the session store (`domain::session` decides create-vs-continue,
  `io::session` persists `<state>/oneharness/sessions/<slug>/<name>.json`; the
  command layer feeds a continue's token through the *existing verified* `--resume`
  mapping, so `--session` needs **no** new argv arm). With no explicit output
  format the command layer selects the first `session_formats` entry; an explicit
  CLI/config format still wins only if it appears in the list, otherwise it is a
  loud usage error before spawning. When the list is empty, `--session` is a loud
  usage error (no id to bind a name to) — never a silent fresh start. It is
  single-harness, refuses batch/`--resume`/`--fork`/`--all`, and echoes a `session`
  block `{name, phase, token, store_file}` in the report. The record binds to the
  **variant-qualified** id: a native token is scoped to one identity's session
  store (each variant is its own `env_from` home), and a base id cannot say which
  identity minted it. So `harness_conflict` compares the whole id, a legacy `0.1`
  record starts fresh rather than guessing, the token is captured from — and
  rebound to — the candidate that actually *ran*, and the fallback anchor prefers
  the identity the record already belongs to. A resume no identity can resolve is
  the `session_not_found` kind, which falls through beside `auth`/`quota`; its
  phrasings in `domain::signals` are captures from real CLIs (cursor's is
  deliberately missing, never guessed).
  <!-- llmlint: ignore-block[no_redundant_instruction_pointers] `README.md` is not in the agent-loaded instruction set (only this file is), so naming the sections that go stale is the instruction, not a redirect to one; dropping it is how the two documents drift. -->
  Update the *Session
  handle* section + `session_capable`/`--session` mentions in `README.md`.
  <!-- llmlint: ignore-end[no_redundant_instruction_pointers] -->
  Also
  declare `fork_reuses_cache` (implies `supports_fork`): true only if a forked run
  reuses
  the parent session's prompt-cache prefix, which is what makes a `min-tokens`
  batch save tokens — gate, **measured** by `oh_batch_fork_enforce` not guessed
  (true for claude-code; false for opencode, whose fork re-sends the prefix cold).
  When true, add the `oh_batch_fork_enforce <id>` live phase and update the batch
  support matrix in `README.md`.
- Set its `native_schema` only if the CLI has a real schema flag, sourced from
  that CLI's docs (never guessed) — and pin the injected argv with a
  `--print-command`/`build_argv` assertion. `None` is the right default: the
  prompt-based structured-output path already works for every harness, and
  oneharness validates the result regardless. If a harness reports its conforming
  value somewhere other than the answer text, extend `structured::extract_value`.
- Set its `reasoning` (`ReasoningDelivery`) only if the CLI takes a
  reasoning/thinking-effort setting **on the argv** headlessly, sourced from that
  CLI's docs (never guessed). Three shapes: `Flag("--effort")` for a dedicated
  flag (claude-code, copilot's `--reasoning-effort`), `ConfigKv("model_reasoning_effort")`
  for a `-c key=value` override (codex), or `ModelSuffix` when effort is a
  `-<tier>` suffix baked into the **model id** (cursor's `claude-opus-4-8` +
  `high` → `--model claude-opus-4-8-high`; cursor-agent rejects a bracketed
  `model[effort=…]` — verified live). The value is an **opaque string** the caller
  picks for their model and oneharness forwards verbatim (reasoning effort is a
  provider/model capability with no shared spelling — OpenAI's `reasoning_effort`
  enum vs. Anthropic's thinking-token budget — so it is per-harness delivery, not
  a normalized spectrum; an effort the model rejects surfaces as that harness's
  own `nonzero`, never a guess). `build_argv` is untouched: the command layer
  renders the delivery — appending `ReasoningDelivery::args` to the harness's
  override args (alongside config `args`/passthrough) for the flag/`-c` shapes, or
  decorating the resolved `--model` value via `ReasoningDelivery::model_suffix` for
  the model-suffix shape (which therefore needs a model — `ReasoningNeedsModel`, a
  loud usage error, when none is set; the recorded result `model` stays the plain
  id). It refuses (`ReasoningUnsupported`, a loud usage error) any selected harness
  with `reasoning: None` that has an effective `--reasoning`/config value — never a
  silent drop. `None` is the honest default (opencode/qwen/crush express effort
  only through their own config file — the `sync`-path follow-up; goose has no
  headless knob at all). Wired today: claude-code (`--effort`), codex
  (`-c model_reasoning_effort=`), copilot (`--reasoning-effort`), cursor
  (`ModelSuffix`). Pin the rendered argv with a `--print-command` assertion, add
  the `reasoning`/`supports_reasoning` column to the README matrix, resolve it per
  harness (`[harness.<id>] reasoning`, next to `model`, since effort values are
  provider-specific), and add the `oh_reasoning_enforce <id> <effort>` live phase —
  a real `--reasoning` run must complete cleanly (a bogus effort is best-effort
  evidence of honoring). That live phase matters most for copilot (a history of
  headless features silently not firing — its hooks were probe-refuted) and cursor
  (a forum report says cursor-agent may reject the very bracket syntax its `--help`
  advertises — the phase fails loudly if so). The config/env/CLI trio gained
  `reasoning` / `ONEHARNESS_REASONING` / `--reasoning`.
- Declare its `large_input` (`LargeInput`): how a **large** prompt/system reaches
  the harness without inlining it into the argv (past the OS ceiling → `E2BIG`;
  issue #1115). Three fields, all sourced from the CLI's headless docs, never
  guessed: `prompt_stdin` (the harness reads the user prompt from stdin — add a
  `c.prompt_stdin` arm to `build_argv` that omits the positional and adds any
  stdin-selecting flags, e.g. Claude's `--input-format text`, Goose's `-i -`);
  `system_rides_prompt` (for a harness with no system flag, whose `--system` is
  already prepended to the prompt — so the combined text rides the same stdin);
  and `system_file_flag` (a CLI flag that reads the system prompt from a file,
  Claude's `--append-system-prompt-file`). The command layer materializes/pipes
  only when a value clears the 64 KiB `LARGE_INPUT_THRESHOLD` (small prompts keep
  the byte-identical inline argv, so `--print-command` is unchanged); `build_argv`
  reads the `BuildCtx::system_file`/`prompt_stdin` fields. Pin the stdin/file arms
  with a `build_argv` assertion, add the harness to the README large-prompt
  matrix, and add the `oh_long_prompt_enforce <id>` live phase (a >128 KiB
  prompt+system must round-trip and stay out of `.command`) — the drift alarm that
  the CLI still reads the off-argv input. `LargeInput::NONE` (inline only) is the
  honest default until a stdin/file route is *verified* from a real invocation —
  a large value then stays inline and the command layer warns loudly rather than
  risking a silent E2BIG. All eight harnesses are wired today (cursor's
  stdin-only-prompt path was closed-source, so it was **probe-verified** via
  `scripts/explore-cursor-stdin.sh` + the dispatch-only `explore-cursor-stdin.yml`
  before wiring — the pattern to reuse for the next uncertain CLI).
  <!-- llmlint: ignore-block[no_redundant_instruction_pointers, agents_md_durable_and_terse, comments_earn_their_place] This bullet can state the two rules an adapter author must satisfy or defer them to `docs/harness-usage.md`, and one rule in this list forbids each; it keeps the minimum, with the pointer intact. `comments_earn_their_place` is listed because the span covers these directive lines too. -->
- Declare its `usage` (`UsageSupport`). Every harness must report an honest tier:
  one that cannot report headroom says *which kind* of cannot (no plan quota at
  all, versus a quota with no non-interactive reader), never a `0%` and never an
  omission. A probing tier requires a zero-turn probe sourced from a real
  capture; a probe that sends a user message or completes a turn is disqualified.
  <!-- llmlint: ignore-end[no_redundant_instruction_pointers, agents_md_durable_and_terse, comments_earn_their_place] -->
<!-- llmlint: ignore-block[no_redundant_instruction_pointers] The capability matrix is per-harness data that lives in `README.md` (like the mode, resume, and events tables above); naming the file an adapter author must edit is the instruction, not a deferral of one. -->
- Declare `control` ([`ControlShape`]) only after `scripts/explore-control.sh
  <id>` and `oh_control_enforce <id>` prove a filesystem-level interrupt through
  oneharness; `None` is the default. A new shape must also source how a
  redirection reaches it (the frame/route that opens a turn on the session it
  just aborted) and add `oh_control_redirect_enforce <id>` — the live proof the
  redirected turn actually runs. Declare its `resume_request` from the CLI's own
  protocol schema, or `None` — a shape that drives its turn and cannot be asked
  to resume refuses a `--session` continue rather than starting over quietly.
  Keep the probe tables, registry, live suite,
  and README matrix aligned. A sidecar also declares `server` ([`ServerSpec`]).
  Its pool key excludes per-turn and per-thread settings; membership is a lease
  naming a live process identity, never a counter or a bare pid.
<!-- llmlint: ignore-end[no_redundant_instruction_pointers] -->
- Give the harness its `global_hook` (the user-global hook location, for `sync
  --global` / `install` at `Scope::Global`) and its `gate_deny` (how it expresses
  a pre-tool deny when it runs `oneharness gate <id>`). Both are registry data
  sourced from the allowlister adapters, never guessed; both are loud when absent
  (a missing `gate_deny` makes `oneharness gate <id>` a usage error). Pin the new
  deny shape with a `--print`-style assertion in `domain::gate`/`crates/oneharness-e2e/tests/cli.rs`.
  Likewise declare `mock_rewrite` (how `oneharness mock <id>` expresses an
  input-rewrite verdict) ONLY once verified — doc-source the shape, pin it in
  `domain::mock` + the registry test, and add the `oh_mock_enforce <id> [scope]
  [run-args…]` live phase (the rewritten command runs, the original doesn't,
  the spy log keeps the original event; forward any opt-in flags the harness's
  hooks engine needs, as codex's phase does); leave it `None` (a loud usage
  error) until the `explore-hooks` probe proves the CLI honors it headlessly.
- Source the real invocation from a known-good driver — the
  `nickderobertis/allowlister` repo's `run_agent()` / `e2e-*.sh` drivers are the
  reference — rather than guessing flags. (`scripts/smoke.sh --live` here is the
  fast way to confirm a real invocation actually works once installed.)
- If the harness's oneharness output format carries a machine-readable **tool
  transcript** (OpenCode's `tool` parts, or the Anthropic content-block stream a
  `stream-json` harness emits — see `extract_events` in `domain::events` and the
  README `events` docs), the normalized `events` array works for free once the
  shape is recognized. A harness with a *new* transcript shape needs a recognizer
  arm in `extract_events` (sourced from a real transcript, never guessed) plus a
  unit test; then add the `oh_events_assert <id> <source> [run-args…]` live phase
  — a tool-using turn must surface at least one `tool_call` event — the honoring
  proof + drift alarm. Events need a **transcript-carrying output format**, which
  `--events`/`--stream` selects per harness via `HarnessSpec.events_format`
  (must not break text extraction — verified live). **Never guess a shape: source
  it from a real transcript** — the `scripts/explore-events.sh` + dispatch-only
  `explore-events.yml` probe dumps every harness's live output to CI logs (run it
  from the Actions tab), which is how the current four recognizers were written;
  re-run it when adding a harness. Coverage today
  (all sourced, all e2e drift-alarmed): opencode (`json`, default),
  cursor (`stream-json`, default, its own `type:"tool_call"` shape), claude-code
  (`--events`→`stream-json`, Anthropic content blocks), codex (`exec --json` by
  default, `command_execution` items), qwen (`--events`→`stream-json`, content
  blocks). Goose/crush/copilot emit only decorative TUI text headlessly (probe-
  confirmed), so `events` stays `null` — correct, not a gap. Forward `--events`
  (or `--stream`) to `oh_events_assert`/`oh_stream_assert` as a run-arg for a
  harness whose transcript needs the upgraded format. Streaming
  (`run --stream`, `io::runner::run_job_streaming` + `events::events_from_value`)
  emits events incrementally so a consumer can short-circuit on bad behavior;
  its lines are the typed Rust `RunStreamEnvelope` contract and
  `oh_stream_assert` is its live proof. It is single-unit only in `parallel` —
  one harness, one model (interleaving); a **fallback chain streams** over both
  axes, since its (harness, model) candidates run in turn. So the streamed
  history attribution is per plan entry, not per selected harness (a model
  fan-out repeats a harness). The constraint is narrower than "one harness":
  stdout must never be committed
  to a candidate the chain then discards. So a fall-through is decided by
  `fallback::RunWork` first — a candidate whose result carries a tool call or
  billed usage (`signals::Usage::reports_billed_work`, the one definition
  `record_work_evidence` also reads a raw record with) ran the task and never
  falls through, whatever its terminal record then says.
  Both drivers read that evidence from the same normalized result, so **streamed
  and buffered chains always select the same candidate**; there is no
  streaming-only rule, and a published line is never retracted (a consumer acts
  on what it reads). `sdk_schema::bundle` is the single Rust
  generation source for that envelope, `HistoryStreamEnvelope`, and the shared
  SDK contracts.
- Everything else in `startup_failure_reason` follows one question — *could this
  candidate have run the task at all?* — never *what did the outcome look
  like?*; classifying by outcome is this module's recurring category error. The
  two **precondition** refusals (`untrusted_directory`,
  `input_too_large`) are that question answered by the harness's own pre-request
  check, which is why they fall through beside `auth`/`quota`. When a refusal
  names its cause in machine-readable terms, that text is carried up
  **verbatim** (`FailureReading::detail` → the result's `error` → the
  `fell_through` entry's `detail`), never paraphrased: a caller shards against
  codex's own `max_chars`, and a consumer three retries downstream reporting a
  schema-validation symptom is what the discarded cause cost. Adding a
  `FailureKind` is three edits in lockstep — the variant (and
  `FailureKind::ALL`), the history version gate
  (`history::gated_failure_kind_version`, which `sdk_schema` builds the SDK
  gates from so the two validators cannot disagree), and a
  `tests/fixtures/sdk-contract-matrix.json` case at the introducing version and
  the one before it. A failure that answers *nothing* still says what it did:
  `RunResult::work` publishes the same `RunWork` the verdict consulted (in the
  record too, gated at history v1.7), and `fallback.stopped_without_work` names
  it where a chain stopped there. It moves no verdict — an unclassified
  candidate still stops the chain, because re-running a task that may genuinely
  have failed for free burns the next identity's quota — but without it a
  candidate that never started reads exactly like one that ran the task and
  lost, and a chain that stopped at the first says nothing about which it was.
  `rate_limit` falls through on ANY chain, not only a model fan-out: the limit
  belongs to whoever is being billed, not to the model, so the next identity
  carries its own.
  `model_not_found` stays model-list-only — a config mistake the user should see
  — except a model the harness's *account* refuses (Codex's `400
  invalid_request_error` "not supported when using Codex with …",
  `signals::model_refused_for_account`), which `io::run::fallback_step` routes
  to `fallback::account_model_refusal_reason` instead: it hands on on any chain
  and from a clean exit too, while `startup_failure_reason` keeps every other
  `model_not_found` non-zero-only and model-list-only. The kind cannot tell the two
  apart, so that predicate re-reads the terminal record the classifier read.
- **A completed run that did the work carries no refusal classification.**
  `status: ok` + exit `0` + [`RunWork::Done`] refutes any `failure_kind` naming a
  refusal, so `RunResult::with_work_evidence` — the funnel every constructor
  already ends at — drops it there (`report::completed_run_that_did_work`, which
  reads work, not billing). That funnel is the single site: `history`'s validity
  rule is opt-in and best-effort, so refusing the record there would leave the
  stdout report carrying the refusal. `FailureKind::is_refusal` is matched
  exhaustively, so a new kind answers deliberately; `tool_deferred` answers no.
