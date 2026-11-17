//! The pure protocol state machines behind server-backed turn control.
//!
//! Claude Code's control rides its ordinary `-p` run: one prompt frame in, a
//! `result` document out. Every other controllable harness instead speaks a
//! **JSON-RPC protocol on stdio** (`codex app-server`, `copilot --acp`,
//! `goose acp`), where driving one turn is a conversation: handshake, open a
//! thread/session, send the prompt, answer whatever the server asks, and read
//! the turn's end off the stream. The interrupt then rides that same open stdin.
//!
//! A [`Dialogue`] is that conversation as a pure state machine: it is fed one
//! stdout line at a time and answers with the lines to write next. No I/O, no
//! clock — so every protocol rule below is unit-testable against a recorded
//! exchange instead of only against a live provider.
//!
//! Two rules here are load-bearing and were learned the hard way from
//! `scripts/explore-control.sh`:
//!
//! * An ACP client **must answer `session/request_permission`**. Goose and
//!   Copilot block indefinitely and never begin work until it does, which reads
//!   exactly like a slow or broken harness.
//! * ACP cancel is a **notification**, never a request. Sent with an `id`, goose
//!   answers `-32601 Method not found` and the turn keeps running.

use serde::Deserialize;
use serde_json::{json, Value};

use crate::domain::control::{AbsolutePath, ControlShape, RedirectInput};
use crate::domain::mode::{ApprovalPosture, PermissionMode};
use crate::domain::signals::Usage;

/// What a run needs to open a turn over one of these protocols. The command
/// layer resolves each value exactly as it would for an ordinary run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DialogueConfig {
    /// The assembled user prompt (system prompt already folded in where the
    /// harness has no separate channel for one).
    pub prompt: String,
    /// The working directory the turn runs in. Absolute by type: the server is
    /// a separate process that would resolve a relative path against its own
    /// directory, which is silently a different place.
    pub cwd: AbsolutePath,
    /// The model to request, when one was resolved — the **candidate's own**
    /// (`[harness.<id>].model`, a variant's `model`, or the run-level `--model`
    /// that resolution already folds in), never the run-level value alone. It
    /// rides every frame that takes one: `thread/start` and `thread/resume`
    /// open the conversation under it, and `turn/start` restates it
    /// (`TurnStartParams.model`, "override the model for this turn and
    /// subsequent turns") so the turn cannot run under whatever the thread was
    /// created with. What the server then says it will run under is read back
    /// into [`Dialogue::observed_model`], and a difference is refused before the
    /// turn starts ([`Dialogue::refusal`]).
    pub model: Option<String>,
    /// The approval mode the turn runs under, for the protocols that name it on
    /// the wire (codex's per-turn `SandboxPolicy` is derived from it). What the
    /// client ANSWERS a permission request with is `posture` below, which is the
    /// harness's own reading of this mode rather than the spectrum's.
    pub mode: PermissionMode,
    /// The conversation this turn continues, or `None` to open a fresh one.
    ///
    /// A driven turn negotiates everything on the wire, so this is the *only*
    /// route a `--session` handle has into it: without it the conversation is
    /// re-created every turn while the session store still looks healthy.
    pub resume: Option<DialogueResume>,
    /// What the harness's own run does about approvals in `mode`
    /// ([`ModeSpec::posture`](crate::domain::harness::ModeSpec)).
    ///
    /// Carried rather than derived from `mode`, because the posture a driven
    /// turn must answer with is the one that mode gives *without* `--control`,
    /// and only the harness's own registry entry knows it — the two ACP
    /// harnesses share one [`ControlShape`] and do not share a mapping.
    pub posture: ApprovalPosture,
    /// The dispatch's own `shell_environment_policy`, carried as the
    /// thread-level `config` map codex's `thread/start` and `thread/resume`
    /// send when the turn runs on a shared app-server, so its thread's
    /// commands see its environment rather than the server's. `None` on a
    /// dispatch's own child, whose process environment already is the
    /// dispatch's. `turn/start` has no `config` map, so it never rides there.
    pub shell_environment_policy: Option<crate::domain::app_server::ShellEnvironmentPolicy>,
}

/// The conversation a driven turn reopens, carrying the request that reopens it.
///
/// The request travels WITH the token rather than beside it, so a token paired
/// with a mechanism that cannot be asked to continue is not a state this type
/// can hold: [`Self::new`] yields `None` for a shape whose protocol has no
/// [`ControlShape::resume_request`], which is the same set the command layer
/// refuses `--session` continues for before anything spawns.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DialogueResume {
    request: &'static str,
    thread: String,
}

impl DialogueResume {
    /// `thread` reopened over `shape`, or `None` when that protocol has no
    /// resume request — there is no such thing as continuing a conversation
    /// over it, so there is no value to build.
    #[must_use]
    pub fn new(shape: ControlShape, thread: String) -> Option<Self> {
        shape
            .resume_request()
            .map(|request| DialogueResume { request, thread })
    }

    /// The client request that reopens this conversation.
    #[must_use]
    pub fn request(&self) -> &'static str {
        self.request
    }

    /// The harness's native identifier for the conversation.
    #[must_use]
    pub fn thread(&self) -> &str {
        &self.thread
    }
}

/// A codex turn identifier as the server named it: never empty, and a type of
/// its own so a thread id cannot be filed among a dialogue's turns.
#[derive(Debug, Clone, PartialEq, Eq)]
struct TurnId(String);

impl TurnId {
    fn new(id: &str) -> Option<Self> {
        (!id.is_empty()).then(|| Self(id.to_string()))
    }
}

/// One `TokenUsageBreakdown` from codex's app-server: the counts oneharness
/// normalizes into the same [`Usage`] fields a `codex exec` turn's
/// `turn.completed` fills (`inputTokens` includes `cachedInputTokens`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct CodexTokenTotals {
    input: u64,
    cached_input: u64,
    output: u64,
}

impl CodexTokenTotals {
    /// The breakdown's three counts, each required by codex's schema; `None`
    /// when any is missing or not a non-negative integer.
    fn from_breakdown(breakdown: &Value) -> Option<Self> {
        let count = |key: &str| breakdown.get(key).and_then(Value::as_u64);
        Some(Self {
            input: count("inputTokens")?,
            cached_input: count("cachedInputTokens")?,
            output: count("outputTokens")?,
        })
    }
}

/// What the driver should do after one line.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum DialogueStep {
    /// Keep driving the turn, writing these newline-terminated frames first.
    #[default]
    Continue,
    /// Keep driving the turn after writing these newline-terminated frames.
    Send(Vec<String>),
    /// The turn is over; the driver stops reading and tears the server down.
    Terminal,
}

impl DialogueStep {
    #[cfg(test)]
    fn frames(&self) -> &[String] {
        match self {
            Self::Send(frames) => frames,
            Self::Continue | Self::Terminal => &[],
        }
    }

    #[cfg(test)]
    fn is_terminal(&self) -> bool {
        *self == Self::Terminal
    }
}

/// Why a dialogue ended before it opened a turn — a refusal oneharness itself
/// answered off the server's own pre-turn statements, at zero cost.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DialogueRefusal {
    /// The server said the conversation would run under `observed`, and the run
    /// asked for `requested`. Compared byte for byte: a model id is the
    /// provider's own token, and oneharness has no aliasing table to guess with.
    ModelMismatch { requested: String, observed: String },
}

impl DialogueRefusal {
    /// The refusal as the result's `error`, naming `harness_id` as the party
    /// that would have run the turn, with both models spelled exactly as the
    /// config and the server spelled them.
    #[must_use]
    pub fn error(&self, harness_id: &str) -> String {
        match self {
            DialogueRefusal::ModelMismatch {
                requested,
                observed,
            } => format!(
                "harness `{harness_id}` reported it would run this turn under \"{observed}\", \
                 not the requested \"{requested}\", so the turn was refused before it started"
            ),
        }
    }
}

/// How far along one turn's conversation is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    /// Waiting for the handshake response.
    Handshake,
    /// Waiting for the thread/session to be created.
    Opening,
    /// The prompt is out; the turn is in flight.
    InTurn,
    /// The turn ended (completed, failed, or interrupted).
    Done,
}

/// One turn driven over a JSON-RPC stdio protocol.
#[derive(Debug, Clone)]
pub struct Dialogue {
    shape: ControlShape,
    config: DialogueConfig,
    phase: Phase,
    next_id: u64,
    /// The prompt request id, whose response ends ACP turns but only acknowledges
    /// Codex turn creation; Codex ends on its later `turn/completed` notification.
    turn_request_id: Option<u64>,
    handshake_id: u64,
    open_id: u64,
    /// The harness's own session/thread identifier, once it exists.
    session_id: Option<String>,
    /// The model the server said the conversation would run under, once it
    /// said so — codex's `ThreadStartResponse.model` / `ThreadResumeResponse.model`,
    /// both required strings in its own schema. Read off the open response,
    /// before any turn is submitted, and never derived from the request.
    observed_model: Option<String>,
    /// Why this dialogue ended without opening a turn, when it did.
    refusal: Option<DialogueRefusal>,
    /// Codex's per-turn identifier, which `turn/interrupt` addresses.
    turn_id: Option<String>,
    /// Every turn this dialogue opened, the redirected one included — the
    /// turns whose token usage is this run's.
    own_turns: Vec<TurnId>,
    /// The thread's cumulative token totals the server reported before any of
    /// this dialogue's turns (its replay on `thread/resume`), subtracted so a
    /// continued conversation's earlier turns are not billed to this run.
    usage_baseline: Option<CodexTokenTotals>,
    /// The thread's cumulative token totals as of this dialogue's latest turn.
    usage_latest: Option<CodexTokenTotals>,
    /// Assistant text accumulated from the update stream.
    text: String,
    /// The message the accumulated text is currently coming from, so two
    /// separate assistant messages do not run together into one word.
    text_item: Option<String>,
    /// The redirection an interrupt committed, waiting for the turn it aborted
    /// to actually end.
    ///
    /// Neither protocol takes a second turn on a thread that already has one —
    /// codex answers `turn/start` with an error and ACP's `session/prompt`
    /// queues behind the live one — so the message is held here from the moment
    /// the interrupt is served until the aborted turn's own end, which is the
    /// first point a new turn can be opened. Held, never re-sent: it is taken
    /// once, so a turn that ends badly costs one attempt rather than looping.
    pending_redirect: Option<RedirectInput>,
    /// What the server answered the turn a redirection opened, once it has —
    /// read once by the run, which answers the interrupt from it.
    ///
    /// Codex acknowledges `turn/start` at once, so its answer says whether the
    /// replacement turn exists; ACP answers `session/prompt` only when the turn
    /// ends, so it has none to wait for (see [`Self::awaits_redirect_answer`]).
    redirect_answer: RedirectAnswer,
}

/// Where the server's answer to a redirected turn's request is.
#[derive(Debug, Clone, PartialEq, Eq)]
enum RedirectAnswer {
    /// No redirected turn is waiting on an answer.
    None,
    /// The redirected turn's request is out; its answer has not arrived.
    Awaited,
    /// The server answered it: the turn exists, or why it does not.
    Answered(Result<(), String>),
}

impl Dialogue {
    /// A dialogue for `shape`, or `None` when the shape needs none (Claude
    /// Code's control rides its ordinary run, which is not a conversation).
    #[must_use]
    pub fn new(shape: ControlShape, config: DialogueConfig) -> Option<Self> {
        matches!(
            shape,
            ControlShape::CodexAppServer | ControlShape::AcpCancel
        )
        .then(|| Dialogue {
            shape,
            config,
            phase: Phase::Handshake,
            next_id: 1,
            turn_request_id: None,
            handshake_id: 0,
            open_id: 0,
            session_id: None,
            observed_model: None,
            refusal: None,
            turn_id: None,
            own_turns: Vec::new(),
            usage_baseline: None,
            usage_latest: None,
            text: String::new(),
            text_item: None,
            pending_redirect: None,
            redirect_answer: RedirectAnswer::None,
        })
    }

    /// The frames that open the conversation, written before any line is read.
    pub fn open(&mut self) -> Vec<String> {
        let id = self.take_id();
        self.handshake_id = id;
        match self.shape {
            ControlShape::CodexAppServer => vec![request(
                id,
                "initialize",
                json!({
                    "clientInfo": {
                        "name": "oneharness",
                        "title": "oneharness",
                        "version": env!("CARGO_PKG_VERSION"),
                    }
                }),
            )],
            // ACP's `initialize` announces what the *client* can do. oneharness
            // reads and writes files itself, so it advertises neither: a server
            // that believes otherwise would wait on a filesystem request nobody
            // is listening for.
            _ => vec![request(
                id,
                "initialize",
                json!({
                    "protocolVersion": 1,
                    "clientCapabilities": {
                        "fs": {"readTextFile": false, "writeTextFile": false}
                    },
                }),
            )],
        }
    }

    /// Advance the conversation with one stdout line.
    pub fn on_line(&mut self, line: &str) -> DialogueStep {
        let trimmed = line.trim();
        if trimmed.is_empty() || self.phase == Phase::Done {
            return DialogueStep::default();
        }
        let Ok(message) = serde_json::from_str::<Value>(trimmed) else {
            return DialogueStep::default();
        };
        let method = message.get("method").and_then(Value::as_str);
        let id = message.get("id");
        match (method, id) {
            // A server->client request: answering is mandatory, or the turn
            // stalls forever waiting on a client that never replies.
            (Some(method), Some(id)) => {
                let params = message.get("params").cloned().unwrap_or(Value::Null);
                self.on_server_request(method, id.clone(), &params)
            }
            (Some(method), None) => self.on_notification(method, &message),
            (None, Some(id)) => self.on_response(id.as_u64(), &message),
            (None, None) => DialogueStep::default(),
        }
    }

    /// The frames that abort the in-flight turn, or `None` when there is no
    /// turn to abort (before the prompt, or after the turn ended).
    ///
    /// `redirect` is the message to deliver with the abort. It is committed only
    /// when the abort itself could be written — a `None` here means nothing was
    /// aborted, so nothing may be queued behind it either.
    pub fn interrupt(&mut self, redirect: Option<&RedirectInput>) -> Option<Vec<String>> {
        if self.phase != Phase::InTurn {
            return None;
        }
        let frames = match self.shape {
            ControlShape::CodexAppServer => {
                let thread = self.session_id.clone()?;
                let turn = self.turn_id.clone()?;
                let id = self.take_id();
                vec![request(
                    id,
                    "turn/interrupt",
                    json!({"threadId": thread, "turnId": turn}),
                )]
            }
            // A NOTIFICATION, not a request. Sent with an `id`, goose answers
            // `-32601 Method not found` and the work carries on.
            _ => {
                let session = self.session_id.as_ref()?;
                vec![notification(
                    "session/cancel",
                    json!({"sessionId": session}),
                )]
            }
        };
        // Taken over only now that the abort exists: a caller whose interrupt
        // was refused above still holds its message, rather than having handed
        // it to a turn that was never stopped.
        if let Some(redirect) = redirect {
            self.pending_redirect = Some(redirect.clone());
        }
        Some(frames)
    }

    /// Whether a redirection is committed and waiting for the aborted turn to
    /// end. The run reads this to tell a redirect that is still in flight from
    /// one the conversation has already opened its new turn for.
    #[must_use]
    pub fn has_pending_redirect(&self) -> bool {
        self.pending_redirect.is_some()
    }

    /// Give back a redirection whose abort could not be written after all, so
    /// the supervisor still owns a message no turn is going to carry.
    pub fn abandon_redirect(&mut self) {
        self.pending_redirect = None;
    }

    /// Whether the turn a redirection opened is still waiting for the server
    /// to say it exists. Only codex says so — its `turn/start` is acknowledged
    /// at once — so only a codex redirection is delivered on that answer
    /// rather than on the write.
    #[must_use]
    pub fn awaits_redirect_answer(&self) -> bool {
        self.redirect_answer == RedirectAnswer::Awaited
    }

    /// The server's answer to the turn a redirection opened, once: `Ok` when it
    /// took the turn, `Err` with its own words when it refused it.
    pub fn take_redirect_answer(&mut self) -> Option<Result<(), String>> {
        match std::mem::replace(&mut self.redirect_answer, RedirectAnswer::None) {
            RedirectAnswer::Answered(answer) => Some(answer),
            other => {
                self.redirect_answer = other;
                None
            }
        }
    }

    /// The shape this conversation was built for, and the only protocol it
    /// speaks: every frame it writes and every line it accepts is that shape's.
    /// A caller binding it can therefore read the mechanism off the dialogue
    /// instead of carrying a second copy that could disagree with it.
    #[must_use]
    pub fn shape(&self) -> ControlShape {
        self.shape
    }

    /// The thread and turn an interrupt would address right now — `Some` only
    /// while a turn is in flight and the server has named it. This is
    /// oneharness's own record of `turn/start`, `turn/started` and
    /// `turn/completed`; the server cannot be asked whether a turn is active.
    #[must_use]
    pub fn active_turn(&self) -> Option<(&str, &str)> {
        (self.phase == Phase::InTurn)
            .then(|| self.session_id.as_deref().zip(self.turn_id.as_deref()))
            .flatten()
    }

    /// Whether the prompt is out and its turn has not ended: the window in
    /// which losing the server loses the turn.
    #[must_use]
    pub fn in_turn(&self) -> bool {
        self.phase == Phase::InTurn
    }

    /// The harness's native session identifier, once the server minted one.
    #[must_use]
    pub fn session_id(&self) -> Option<&str> {
        self.session_id.as_deref()
    }

    /// The model the server itself reported the conversation would run under,
    /// once it reported one — `None` on a protocol that names none (ACP) and
    /// on an open response that carried no `model`. Never inferred from the
    /// request: this is the harness's claim or nothing.
    #[must_use]
    pub fn observed_model(&self) -> Option<&str> {
        self.observed_model.as_deref()
    }

    /// Why the conversation ended **without opening a turn**, when it did —
    /// `None` on every dialogue that submitted (or is still to submit) one.
    #[must_use]
    pub fn refusal(&self) -> Option<&DialogueRefusal> {
        self.refusal.as_ref()
    }

    /// The assistant text accumulated from the update stream, or `None` when
    /// the turn produced none. Never fabricated.
    #[must_use]
    pub fn text(&self) -> Option<String> {
        let text = self.text.trim();
        (!text.is_empty()).then(|| text.to_string())
    }

    /// How [`Self::text`] was obtained, for the report's `text_source`.
    #[must_use]
    pub fn text_source(&self) -> &'static str {
        match self.shape {
            ControlShape::CodexAppServer => "jsonrpc:codex-app-server",
            _ => "jsonrpc:acp-session-update",
        }
    }

    fn take_id(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    /// Whether this run's approval posture allows the server to act — the
    /// harness's own answer for this mode, so a driven turn is under the policy
    /// the same mode gives without `--control`.
    ///
    /// A permission request is a *backstop* on the harnesses whose own mapping
    /// already reaches the protocol server: goose's `GOOSE_MODE` is injected
    /// into the ACP child's environment, and copilot's permission flags ride
    /// the `--acp` argv beside it. So this answers what that mapping did not
    /// already decide, and declining is the safe way to answer one — notably
    /// for `edit`, which promises auto-approved edits with shell still gated
    /// and whose ask carries no sourced way to tell those apart.
    fn permits_action(&self) -> bool {
        self.config.posture.is_unattended()
    }

    fn on_server_request(&mut self, method: &str, id: Value, params: &Value) -> DialogueStep {
        let allow = self.permits_action();
        let result = match self.shape {
            // Approve only what is actually an approval request. A permissive
            // mode means "act without asking", not "say yes to any method the
            // server invents" — and a method this build does not recognize is
            // still *answered*, because an unanswered request stalls the turn.
            ControlShape::CodexAppServer if is_codex_approval(method) => {
                json!({"decision": if allow { "approved" } else { "denied" }})
            }
            ControlShape::CodexAppServer => json!({}),
            // ACP: select one of the server's OWN options — the protocol offers
            // no way to narrow a grant, so the choice is which option, not how
            // much of it.
            _ if method == "session/request_permission" => {
                let chosen = allow
                    .then(|| acp_allow_option(params.get("options").unwrap_or(&Value::Null)))
                    .flatten();
                match chosen {
                    Some(option_id) => {
                        json!({"outcome": {"outcome": "selected", "optionId": option_id}})
                    }
                    None => json!({"outcome": {"outcome": "cancelled"}}),
                }
            }
            // An unrecognized request still gets an answer: both ACP harnesses
            // block indefinitely on an unanswered one, so silence is the one
            // reply that guarantees the turn never starts.
            _ => json!({}),
        };
        DialogueStep::Send(vec![response(id, result)])
    }

    fn on_notification(&mut self, method: &str, message: &Value) -> DialogueStep {
        let params = message.get("params").unwrap_or(&Value::Null);
        match (self.shape, method) {
            (ControlShape::CodexAppServer, "turn/started") => {
                self.turn_id = params
                    .get("turn")
                    .and_then(|turn| turn.get("id"))
                    .and_then(Value::as_str)
                    .map(str::to_string);
                self.own_current_turn();
                DialogueStep::default()
            }
            (ControlShape::CodexAppServer, "thread/tokenUsage/updated") => {
                self.absorb_codex_token_usage(params);
                DialogueStep::default()
            }
            (ControlShape::CodexAppServer, "item/agentMessage/delta") => {
                if let Some(delta) = params.get("delta").and_then(Value::as_str) {
                    let item = params
                        .get("itemId")
                        .and_then(Value::as_str)
                        .map(str::to_string);
                    self.push_text(item, delta);
                }
                DialogueStep::default()
            }
            (ControlShape::CodexAppServer, "turn/completed") => self.finish(),
            (ControlShape::AcpCancel, "session/update") => {
                self.absorb_acp_update(params);
                DialogueStep::default()
            }
            _ => DialogueStep::default(),
        }
    }

    fn own_current_turn(&mut self) {
        if let Some(turn) = self.turn_id.as_deref().and_then(TurnId::new) {
            if !self.own_turns.contains(&turn) {
                self.own_turns.push(turn);
            }
        }
    }

    // llmlint: ignore-block[changed_behavior_has_e2e] The reachable paths are proved through the built binary (`crates/oneharness-e2e/tests/app_server.rs`: repeated cumulative readings across a redirect, the replayed resume baseline, another thread's frames on a broadcasting server, a timed-out turn). What stays unit-only is a server breaking codex's own schema — no `turnId`, non-integer or missing counts, totals below the replayed baseline — which real codex cannot be made to send and the double deliberately never does; the unit tests drive this pure state machine with exactly those lines.
    /// Read one `thread/tokenUsage/updated` notification
    /// (`ThreadTokenUsageUpdatedNotification`: `threadId`, `turnId`, and
    /// `tokenUsage.total` — the thread's cumulative `TokenUsageBreakdown`).
    ///
    /// Only this dialogue's thread counts. One naming a turn this dialogue
    /// opened moves the latest totals; one naming any other turn, before this
    /// dialogue's own have reported, is the thread's history (the server's
    /// replay on attach) and becomes the baseline. A notification that names
    /// no turn (`turnId` is required), or whose totals are not the schema's
    /// non-negative integers, is no reading at all.
    fn absorb_codex_token_usage(&mut self, params: &Value) {
        let thread = params.get("threadId").and_then(Value::as_str);
        if thread.is_none() || thread != self.session_id.as_deref() {
            return;
        }
        let Some(turn) = params
            .get("turnId")
            .and_then(Value::as_str)
            .and_then(TurnId::new)
        else {
            return;
        };
        let Some(totals) = params
            .pointer("/tokenUsage/total")
            .and_then(CodexTokenTotals::from_breakdown)
        else {
            return;
        };
        if self.own_turns.contains(&turn) {
            self.usage_latest = Some(totals);
        } else if self.usage_latest.is_none() {
            self.usage_baseline = Some(totals);
        }
    }

    /// The token usage the server reported for this dialogue's turns, or
    /// `None` when it reported none for them. The thread's cumulative totals
    /// less whatever it had spent before this dialogue's first turn, so a turn
    /// on a continued conversation counts only itself; a count that would go
    /// negative against that baseline is unknown, never clamped.
    #[must_use]
    pub fn usage(&self) -> Option<Usage> {
        let latest = self.usage_latest?;
        let since = |now: u64, before: fn(&CodexTokenTotals) -> u64| match &self.usage_baseline {
            Some(baseline) => now.checked_sub(before(baseline)),
            None => Some(now),
        };
        Some(Usage {
            input_tokens: since(latest.input, |b| b.input),
            output_tokens: since(latest.output, |b| b.output),
            cache_read_tokens: since(latest.cached_input, |b| b.cached_input),
            ..Usage::default()
        })
    }
    // llmlint: ignore-end[changed_behavior_has_e2e]

    /// Append `delta`, separating it from the previous message when it belongs
    /// to a different one — codex streams a running commentary and a final
    /// answer as distinct items, which otherwise concatenate mid-sentence.
    fn push_text(&mut self, item: Option<String>, delta: &str) {
        if item.is_some() && self.text_item.is_some() && item != self.text_item {
            self.text.push('\n');
        }
        if item.is_some() {
            self.text_item = item;
        }
        self.text.push_str(delta);
    }

    fn absorb_acp_update(&mut self, params: &Value) {
        let update = params.get("update").unwrap_or(&Value::Null);
        if update.get("sessionUpdate").and_then(Value::as_str) != Some("agent_message_chunk") {
            return;
        }
        if let Some(text) = update
            .get("content")
            .and_then(|content| content.get("text"))
            .and_then(Value::as_str)
        {
            self.text.push_str(text);
        }
    }

    fn on_response(&mut self, id: Option<u64>, message: &Value) -> DialogueStep {
        // Only an error answering one of this dialogue's active requests ends
        // it. A pooled/server stream may carry unrelated JSON-RPC responses;
        // terminating on an uncorrelated error would abort the wrong turn.
        let active_id = id.is_some_and(|id| {
            (self.phase == Phase::Handshake && id == self.handshake_id)
                || (self.phase == Phase::Opening && id == self.open_id)
                || (self.phase == Phase::InTurn && Some(id) == self.turn_request_id)
        });
        // The answer to a redirected turn's request says whether that turn
        // exists, which is what the interrupt carrying it is answered from. An
        // acknowledgement is taken only when it names the turn it opened —
        // `TurnStartResponse.turn.id` is required — so an answer that names none
        // is no evidence the message became a turn, and is refused as such.
        // Nor is there a turn to follow afterwards, so the conversation ends
        // there rather than waiting for a `turn/completed` nothing will send.
        if self.redirect_answer == RedirectAnswer::Awaited
            && self.phase == Phase::InTurn
            && id.is_some_and(|id| Some(id) == self.turn_request_id)
        {
            let named = message
                .pointer("/result/turn/id")
                .and_then(Value::as_str)
                .is_some_and(|turn| !turn.is_empty());
            self.redirect_answer = RedirectAnswer::Answered(match message.get("error") {
                Some(error) => Err(error
                    .get("message")
                    .and_then(Value::as_str)
                    .map_or_else(|| error.to_string(), str::to_string)),
                None if named => Ok(()),
                None => Err(format!(
                    "the server answered the replacement `turn/start` without naming a turn: {message}"
                )),
            });
            if !named && message.get("error").is_none() {
                return self.finish();
            }
        }
        if active_id && message.get("error").is_some() {
            return self.finish();
        }
        let result = message.get("result").unwrap_or(&Value::Null);
        match id {
            Some(id) if id == self.handshake_id && self.phase == Phase::Handshake => {
                self.phase = Phase::Opening;
                let open_id = self.take_id();
                self.open_id = open_id;
                match self.shape {
                    // Continue the conversation the caller's handle names when
                    // there is one: a driven turn builds no argv, so `thread/
                    // resume` is the only place a `--session` token can reach
                    // codex at all, and opening `thread/start` instead is a new
                    // conversation the store would then record as the old one.
                    ControlShape::CodexAppServer => {
                        let (method, params) = match self.config.resume.clone() {
                            Some(resume) => {
                                (resume.request(), self.thread_resume_params(resume.thread()))
                            }
                            None => ("thread/start", self.thread_start_params()),
                        };
                        DialogueStep::Send(vec![
                            notification("initialized", json!({})),
                            request(open_id, method, params),
                        ])
                    }
                    _ => DialogueStep::Send(vec![request(
                        open_id,
                        "session/new",
                        json!({"cwd": self.config.cwd.to_string(), "mcpServers": []}),
                    )]),
                }
            }
            Some(id) if id == self.open_id && self.phase == Phase::Opening => {
                self.session_id = match self.shape {
                    ControlShape::CodexAppServer => result
                        .get("thread")
                        .and_then(|thread| thread.get("id"))
                        .and_then(Value::as_str)
                        .map(str::to_string),
                    _ => result
                        .get("sessionId")
                        .and_then(Value::as_str)
                        .map(str::to_string),
                };
                // The server's own statement of the model the thread runs
                // under, before any token is spent: a required field of both
                // `ThreadStartResponse` and `ThreadResumeResponse`, and of
                // nothing later — `turn/started` carries no model.
                // llmlint: ignore[boundary_inputs_validated] A response that
                // states no model is deliberately no observation rather than a
                // refusal: the check below refuses a model the server NAMED
                // that differs from the one requested, and an app-server whose
                // open response carries no `model` (0.153.4's schema requires
                // one; a server of another version need not) has claimed
                // nothing to refuse — ending every controlled turn against it
                // would be a regression with no wrong model to point at. The
                // tolerated arm is pinned end to end by
                // `a_controlled_codex_turn_proceeds_when_the_server_names_no_model`.
                if self.shape == ControlShape::CodexAppServer {
                    self.observed_model = result
                        .get("model")
                        .and_then(Value::as_str)
                        .map(str::to_string);
                }
                let Some(session) = self.session_id.clone() else {
                    // No identifier means no turn to address and no handle to
                    // resume; stop rather than send a prompt nobody can interrupt.
                    return self.finish();
                };
                // The requested model and the reported one differ: the turn
                // would be billed to a model nobody asked for, so it is refused
                // HERE, before `turn/start`, at zero cost. An absent claim on
                // either side contradicts nothing and proceeds — the server may
                // not name one, and a run may not have asked for one.
                if let (Some(requested), Some(observed)) =
                    (self.config.model.as_deref(), self.observed_model.as_deref())
                {
                    if requested != observed {
                        self.refusal = Some(DialogueRefusal::ModelMismatch {
                            requested: requested.to_string(),
                            observed: observed.to_string(),
                        });
                        self.phase = Phase::Done;
                        return DialogueStep::Terminal;
                    }
                }
                self.phase = Phase::InTurn;
                let turn_id = self.take_id();
                self.turn_request_id = Some(turn_id);
                let prompt = self.config.prompt.clone();
                DialogueStep::Send(vec![request(
                    turn_id,
                    self.turn_method(),
                    self.turn_params(&session, &prompt),
                )])
            }
            Some(id) if Some(id) == self.turn_request_id => match self.shape {
                // Codex answers `turn/start` IMMEDIATELY with the new turn's id
                // and `status: "inProgress"` — the response is the turn's
                // *acknowledgement*, not its end. Treating it as terminal ended
                // every controlled run in under half a second, before the agent
                // did anything. The turn ends on `turn/completed`.
                ControlShape::CodexAppServer => {
                    if let Some(turn) = result
                        .get("turn")
                        .and_then(|turn| turn.get("id"))
                        .and_then(Value::as_str)
                    {
                        self.turn_id = Some(turn.to_string());
                        self.own_current_turn();
                    }
                    DialogueStep::default()
                }
                // ACP's `session/prompt` response carries the stop reason, so it
                // really is the end of the turn.
                _ => self.finish(),
            },
            _ => DialogueStep::default(),
        }
    }

    /// The turn is over. If an interrupt committed a redirection, this is the
    /// first moment a new turn can carry it, so the conversation opens one
    /// instead of ending — otherwise the dialogue is done.
    fn finish(&mut self) -> DialogueStep {
        if let Some(redirect) = self.pending_redirect.take() {
            if let Some(frames) = self.open_redirected_turn(&redirect) {
                return DialogueStep::Send(frames);
            }
        }
        self.phase = Phase::Done;
        DialogueStep::Terminal
    }

    /// The frames that open a fresh turn carrying `prompt` on the session this
    /// conversation already holds — the redirection's delivery.
    ///
    /// `None` when the server never named a session, which is the one case
    /// there is nothing to address a new turn to.
    fn open_redirected_turn(&mut self, prompt: &RedirectInput) -> Option<Vec<String>> {
        let session = self.session_id.clone()?;
        // The aborted turn's answer and the redirected one are different
        // answers; run together they read as one sentence that changes its mind.
        if !self.text.is_empty() {
            self.text.push('\n');
            self.text_item = None;
        }
        // The old turn is gone: codex mints a new id for the new one, and
        // interrupting against the stale id would address a turn that ended.
        self.turn_id = None;
        self.phase = Phase::InTurn;
        let id = self.take_id();
        self.turn_request_id = Some(id);
        if self.shape == ControlShape::CodexAppServer {
            self.redirect_answer = RedirectAnswer::Awaited;
        }
        Some(vec![request(
            id,
            self.turn_method(),
            self.turn_params(&session, prompt.as_str()),
        )])
    }

    fn turn_method(&self) -> &'static str {
        match self.shape {
            ControlShape::CodexAppServer => "turn/start",
            _ => "session/prompt",
        }
    }

    fn thread_start_params(&self) -> Value {
        let mut params = json!({
            "cwd": self.config.cwd.to_string(),
            "approvalPolicy": self.codex_approval_policy(),
        });
        if let Some(model) = &self.config.model {
            params["model"] = json!(model);
        }
        if let Some(policy) = &self.config.shell_environment_policy {
            params["config"] = policy.thread_config();
        }
        params
    }

    /// The params that reopen the thread `id` names, in codex's own spelling.
    ///
    /// `threadId` is the only required field, and its schema says to prefer it
    /// over the history/path forms. The rest are the per-run settings a fresh
    /// thread would have been started with, restated so a resumed thread runs
    /// under this run's working directory and model rather than the ones the
    /// conversation was created with.
    fn thread_resume_params(&self, id: &str) -> Value {
        let mut params = self.thread_start_params();
        params["threadId"] = json!(id);
        params
    }

    /// The params that open a turn on `session` carrying `prompt` — the run's
    /// own prompt first, and afterwards whatever a redirect delivered. One
    /// function for both, so a redirected turn is negotiated in exactly the
    /// posture and working directory the original was.
    fn turn_params(&self, session: &str, prompt: &str) -> Value {
        match self.shape {
            ControlShape::CodexAppServer => {
                let mut params = json!({
                    "threadId": session,
                    "cwd": self.config.cwd.to_string(),
                    "input": [{"type": "text", "text": prompt}],
                    "approvalPolicy": self.codex_approval_policy(),
                    "sandboxPolicy": self.codex_sandbox_policy(),
                });
                // Restated per turn (`TurnStartParams.model` overrides the
                // model for this turn and the ones after it), so the turn runs
                // under the model the run resolved even on a thread that was
                // created — or resumed — under another.
                if let Some(model) = &self.config.model {
                    params["model"] = json!(model);
                }
                params
            }
            _ => json!({
                "sessionId": session,
                "prompt": [{"type": "text", "text": prompt}],
            }),
        }
    }

    /// Codex's own approval spelling for this run's mode. `never` is the
    /// headless posture: the client still answers any request that arrives, but
    /// a turn should not be *waiting* on approvals it cannot show anyone.
    fn codex_approval_policy(&self) -> &'static str {
        "never"
    }

    /// The narrowest sandbox that still lets the mode do what it promises, in
    /// Codex's own spelling (sourced from its app-server `SandboxPolicy`).
    ///
    /// `bypass` must mean here exactly what it means on `codex exec`, where it
    /// maps to `--dangerously-bypass-approvals-and-sandbox` — no sandbox at all.
    /// Confining it to `workspaceWrite` instead made `--control --mode bypass`
    /// strictly more restricted than the same mode without `--control`, and the
    /// difference is not cosmetic: Codex enforces `workspaceWrite` by launching
    /// every shell call under bubblewrap in its own network namespace, and where
    /// that namespace cannot be set up the call fails BEFORE running
    /// (`bwrap: loopback: Failed RTM_NEWADDR` on a GitHub Actions runner), so
    /// the turn does no work whatsoever. It went unnoticed because a developer
    /// box where the sandbox does not bite behaves identically either way;
    /// `e2e-control.sh` caught it the first time it ran somewhere it does.
    ///
    /// `auto` is the mode that genuinely wants the narrow grant — it promises to
    /// act on its own within the workspace, not to hand over the machine.
    pub(crate) fn codex_sandbox_policy(&self) -> Value {
        match self.config.mode {
            PermissionMode::Bypass => json!({"type": "dangerFullAccess"}),
            PermissionMode::Auto => json!({
                "type": "workspaceWrite",
                "writableRoots": [self.config.cwd.to_string()],
                "networkAccess": false,
            }),
            PermissionMode::ReadOnly
            | PermissionMode::Plan
            | PermissionMode::Default
            | PermissionMode::Edit => json!({"type": "readOnly"}),
        }
    }
}

/// What one ACP permission option does, read off its `kind`.
///
/// A closed set parsed at the boundary rather than substring checks on the
/// server's raw string: the four ACP kinds happen to be distinguishable by
/// substring, but an invented one need not be — `disallow_once` contains
/// "allow" and would be selected as a grant. An unknown kind is never
/// selected, because an option whose meaning cannot be read is not one to
/// grant.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(from = "String")]
enum OptionKind {
    AllowOnce,
    AllowAlways,
    RejectOnce,
    RejectAlways,
    #[default]
    Unknown,
}

impl From<String> for OptionKind {
    fn from(raw: String) -> Self {
        match raw.as_str() {
            "allow_once" => OptionKind::AllowOnce,
            "allow_always" => OptionKind::AllowAlways,
            "reject_once" => OptionKind::RejectOnce,
            "reject_always" => OptionKind::RejectAlways,
            _ => OptionKind::Unknown,
        }
    }
}

/// An ACP permission option the client may select, as the server offers it.
#[derive(Debug, Deserialize)]
struct PermissionOption {
    #[serde(rename = "optionId")]
    option_id: String,
    #[serde(default)]
    kind: OptionKind,
}

/// The option to select for a `session/request_permission` under a permissive
/// mode: the server's own **allow** option, preferring a one-shot grant over a
/// standing one.
///
/// `None` when no offered option says it allows anything — including when the
/// server offered options this build cannot read. Picking whatever came first
/// would be granting a permission nobody chose, which is worse than declining:
/// declining costs a turn, granting costs whatever the option was.
#[must_use]
pub fn acp_allow_option(options: &Value) -> Option<String> {
    let options: Vec<PermissionOption> = serde_json::from_value(options.clone()).ok()?;
    let of_kind = |wanted: OptionKind| options.iter().find(move |o| o.kind == wanted);
    of_kind(OptionKind::AllowOnce)
        .or_else(|| of_kind(OptionKind::AllowAlways))
        .map(|option| option.option_id.clone())
}

/// Whether `method` is one of Codex's approval requests — the only ones an
/// approval decision is a meaningful answer to. Sourced from the app-server's
/// own generated protocol schema, never guessed.
fn is_codex_approval(method: &str) -> bool {
    matches!(
        method,
        "applyPatchApproval"
            | "execCommandApproval"
            | "item/commandExecution/requestApproval"
            | "item/fileChange/requestApproval"
            | "item/permissions/requestApproval"
    )
}

fn request(id: u64, method: &str, params: Value) -> String {
    line(json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}))
}

fn notification(method: &str, params: Value) -> String {
    line(json!({"jsonrpc": "2.0", "method": method, "params": params}))
}

fn response(id: Value, result: Value) -> String {
    line(json!({"jsonrpc": "2.0", "id": id, "result": result}))
}

/// One newline-terminated wire line. The shapes are closed, so encoding cannot
/// fail; an empty object would be a protocol violation either way, so a failure
/// is surfaced as one rather than silently dropped.
fn line(value: Value) -> String {
    format!("{value}\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::control::{absolute_for_test, absolute_text_for_test};

    /// The one working directory every fixture turn runs in.
    const WORK: &str = "/work";

    fn config() -> DialogueConfig {
        DialogueConfig {
            prompt: "do the thing".to_string(),
            cwd: absolute_for_test(WORK),
            model: Some("gpt-5-codex".to_string()),
            mode: PermissionMode::Bypass,
            resume: None,
            posture: ApprovalPosture::Unattended,
            shell_environment_policy: None,
        }
    }

    /// `text` as the validated redirection an interrupt would carry.
    fn redirection(text: &str) -> RedirectInput {
        RedirectInput::new(text).expect("fixture redirections are messages")
    }

    fn parse(frame: &str) -> Value {
        assert!(frame.ends_with('\n'), "frames are newline-terminated");
        serde_json::from_str(frame).expect("frames are JSON")
    }

    #[test]
    fn claude_control_needs_no_dialogue() {
        assert!(Dialogue::new(ControlShape::ClaudeControlRequest, config()).is_none());
        assert!(Dialogue::new(ControlShape::CodexAppServer, config()).is_some());
        assert!(Dialogue::new(ControlShape::AcpCancel, config()).is_some());
    }

    /// Drive a whole codex turn the way the app-server really answers, and
    /// A named handle reopens the conversation it names instead of minting one.
    ///
    /// A driven turn builds no argv, so the harness's verified `--resume`
    /// mapping is never reached and this request is the ONLY route a `--session`
    /// token has into codex. Opening `thread/start` here is a fresh
    /// conversation the session store would then record as the old one — the
    /// flag accepted, the token rotated, and every earlier turn gone.
    #[test]
    fn a_resumed_codex_turn_reopens_the_thread_the_handle_names() {
        let mut d = Dialogue::new(
            ControlShape::CodexAppServer,
            DialogueConfig {
                resume: DialogueResume::new(ControlShape::CodexAppServer, "th-stored".to_string()),
                ..config()
            },
        )
        .unwrap();
        d.open();
        let step = d.on_line(r#"{"jsonrpc":"2.0","id":1,"result":{}}"#);
        assert_eq!(parse(&step.frames()[0])["method"], "initialized");
        let resume = parse(&step.frames()[1]);
        assert_eq!(resume["method"], "thread/resume");
        assert_eq!(resume["params"]["threadId"], "th-stored");
        // The per-run settings a fresh thread would carry are restated, so a
        // resumed conversation runs in THIS run's directory under THIS run's
        // model rather than the ones it was created with.
        assert_eq!(resume["params"]["cwd"], absolute_text_for_test(WORK));
        assert_eq!(resume["params"]["approvalPolicy"], "never");
        assert_eq!(resume["params"]["model"], "gpt-5-codex");

        // `ThreadResumeResponse` carries the same required `thread` field
        // `ThreadStartResponse` does, so one arm reads the opened conversation
        // either way — and the turn goes out on the thread that was resumed.
        let step = d.on_line(r#"{"jsonrpc":"2.0","id":2,"result":{"thread":{"id":"th-stored"}}}"#);
        assert_eq!(d.session_id(), Some("th-stored"));
        let turn = parse(&step.frames()[0]);
        assert_eq!(turn["method"], "turn/start");
        assert_eq!(turn["params"]["threadId"], "th-stored");
        assert_eq!(turn["params"]["input"][0]["text"], "do the thing");
    }

    /// assert every frame oneharness sends back.
    #[test]
    fn codex_handshakes_opens_a_thread_and_sends_the_prompt() {
        let mut d = Dialogue::new(ControlShape::CodexAppServer, config()).unwrap();
        let open = d.open();
        assert_eq!(open.len(), 1);
        let init = parse(&open[0]);
        assert_eq!(init["method"], "initialize");
        assert_eq!(init["id"], 1);

        // Nothing to interrupt before the prompt is out.
        assert!(d.interrupt(None).is_none());

        let step = d.on_line(r#"{"jsonrpc":"2.0","id":1,"result":{}}"#);
        assert!(!step.is_terminal());
        assert_eq!(step.frames().len(), 2);
        assert_eq!(parse(&step.frames()[0])["method"], "initialized");
        let start = parse(&step.frames()[1]);
        assert_eq!(start["method"], "thread/start");
        assert_eq!(start["params"]["cwd"], absolute_text_for_test(WORK));
        assert_eq!(start["params"]["approvalPolicy"], "never");
        assert_eq!(start["params"]["model"], "gpt-5-codex");

        let step = d.on_line(r#"{"jsonrpc":"2.0","id":2,"result":{"thread":{"id":"th-1"}}}"#);
        assert!(!step.is_terminal());
        assert_eq!(d.session_id(), Some("th-1"));
        let turn = parse(&step.frames()[0]);
        assert_eq!(turn["method"], "turn/start");
        assert_eq!(turn["params"]["threadId"], "th-1");
        assert_eq!(turn["params"]["input"][0]["text"], "do the thing");
        // `bypass` asks for no sandbox, exactly as it does on `codex exec`.
        // Anything narrower runs the shell under bubblewrap, which does no work
        // at all where its network namespace cannot be set up.
        assert_eq!(turn["params"]["sandboxPolicy"]["type"], "dangerFullAccess");

        // The turn id arrives on the event stream and is what interrupt addresses.
        d.on_line(r#"{"jsonrpc":"2.0","method":"turn/started","params":{"threadId":"th-1","turn":{"id":"tu-9"}}}"#);
        let frames = d.interrupt(None).expect("a turn is in flight");
        let abort = parse(&frames[0]);
        assert_eq!(abort["method"], "turn/interrupt");
        assert_eq!(abort["params"]["threadId"], "th-1");
        assert_eq!(abort["params"]["turnId"], "tu-9");

        // Assistant text is accumulated from the delta stream.
        d.on_line(
            r#"{"jsonrpc":"2.0","method":"item/agentMessage/delta","params":{"itemId":"m1","delta":"all "}}"#,
        );
        d.on_line(
            r#"{"jsonrpc":"2.0","method":"item/agentMessage/delta","params":{"itemId":"m1","delta":"done"}}"#,
        );
        // A second message is separated rather than run together mid-sentence.
        d.on_line(
            r#"{"jsonrpc":"2.0","method":"item/agentMessage/delta","params":{"itemId":"m2","delta":"and here is why"}}"#,
        );
        assert_eq!(d.text().as_deref(), Some("all done\nand here is why"));

        // The turn/start response ACKNOWLEDGES the turn; it does not end it.
        let step = d.on_line(
            r#"{"jsonrpc":"2.0","id":3,"result":{"turn":{"id":"tu-9","status":"inProgress"}}}"#,
        );
        assert!(
            !step.is_terminal(),
            "codex acknowledges turn/start immediately — ending here would end every run instantly"
        );
        assert!(d.interrupt(None).is_some(), "the turn is still in flight");

        // `turn/completed` is what ends it.
        let step = d.on_line(r#"{"jsonrpc":"2.0","method":"turn/completed","params":{}}"#);
        assert!(step.is_terminal());
        assert!(d.interrupt(None).is_none(), "no turn left to interrupt");
    }

    /// A `thread/tokenUsage/updated` line in codex's own
    /// `ThreadTokenUsageUpdatedNotification` shape: `total` is the thread's
    /// cumulative breakdown, `last` the latest model call's.
    fn token_usage(thread: &str, turn: &str, input: u64, cached: u64, output: u64) -> String {
        json!({"jsonrpc": "2.0", "method": "thread/tokenUsage/updated", "params": {
            "threadId": thread, "turnId": turn, "tokenUsage": {
                "total": {"totalTokens": input + output, "inputTokens": input,
                    "cachedInputTokens": cached, "outputTokens": output,
                    "reasoningOutputTokens": 0},
                "last": {"totalTokens": 1, "inputTokens": 1, "cachedInputTokens": 0,
                    "outputTokens": 0, "reasoningOutputTokens": 0},
                "modelContextWindow": null}}})
        .to_string()
    }

    /// Codex's own schema for the notification, generated by
    /// `codex app-server generate-json-schema` (codex-cli 0.159.2).
    const TOKEN_USAGE_SCHEMA: &str =
        include_str!("../../../../tests/fixtures/codex-thread-token-usage.schema.json");

    /// The fixture above speaks codex's schema, and every count the reader
    /// takes is one the schema requires — so a rename upstream fails here
    /// rather than reading as a turn that reported nothing.
    #[test]
    fn codex_usage_reads_only_what_codex_s_schema_requires() {
        let schema: Value = serde_json::from_str(TOKEN_USAGE_SCHEMA).unwrap();
        let validator = jsonschema::validator_for(&schema).unwrap();
        let frame: Value = serde_json::from_str(&token_usage("th", "tu", 3, 1, 2)).unwrap();
        assert!(validator.is_valid(&frame["params"]), "{frame}");
        let required = |path: &str| schema.pointer(path).cloned().unwrap_or_default();
        for key in ["threadId", "turnId", "tokenUsage"] {
            assert!(required("/required")
                .as_array()
                .unwrap()
                .contains(&json!(key)));
        }
        assert!(required("/definitions/ThreadTokenUsage/required")
            .as_array()
            .unwrap()
            .contains(&json!("total")));
        for key in ["inputTokens", "cachedInputTokens", "outputTokens"] {
            assert!(required("/definitions/TokenUsageBreakdown/required")
                .as_array()
                .unwrap()
                .contains(&json!(key)));
        }
    }

    /// The turn's usage is the thread's latest cumulative total the server
    /// reported for it — not the last model call's, and not a sum of repeats.
    #[test]
    fn codex_usage_is_the_turn_s_reported_total() {
        let mut d = codex_in_turn();
        assert_eq!(d.usage(), None, "nothing reported yet is no reading");
        d.on_line(&token_usage("th-1", "tu-9", 900, 100, 20));
        d.on_line(&token_usage("th-1", "tu-9", 1500, 400, 75));
        // Another thread's accounting on a shared stream is not this turn's.
        d.on_line(&token_usage("th-other", "tu-9", 99_999, 0, 99_999));
        d.on_line(r#"{"jsonrpc":"2.0","method":"turn/completed","params":{}}"#);
        let usage = d.usage().expect("the server reported usage");
        assert_eq!(usage.input_tokens, Some(1500));
        assert_eq!(usage.output_tokens, Some(75));
        assert_eq!(usage.cache_read_tokens, Some(400));
        assert_eq!(usage.cache_write_tokens, None);
        assert_eq!(usage.cost_usd, None);
    }

    /// A continued conversation's earlier turns, replayed by the server when
    /// the thread is attached, are the baseline: only this turn is counted.
    #[test]
    fn codex_usage_on_a_resumed_thread_counts_only_this_turn() {
        let mut d = codex_in_turn();
        d.on_line(&token_usage("th-1", "tu-earlier", 1000, 200, 50));
        d.on_line(&token_usage("th-1", "tu-9", 1600, 500, 80));
        let usage = d.usage().unwrap();
        assert_eq!(usage.input_tokens, Some(600));
        assert_eq!(usage.cache_read_tokens, Some(300));
        assert_eq!(usage.output_tokens, Some(30));
        // A replay arriving after the turn reported is history, not a baseline.
        d.on_line(&token_usage("th-1", "tu-earlier", 0, 0, 0));
        assert_eq!(d.usage().unwrap().input_tokens, Some(600));
    }

    /// A count the baseline exceeds is unknown rather than clamped to zero, and
    /// a breakdown missing a schema-required count is no reading at all.
    #[test]
    fn codex_usage_never_fabricates_a_count() {
        let mut d = codex_in_turn();
        d.on_line(&token_usage("th-1", "tu-earlier", 1000, 0, 50));
        d.on_line(&token_usage("th-1", "tu-9", 900, 0, 80));
        let usage = d.usage().unwrap();
        assert_eq!(usage.input_tokens, None);
        assert_eq!(usage.output_tokens, Some(30));

        // A notification naming no turn cannot be placed, so it is not a
        // baseline either: this turn's later reading is still its whole usage.
        let mut d = codex_in_turn();
        d.on_line(
            r#"{"jsonrpc":"2.0","method":"thread/tokenUsage/updated","params":{"threadId":"th-1","tokenUsage":{"total":{"inputTokens":50,"cachedInputTokens":0,"outputTokens":5}}}}"#,
        );
        d.on_line(&token_usage("th-1", "tu-9", 70, 0, 9));
        assert_eq!(d.usage().unwrap().input_tokens, Some(70));

        let mut d = codex_in_turn();
        d.on_line(
            r#"{"jsonrpc":"2.0","method":"thread/tokenUsage/updated","params":{"threadId":"th-1","turnId":"tu-9","tokenUsage":{"total":{"inputTokens":-1,"cachedInputTokens":0,"outputTokens":3}}}}"#,
        );
        d.on_line(
            r#"{"jsonrpc":"2.0","method":"thread/tokenUsage/updated","params":{"threadId":"th-1","turnId":"tu-9","tokenUsage":{"total":{"inputTokens":5,"outputTokens":3}}}}"#,
        );
        assert_eq!(d.usage(), None);
    }

    /// The turn a redirection opens is this run's too, so its usage counts.
    #[test]
    fn codex_usage_includes_the_redirected_turn() {
        let mut d = codex_in_turn();
        d.on_line(&token_usage("th-1", "tu-9", 100, 0, 10));
        d.interrupt(Some(&redirection("do the other thing")))
            .unwrap();
        d.on_line(r#"{"jsonrpc":"2.0","method":"turn/completed","params":{}}"#);
        d.on_line(r#"{"jsonrpc":"2.0","method":"turn/started","params":{"threadId":"th-1","turn":{"id":"tu-2"}}}"#);
        d.on_line(&token_usage("th-1", "tu-2", 250, 0, 40));
        assert_eq!(d.usage().unwrap().input_tokens, Some(250));
    }

    /// Drive a codex dialogue through its handshake and return the open
    /// request (`thread/start` or `thread/resume`) it sent.
    fn codex_opened(d: &mut Dialogue) -> Value {
        d.open();
        let step = d.on_line(r#"{"jsonrpc":"2.0","id":1,"result":{}}"#);
        parse(&step.frames()[1])
    }

    /// A pooled dispatch's environment rides the thread, on both frames that
    /// open one, and never the turn: `turn/start` has no `config` map. Its
    /// turn is recorded as active from the moment the server names it until
    /// `turn/completed`.
    #[test]
    fn codex_carries_the_thread_config_on_thread_start_and_resume_only() {
        use crate::domain::app_server::{
            DispatchMarker, EnvPlatform, ShellEnvironmentPolicy, DISPATCH_MARKER_VAR,
            SERVER_MARKER_VAR,
        };
        let policy = ShellEnvironmentPolicy::for_dispatch(
            &[("A".to_string(), "1".to_string())],
            &[(SERVER_MARKER_VAR.to_string(), "s".to_string())],
            &DispatchMarker::new("m").unwrap(),
            EnvPlatform::Unix,
        );
        let thread_config = json!({"shell_environment_policy": {
            "set": {"A": "1", DISPATCH_MARKER_VAR: "m"},
            "exclude": [SERVER_MARKER_VAR],
        }});
        for resume in [
            None,
            DialogueResume::new(ControlShape::CodexAppServer, "th-old".to_string()),
        ] {
            let mut d = Dialogue::new(
                ControlShape::CodexAppServer,
                DialogueConfig {
                    shell_environment_policy: Some(policy.clone()),
                    resume: resume.clone(),
                    ..config()
                },
            )
            .unwrap();
            let opened = codex_opened(&mut d);
            assert_eq!(opened["params"]["config"], thread_config, "{opened}");
            assert_eq!(d.active_turn(), None);
            assert!(!d.in_turn());
            let step = d.on_line(
                r#"{"jsonrpc":"2.0","id":2,"result":{"thread":{"id":"th-1"},"model":"gpt-5-codex"}}"#,
            );
            let turn = parse(&step.frames()[0]);
            assert!(turn["params"].get("config").is_none(), "{turn}");
            assert!(d.in_turn());
            assert_eq!(d.active_turn(), None, "no turn id yet: nothing to address");
            d.on_line(r#"{"jsonrpc":"2.0","id":3,"result":{"turn":{"id":"tu-1"}}}"#);
            assert_eq!(d.active_turn(), Some(("th-1", "tu-1")));
            d.on_line(r#"{"jsonrpc":"2.0","method":"turn/completed","params":{"threadId":"th-1","turn":{"id":"tu-1"}}}"#);
            assert_eq!(d.active_turn(), None);
            assert!(!d.in_turn());
        }
        let mut own_child = Dialogue::new(ControlShape::CodexAppServer, config()).unwrap();
        assert!(codex_opened(&mut own_child)["params"]
            .get("config")
            .is_none());
    }

    /// The model rides every codex frame that takes one, and it is the
    /// candidate's own: `thread/start` opens the conversation under it and
    /// `turn/start` restates it, so a turn cannot run under whatever model the
    /// thread happened to be created with.
    #[test]
    fn codex_requests_the_model_on_thread_start_and_turn_start_alike() {
        let mut d = Dialogue::new(ControlShape::CodexAppServer, config()).unwrap();
        assert_eq!(codex_opened(&mut d)["params"]["model"], "gpt-5-codex");
        let step = d.on_line(
            r#"{"jsonrpc":"2.0","id":2,"result":{"thread":{"id":"th-1"},"model":"gpt-5-codex"}}"#,
        );
        let turn = parse(&step.frames()[0]);
        assert_eq!(turn["method"], "turn/start");
        assert_eq!(turn["params"]["model"], "gpt-5-codex");
        assert_eq!(d.observed_model(), Some("gpt-5-codex"));
        assert!(d.refusal().is_none());

        // A redirected turn is negotiated under the same model as the first.
        d.on_line(r#"{"jsonrpc":"2.0","method":"turn/started","params":{"turn":{"id":"tu-9"}}}"#);
        d.interrupt(Some(&redirection("do X instead"))).unwrap();
        let step = d.on_line(r#"{"jsonrpc":"2.0","method":"turn/completed","params":{}}"#);
        let next = parse(&step.frames()[0]);
        assert_eq!(next["method"], "turn/start");
        assert_eq!(next["params"]["model"], "gpt-5-codex");

        // With no model resolved, no frame names one — the server's default is
        // the honest request, and `null` is not a model.
        let mut unset = Dialogue::new(
            ControlShape::CodexAppServer,
            DialogueConfig {
                model: None,
                ..config()
            },
        )
        .unwrap();
        assert!(codex_opened(&mut unset)["params"].get("model").is_none());
        let step = unset.on_line(
            r#"{"jsonrpc":"2.0","id":2,"result":{"thread":{"id":"th-1"},"model":"gpt-6-astra"}}"#,
        );
        let turn = parse(&step.frames()[0]);
        assert_eq!(
            turn["method"], "turn/start",
            "nothing requested, nothing to contradict"
        );
        assert!(turn["params"].get("model").is_none());
        assert_eq!(unset.observed_model(), Some("gpt-6-astra"));
        assert!(unset.refusal().is_none());
    }

    /// The server said it would run the thread under a model other than the one
    /// requested: no `turn/start` goes out, the dialogue ends, and it says why
    /// with both names spelled as the config and the server spelled them.
    #[test]
    fn codex_refuses_a_thread_the_server_would_run_under_another_model() {
        let mut d = Dialogue::new(ControlShape::CodexAppServer, config()).unwrap();
        codex_opened(&mut d);
        let step = d.on_line(
            r#"{"jsonrpc":"2.0","id":2,"result":{"thread":{"id":"th-1"},"model":"gpt-6-astra"}}"#,
        );
        assert!(
            step.is_terminal(),
            "the turn must not be submitted: {step:?}"
        );
        assert!(step.frames().is_empty(), "no turn/start: {step:?}");
        assert_eq!(
            d.refusal(),
            Some(&DialogueRefusal::ModelMismatch {
                requested: "gpt-5-codex".to_string(),
                observed: "gpt-6-astra".to_string(),
            })
        );
        assert_eq!(d.observed_model(), Some("gpt-6-astra"));
        // The thread the server opened is still real, and still the handle.
        assert_eq!(d.session_id(), Some("th-1"));
        let error = d.refusal().unwrap().error("codex");
        assert!(
            error.contains("\"gpt-6-astra\"") && error.contains("\"gpt-5-codex\""),
            "both names verbatim: {error}"
        );
        assert!(error.starts_with("harness `codex`"), "{error}");
        // Ended: nothing to interrupt, and later lines change nothing.
        assert!(d.interrupt(None).is_none());
        assert_eq!(
            d.on_line(r#"{"jsonrpc":"2.0","method":"turn/completed","params":{}}"#),
            DialogueStep::default()
        );

        // Byte for byte: a case or suffix difference is a different model id.
        let mut cased = Dialogue::new(ControlShape::CodexAppServer, config()).unwrap();
        codex_opened(&mut cased);
        let step = cased.on_line(
            r#"{"jsonrpc":"2.0","id":2,"result":{"thread":{"id":"th-1"},"model":"GPT-5-codex"}}"#,
        );
        assert!(step.is_terminal());
        assert!(cased.refusal().is_some());

        // The same on a resumed thread: `ThreadResumeResponse.model` is the
        // same required field, read on the same arm.
        let mut resumed = Dialogue::new(
            ControlShape::CodexAppServer,
            DialogueConfig {
                resume: DialogueResume::new(ControlShape::CodexAppServer, "th-stored".to_string()),
                ..config()
            },
        )
        .unwrap();
        assert_eq!(codex_opened(&mut resumed)["method"], "thread/resume");
        let step = resumed.on_line(
            r#"{"jsonrpc":"2.0","id":2,"result":{"thread":{"id":"th-stored"},"model":"gpt-6-astra"}}"#,
        );
        assert!(step.is_terminal());
        assert_eq!(resumed.observed_model(), Some("gpt-6-astra"));
        assert!(resumed.refusal().is_some());
    }

    /// An open response that names no model contradicts nothing: the turn
    /// proceeds exactly as before the field was read, and nothing is inferred.
    #[test]
    fn codex_proceeds_when_the_open_response_names_no_model() {
        let mut d = Dialogue::new(ControlShape::CodexAppServer, config()).unwrap();
        codex_opened(&mut d);
        let step = d.on_line(r#"{"jsonrpc":"2.0","id":2,"result":{"thread":{"id":"th-1"}}}"#);
        let turn = parse(&step.frames()[0]);
        assert_eq!(turn["method"], "turn/start");
        assert_eq!(d.observed_model(), None, "never derived from the request");
        assert!(d.refusal().is_none());

        // ACP names no model on `session/new` at all, and is never refused.
        let mut acp = Dialogue::new(ControlShape::AcpCancel, config()).unwrap();
        acp.open();
        acp.on_line(r#"{"jsonrpc":"2.0","id":1,"result":{}}"#);
        let step = acp
            .on_line(r#"{"jsonrpc":"2.0","id":2,"result":{"sessionId":"sess-7","model":"other"}}"#);
        assert_eq!(parse(&step.frames()[0])["method"], "session/prompt");
        assert_eq!(acp.observed_model(), None);
        assert!(acp.refusal().is_none());
    }

    /// The open response as codex 0.153.4 really answers it, with `result.model`
    /// where the dialogue reads it — captured exchange in
    /// `tests/fixtures/codex-app-server-thread-start.jsonl` (request as sent,
    /// response, `thread/started`). `thread/start` spends nothing, so the
    /// fixture cost no quota and the scheduled live suite is its re-check.
    #[test]
    fn codex_reads_the_served_model_off_a_captured_thread_start_response() {
        let fixture =
            include_str!("../../../../tests/fixtures/codex-app-server-thread-start.jsonl");
        let mut lines = fixture.lines();
        let request: Value = serde_json::from_str(lines.next().unwrap()).unwrap();
        let response = lines.next().unwrap();
        let started = lines.next().unwrap();
        assert_eq!(request["method"], "thread/start");
        let requested = request["params"]["model"].as_str().unwrap();
        assert_eq!(requested, "gpt-5.6-sol");

        // Requested exactly what the capture requested: the server answers the
        // same model and the turn goes out.
        let mut d = Dialogue::new(
            ControlShape::CodexAppServer,
            DialogueConfig {
                model: Some(requested.to_string()),
                ..config()
            },
        )
        .unwrap();
        assert_eq!(codex_opened(&mut d)["params"]["model"], requested);
        let step = d.on_line(response);
        assert_eq!(d.observed_model(), Some(requested));
        assert!(d.refusal().is_none());
        let turn = parse(&step.frames()[0]);
        assert_eq!(turn["method"], "turn/start");
        assert_eq!(
            turn["params"]["threadId"],
            "01a0908e-1c1e-74e2-94f6-2b7d76737f6d"
        );
        assert_eq!(turn["params"]["model"], requested);
        // `thread/started` repeats it under `thread.model` and changes nothing.
        assert_eq!(d.on_line(started), DialogueStep::default());
        assert_eq!(d.observed_model(), Some(requested));

        // The same response against a run that asked for another model is the
        // refusal — the server's word, not the request, decides `observed_model`.
        let mut other = Dialogue::new(
            ControlShape::CodexAppServer,
            DialogueConfig {
                model: Some("gpt-6-astra".to_string()),
                ..config()
            },
        )
        .unwrap();
        codex_opened(&mut other);
        assert!(other.on_line(response).is_terminal());
        assert_eq!(other.observed_model(), Some(requested));
        assert_eq!(
            other.refusal(),
            Some(&DialogueRefusal::ModelMismatch {
                requested: "gpt-6-astra".to_string(),
                observed: requested.to_string(),
            })
        );
    }

    #[test]
    fn codex_takes_the_turn_id_from_the_turn_start_response_too() {
        // The id arrives on whichever comes first — the response or the
        // `turn/started` notification — and interrupt needs it either way.
        let mut d = Dialogue::new(ControlShape::CodexAppServer, config()).unwrap();
        d.open();
        d.on_line(r#"{"jsonrpc":"2.0","id":1,"result":{}}"#);
        d.on_line(r#"{"jsonrpc":"2.0","id":2,"result":{"thread":{"id":"th-1"}}}"#);
        assert!(d.interrupt(None).is_none(), "no turn id yet");
        d.on_line(r#"{"jsonrpc":"2.0","id":3,"result":{"turn":{"id":"tu-3"}}}"#);
        let abort = parse(&d.interrupt(None).expect("a turn is in flight")[0]);
        assert_eq!(abort["params"]["turnId"], "tu-3");
    }

    #[test]
    fn codex_turn_completed_notification_also_ends_the_turn() {
        let mut d = Dialogue::new(ControlShape::CodexAppServer, config()).unwrap();
        d.open();
        d.on_line(r#"{"jsonrpc":"2.0","id":1,"result":{}}"#);
        d.on_line(r#"{"jsonrpc":"2.0","id":2,"result":{"thread":{"id":"th-1"}}}"#);
        let step = d
            .on_line(r#"{"jsonrpc":"2.0","method":"turn/completed","params":{"threadId":"th-1"}}"#);
        assert!(step.is_terminal());
    }

    #[test]
    fn acp_handshakes_opens_a_session_and_prompts() {
        let mut d = Dialogue::new(ControlShape::AcpCancel, config()).unwrap();
        let init = parse(&d.open()[0]);
        assert_eq!(init["method"], "initialize");
        assert_eq!(init["params"]["protocolVersion"], 1);
        // oneharness does not serve filesystem requests, so it says so.
        assert_eq!(
            init["params"]["clientCapabilities"]["fs"]["readTextFile"],
            false
        );

        let step = d.on_line(r#"{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":1}}"#);
        let new = parse(&step.frames()[0]);
        assert_eq!(new["method"], "session/new");
        assert_eq!(new["params"]["cwd"], absolute_text_for_test(WORK));

        let step = d.on_line(r#"{"jsonrpc":"2.0","id":2,"result":{"sessionId":"sess-7"}}"#);
        assert_eq!(d.session_id(), Some("sess-7"));
        let prompt = parse(&step.frames()[0]);
        assert_eq!(prompt["method"], "session/prompt");
        assert_eq!(prompt["params"]["sessionId"], "sess-7");
        assert_eq!(prompt["params"]["prompt"][0]["text"], "do the thing");

        // Cancel is a NOTIFICATION: with an `id`, goose answers -32601 and the
        // work carries on.
        let frames = d.interrupt(None).expect("a turn is in flight");
        let cancel = parse(&frames[0]);
        assert_eq!(cancel["method"], "session/cancel");
        assert_eq!(cancel["params"]["sessionId"], "sess-7");
        assert!(cancel.get("id").is_none(), "cancel must carry no id");

        // Text comes off the update stream.
        d.on_line(
            r#"{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"sess-7","update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"hi"}}}}"#,
        );
        assert_eq!(d.text().as_deref(), Some("hi"));
        // An unrelated update is not text.
        d.on_line(
            r#"{"jsonrpc":"2.0","method":"session/update","params":{"update":{"sessionUpdate":"tool_call","title":"bash"}}}"#,
        );
        assert_eq!(d.text().as_deref(), Some("hi"));

        // The prompt response ends the turn — even reporting `end_turn` after a
        // real cancellation, which is exactly why oneharness records the
        // interrupt from its own side instead of reading this.
        let step = d.on_line(r#"{"jsonrpc":"2.0","id":3,"result":{"stopReason":"end_turn"}}"#);
        assert!(step.is_terminal());
    }

    #[test]
    fn an_acp_permission_request_is_always_answered() {
        // Goose and Copilot block forever until the client replies, so a
        // request must never fall through unanswered.
        let mut d = Dialogue::new(ControlShape::AcpCancel, config()).unwrap();
        let step = d.on_line(
            r#"{"jsonrpc":"2.0","id":41,"method":"session/request_permission","params":{"options":[{"optionId":"a","kind":"allow_once"},{"optionId":"r","kind":"reject_once"}]}}"#,
        );
        assert_eq!(step.frames().len(), 1, "a request must be answered");
        let reply = parse(&step.frames()[0]);
        assert_eq!(reply["id"], 41);
        assert_eq!(reply["result"]["outcome"]["outcome"], "selected");
        assert_eq!(reply["result"]["outcome"]["optionId"], "a");
    }

    #[test]
    fn edit_mode_declines_rather_than_granting_shell_authority_it_denies() {
        // `edit` promises auto-approved edits with shell still gated, and an ACP
        // permission request carries no sourced way to tell those apart — so a
        // blanket grant here would hand the agent authority the mode refuses.
        // The mode is not refused any more: copilot's own permission flags ride
        // its `--acp` launch, so the CLI enforces `edit` and this answer is the
        // backstop for an ask that mapping did not already decide.
        let mut d = Dialogue::new(
            ControlShape::AcpCancel,
            DialogueConfig {
                mode: PermissionMode::Edit,
                posture: ApprovalPosture::Gated,
                ..config()
            },
        )
        .unwrap();
        let reply = parse(
            &d.on_line(
                r#"{"jsonrpc":"2.0","id":9,"method":"session/request_permission","params":{"options":[{"optionId":"a","kind":"allow_once"}]}}"#,
            )
            .frames()[0],
        );
        assert_eq!(
            reply["result"]["outcome"]["outcome"], "cancelled",
            "edit must not select an allow option it cannot narrow"
        );
    }

    #[test]
    fn a_restrictive_mode_declines_instead_of_granting() {
        let mut d = Dialogue::new(
            ControlShape::AcpCancel,
            DialogueConfig {
                mode: PermissionMode::Default,
                posture: ApprovalPosture::Gated,
                ..config()
            },
        )
        .unwrap();
        let reply = parse(
            &d.on_line(
                r#"{"jsonrpc":"2.0","id":7,"method":"session/request_permission","params":{"options":[{"optionId":"a","kind":"allow_once"}]}}"#,
            )
            .frames()[0],
        );
        assert_eq!(reply["result"]["outcome"]["outcome"], "cancelled");

        let mut codex = Dialogue::new(
            ControlShape::CodexAppServer,
            DialogueConfig {
                mode: PermissionMode::ReadOnly,
                posture: ApprovalPosture::Gated,
                ..config()
            },
        )
        .unwrap();
        let reply = parse(
            &codex
                .on_line(r#"{"jsonrpc":"2.0","id":5,"method":"execCommandApproval","params":{}}"#)
                .frames()[0],
        );
        assert_eq!(reply["result"]["decision"], "denied");
        assert_eq!(codex.codex_sandbox_policy()["type"], "readOnly");
    }

    /// Every mode's sandbox, pinned against Codex's own `SandboxPolicy` shape.
    ///
    /// `bypass` is the one that matters: it must ask for no sandbox, matching
    /// what the same mode maps to on `codex exec`. A `workspaceWrite` here runs
    /// each shell call under bubblewrap in a fresh network namespace, and a host
    /// that cannot create one fails every call before it executes — a turn that
    /// silently does nothing rather than an error anyone can see.
    #[test]
    fn codex_sandbox_follows_the_mode_the_run_asked_for() {
        let policy = |mode| {
            Dialogue::new(
                ControlShape::CodexAppServer,
                DialogueConfig {
                    mode,
                    posture: ApprovalPosture::of(mode),
                    ..config()
                },
            )
            .unwrap()
            .codex_sandbox_policy()
        };

        let bypass = policy(PermissionMode::Bypass);
        assert_eq!(bypass["type"], "dangerFullAccess");
        assert!(
            bypass.get("writableRoots").is_none(),
            "full access takes no roots: {bypass}"
        );

        // `auto` is the mode that does want the narrow grant.
        let auto = policy(PermissionMode::Auto);
        assert_eq!(auto["type"], "workspaceWrite");
        assert_eq!(auto["writableRoots"][0], absolute_text_for_test(WORK));
        assert_eq!(auto["networkAccess"], false);

        for mode in [
            PermissionMode::ReadOnly,
            PermissionMode::Plan,
            PermissionMode::Default,
            PermissionMode::Edit,
        ] {
            assert_eq!(
                policy(mode)["type"],
                "readOnly",
                "{mode:?} promises no mutation"
            );
        }
    }

    #[test]
    fn codex_answers_an_unrecognized_request_without_granting_it() {
        // Answering is mandatory — an unanswered request stalls the turn — but a
        // permissive mode means "act without asking", not "approve any method
        // the server invents".
        let mut d = Dialogue::new(ControlShape::CodexAppServer, config()).unwrap();
        let reply = parse(
            &d.on_line(r#"{"jsonrpc":"2.0","id":8,"method":"attestation/generate","params":{}}"#)
                .frames()[0],
        );
        assert_eq!(reply["id"], 8, "the request is still answered");
        assert!(
            reply["result"].get("decision").is_none(),
            "an unrecognized method gets no approval: {reply}"
        );

        let approved = parse(
            &d.on_line(
                r#"{"jsonrpc":"2.0","id":9,"method":"item/commandExecution/requestApproval","params":{}}"#,
            )
            .frames()[0],
        );
        assert_eq!(approved["result"]["decision"], "approved");
    }

    #[test]
    fn allow_option_prefers_a_one_shot_grant() {
        let options = json!([
            {"optionId": "always", "kind": "allow_always"},
            {"optionId": "once", "kind": "allow_once"},
            {"optionId": "no", "kind": "reject_once"},
        ]);
        assert_eq!(acp_allow_option(&options).as_deref(), Some("once"));
        let only_always = json!([{"optionId": "always", "kind": "allow_always"}]);
        assert_eq!(acp_allow_option(&only_always).as_deref(), Some("always"));
        assert_eq!(acp_allow_option(&json!([])), None);
        // No option says it allows anything, so nothing is granted — picking
        // the first would grant a permission nobody chose. `disallow_once` is
        // the trap a substring read falls into: it contains "allow".
        let none_allow = json!([
            {"optionId": "reject", "kind": "reject_once"},
            {"optionId": "mystery", "kind": "something_new"},
            {"optionId": "no", "kind": "disallow_once"},
            {"optionId": "unnamed"},
        ]);
        assert_eq!(acp_allow_option(&none_allow), None);
    }

    #[test]
    fn a_server_error_ends_the_turn_rather_than_hanging() {
        let mut d = Dialogue::new(ControlShape::AcpCancel, config()).unwrap();
        d.open();
        let step = d.on_line(
            r#"{"jsonrpc":"2.0","id":1,"error":{"code":-32603,"message":"Internal error"}}"#,
        );
        assert!(step.is_terminal(), "there is nothing further to drive");
        assert_eq!(d.session_id(), None);
        assert_eq!(d.text(), None);
    }

    #[test]
    fn an_uncorrelated_server_error_does_not_end_the_active_dialogue() {
        let mut d = Dialogue::new(ControlShape::AcpCancel, config()).unwrap();
        d.open();
        let step = d.on_line(
            r#"{"jsonrpc":"2.0","id":99,"error":{"code":-32603,"message":"other request"}}"#,
        );
        assert_eq!(step, DialogueStep::Continue);

        let handshake = d.on_line(r#"{"jsonrpc":"2.0","id":1,"result":{}}"#);
        assert_eq!(handshake.frames().len(), 1, "the dialogue must remain live");
    }

    #[test]
    fn a_session_the_server_never_named_ends_the_turn() {
        // No identifier means no turn to address and no handle to resume, so
        // sending a prompt would create work nobody could interrupt.
        let mut d = Dialogue::new(ControlShape::AcpCancel, config()).unwrap();
        d.open();
        d.on_line(r#"{"jsonrpc":"2.0","id":1,"result":{}}"#);
        let step = d.on_line(r#"{"jsonrpc":"2.0","id":2,"result":{}}"#);
        assert!(step.is_terminal());
        assert!(step.frames().is_empty());
    }

    #[test]
    fn noise_and_partial_lines_are_ignored() {
        let mut d = Dialogue::new(ControlShape::CodexAppServer, config()).unwrap();
        d.open();
        for noise in ["", "   ", "not json", "{}", r#"{"jsonrpc":"2.0"}"#] {
            assert_eq!(d.on_line(noise), DialogueStep::default(), "{noise}");
        }
    }

    /// Drive a codex conversation to the point where a turn is in flight.
    fn codex_in_turn() -> Dialogue {
        let mut d = Dialogue::new(ControlShape::CodexAppServer, config()).unwrap();
        d.open();
        d.on_line(r#"{"jsonrpc":"2.0","id":1,"result":{}}"#);
        d.on_line(r#"{"jsonrpc":"2.0","id":2,"result":{"thread":{"id":"th-1"}}}"#);
        d.on_line(r#"{"jsonrpc":"2.0","method":"turn/started","params":{"threadId":"th-1","turn":{"id":"tu-9"}}}"#);
        d
    }

    #[test]
    fn a_codex_interrupt_carrying_a_redirection_opens_the_next_turn_when_this_one_ends() {
        // Codex refuses a second turn on a thread that already has one, so the
        // redirection is held from the moment the interrupt is served until
        // `turn/completed` — the first point a new turn can carry it. The
        // supervisor's message is the run's from the interrupt onwards, which is
        // what makes the two one operation.
        let mut d = codex_in_turn();
        d.on_line(
            r#"{"jsonrpc":"2.0","method":"item/agentMessage/delta","params":{"itemId":"m1","delta":"wrong path"}}"#,
        );
        let abort = parse(
            &d.interrupt(Some(&redirection("do X instead")))
                .expect("a turn is in flight")[0],
        );
        assert_eq!(abort["method"], "turn/interrupt");
        assert!(d.has_pending_redirect());

        // The aborted turn ending is delivery, not the end of the conversation.
        let step = d.on_line(r#"{"jsonrpc":"2.0","method":"turn/completed","params":{}}"#);
        assert!(
            !step.is_terminal(),
            "the redirection still has to be carried"
        );
        assert!(!d.has_pending_redirect(), "it was handed over exactly once");
        let next = parse(&step.frames()[0]);
        assert_eq!(next["method"], "turn/start");
        assert_eq!(next["params"]["threadId"], "th-1", "same thread, new turn");
        assert_eq!(next["params"]["input"][0]["text"], "do X instead");
        // Negotiated in the run's own posture and directory, exactly as the
        // first turn was — a redirection is not a chance to widen either.
        assert_eq!(next["params"]["cwd"], absolute_text_for_test(WORK));
        assert_eq!(next["params"]["sandboxPolicy"]["type"], "dangerFullAccess");
        assert_eq!(next["params"]["approvalPolicy"], "never");

        // The new turn is interruptible in its own right, once codex names it.
        assert!(
            d.interrupt(None).is_none(),
            "the old turn's id must not address the new one"
        );
        d.on_line(r#"{"jsonrpc":"2.0","method":"turn/started","params":{"turn":{"id":"tu-10"}}}"#);
        let again = parse(&d.interrupt(None).expect("the redirected turn is in flight")[0]);
        assert_eq!(again["params"]["turnId"], "tu-10");

        // And it ends the conversation, because nothing is queued behind it.
        let step = d.on_line(r#"{"jsonrpc":"2.0","method":"turn/completed","params":{}}"#);
        assert!(step.is_terminal());

        // Both answers are kept, separated rather than run together into one
        // sentence that changes its mind.
        d.on_line(
            r#"{"jsonrpc":"2.0","method":"item/agentMessage/delta","params":{"itemId":"m2","delta":"ignored"}}"#,
        );
        assert_eq!(d.text().as_deref(), Some("wrong path"));
    }

    #[test]
    fn an_acp_interrupt_carrying_a_redirection_prompts_the_same_session_again() {
        let mut d = Dialogue::new(ControlShape::AcpCancel, config()).unwrap();
        d.open();
        d.on_line(r#"{"jsonrpc":"2.0","id":1,"result":{}}"#);
        d.on_line(r#"{"jsonrpc":"2.0","id":2,"result":{"sessionId":"sess-7"}}"#);
        let cancel = parse(
            &d.interrupt(Some(&redirection("do X instead")))
                .expect("in flight")[0],
        );
        assert_eq!(cancel["method"], "session/cancel");
        assert!(cancel.get("id").is_none(), "cancel is still a notification");

        // ACP ends the turn on the prompt response — reporting `end_turn` even
        // after a real cancellation — and that is where the redirection lands.
        let step = d.on_line(r#"{"jsonrpc":"2.0","id":3,"result":{"stopReason":"cancelled"}}"#);
        assert!(!step.is_terminal());
        let next = parse(&step.frames()[0]);
        assert_eq!(next["method"], "session/prompt");
        assert_eq!(
            next["params"]["sessionId"], "sess-7",
            "the session survived"
        );
        assert_eq!(next["params"]["prompt"][0]["text"], "do X instead");

        // ACP's answer to the redirected prompt is the END of that turn, so
        // there is nothing to wait for: the write is the delivery.
        assert!(!d.awaits_redirect_answer());
        let step = d.on_line(r#"{"jsonrpc":"2.0","id":4,"result":{"stopReason":"end_turn"}}"#);
        assert!(step.is_terminal(), "nothing is queued behind the redirect");
        assert!(d.take_redirect_answer().is_none());
    }

    #[test]
    fn a_codex_redirected_turn_is_delivered_on_the_server_s_answer_to_it() {
        // Codex acknowledges `turn/start` at once, so its answer — not the
        // write — says whether the replacement turn exists. An acknowledged one
        // is delivered; a refused one is not, in the server's own words.
        for (answer, expected) in [
            (
                r#"{"jsonrpc":"2.0","id":5,"result":{"turn":{"id":"tu-10","status":"inProgress"}}}"#,
                Ok(()),
            ),
            (
                r#"{"jsonrpc":"2.0","id":5,"error":{"code":-32600,"message":"thread not found: th-1"}}"#,
                Err("thread not found: th-1".to_string()),
            ),
            // An answer that names no turn is no evidence the turn exists.
            (
                r#"{"jsonrpc":"2.0","id":5,"result":{}}"#,
                Err(r#"the server answered the replacement `turn/start` without naming a turn: {"id":5,"jsonrpc":"2.0","result":{}}"#.to_string()),
            ),
        ] {
            let mut d = codex_in_turn();
            d.interrupt(Some(&redirection("do X instead"))).unwrap();
            assert!(!d.awaits_redirect_answer(), "no replacement turn yet");
            let step = d.on_line(r#"{"jsonrpc":"2.0","method":"turn/completed","params":{}}"#);
            // `turn/start` is request 5: the interrupt itself was 4.
            assert_eq!(parse(&step.frames()[0])["id"], 5, "the replacement turn");
            assert!(d.awaits_redirect_answer());
            assert!(d.take_redirect_answer().is_none(), "not answered yet");
            // An unrelated answer is not the replacement turn's.
            d.on_line(r#"{"jsonrpc":"2.0","id":99,"result":{}}"#);
            assert!(d.awaits_redirect_answer());
            let ended = d.on_line(answer);
            assert_eq!(d.take_redirect_answer(), Some(expected.clone()));
            assert!(d.take_redirect_answer().is_none(), "read once");
            assert!(!d.awaits_redirect_answer());
            // Refused, there is no replacement turn to follow: the conversation
            // ends rather than waiting for one.
            assert_eq!(ended.is_terminal(), expected.is_err(), "{answer}");
        }
    }

    #[test]
    fn a_redirection_is_only_taken_over_when_the_abort_it_rides_exists() {
        // The supervisor still owns its message unless the turn was really
        // stopped: taking it and then refusing the interrupt would be the exact
        // window this feature exists to close.
        let mut before_the_turn = Dialogue::new(ControlShape::CodexAppServer, config()).unwrap();
        before_the_turn.open();
        assert!(before_the_turn
            .interrupt(Some(&redirection("too early")))
            .is_none());
        assert!(!before_the_turn.has_pending_redirect());

        // In flight, but codex has not named the turn yet, so there is no id to
        // abort against.
        let mut unnamed = Dialogue::new(ControlShape::CodexAppServer, config()).unwrap();
        unnamed.open();
        unnamed.on_line(r#"{"jsonrpc":"2.0","id":1,"result":{}}"#);
        unnamed.on_line(r#"{"jsonrpc":"2.0","id":2,"result":{"thread":{"id":"th-1"}}}"#);
        assert!(unnamed
            .interrupt(Some(&redirection("no turn id yet")))
            .is_none());
        assert!(!unnamed.has_pending_redirect());

        // And one whose abort could not be written is given back, so the turn
        // that ends afterwards ends rather than carrying a message nobody
        // believes was delivered.
        let mut abandoned = codex_in_turn();
        assert!(abandoned.interrupt(Some(&redirection("dropped"))).is_some());
        abandoned.abandon_redirect();
        assert!(!abandoned.has_pending_redirect());
        let step = abandoned.on_line(r#"{"jsonrpc":"2.0","method":"turn/completed","params":{}}"#);
        assert!(step.is_terminal());
    }

    #[test]
    fn a_redirection_is_attempted_once_even_when_the_turn_ends_badly() {
        // A server error ends the turn too, and the message has been committed,
        // so it is still offered the one delivery it was promised — and then the
        // conversation ends rather than retrying forever.
        let mut d = codex_in_turn();
        d.interrupt(Some(&redirection("do X instead"))).unwrap();
        let step =
            d.on_line(r#"{"jsonrpc":"2.0","id":3,"error":{"code":-32603,"message":"boom"}}"#);
        assert!(!step.is_terminal());
        let next = parse(&step.frames()[0]);
        assert_eq!(next["method"], "turn/start");
        let step = d.on_line(&format!(
            r#"{{"jsonrpc":"2.0","id":{},"error":{{"code":-32603,"message":"boom"}}}}"#,
            next["id"]
        ));
        assert!(step.is_terminal(), "one attempt, not a loop");
    }

    #[test]
    fn text_is_never_fabricated() {
        let mut d = Dialogue::new(ControlShape::CodexAppServer, config()).unwrap();
        assert_eq!(d.text(), None);
        d.on_line(
            r#"{"jsonrpc":"2.0","method":"item/agentMessage/delta","params":{"delta":"   "}}"#,
        );
        assert_eq!(d.text(), None, "whitespace is not an answer");
        assert_eq!(d.text_source(), "jsonrpc:codex-app-server");
    }
}
