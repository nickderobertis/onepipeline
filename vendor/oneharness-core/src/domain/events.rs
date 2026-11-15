//! Best-effort extraction of a harness's **normalized tool-call / action events**
//! from its raw stdout. Pure: no I/O.
//!
//! Like `text` and `usage`, this is a convenience layered over the guaranteed
//! execution envelope: the array is `None` when a harness's output carries no
//! recognizable tool trace, is **never fabricated**, and records how it was
//! recovered (`events_source`, parallel to `text_source`) so a consumer can tell
//! "this harness doesn't expose tool events" from "no tools were used." An absent
//! trace is the honest answer, not an error.
//!
//! The motivation is behavioral skill-testing (the `skilltest` consumer, issue
//! #1096): asserting what a harness actually *did* ("ran `git commit`", "edited
//! exactly one file", "used ≤ 3 tool calls") — none of which the text-only
//! envelope could express. Normalizing here, where the per-harness output shapes
//! are already known, spares every consumer from per-harness stdout parsing.
//!
//! Five output shapes are recognized, all sourced from real transcripts captured
//! from the live CLIs (the `explore-events` CI probe), not guessed:
//! - **OpenCode** (`run --format json`): JSONL events whose `part.type == "tool"`
//!   carry the tool `name` (`part.tool`) and a `state` object holding the call
//!   `input` and observed `output`. One part is one completed call → one
//!   `tool_call` event carrying its `output`. A completed `text` part becomes a
//!   `message` event and a completed `reasoning` part a `reasoning` event, one
//!   per part id, carrying the part's `text`.
//! - **Anthropic content blocks** (Claude Code `stream-json`, Qwen `stream-json`
//!   / `json`): messages whose `message.content[]` holds `tool_use` blocks
//!   (`name` + structured `input`) and `tool_result` blocks (the observation) —
//!   `tool_call` and `tool_result` events respectively.
//! - **Cursor** (`--output-format stream-json`): top-level `type:"tool_call"`
//!   events whose `tool_call` object nests a `<name>ToolCall` payload (e.g.
//!   `shellToolCall`) with `args` and, once complete, `result.success`. The tool
//!   name is the payload key minus its `ToolCall` suffix.
//! - **Codex** (`exec --json`): flat `item.started` / `item.completed` lifecycle
//!   events for command, MCP, collaboration, and web-search tool items.
//! - **Codex app-server** (controlled turns): JSON-RPC `item/started` /
//!   `item/completed` notifications whose `params.item` carries the tool item.
//!
//! Goose, Crush, and Copilot expose no machine-readable transcript headlessly
//! (decorative TUI text, or no JSON output mode at all — confirmed by the probe),
//! and Claude Code / Cursor under their non-stream `json` mode collapse to only a
//! final result object. In those cases `events` stays `None` rather than being
//! invented. Which format yields a transcript per harness is declared by
//! `HarnessSpec.events_format` and selected by `run --events` / `--stream`.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::domain::report::{OutputFormat, OutputObservation, Status};

/// One normalized action a harness took, harness-agnostic so a single consumer
/// assertion works across harnesses. Every field is always serialized (null when
/// absent) so the shape is stable, mirroring the `usage` contract.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ActionEvent {
    /// The kind of event: `tool_call` (the model invoked a tool),
    /// `tool_result` (the observation returned to the model), `message` (the
    /// agent's own text, in `output`) or `reasoning` (its reasoning text, in
    /// `output`). A `message` or `reasoning` event is one finished item, never
    /// a token delta. Left open for future kinds rather than an enum, so a new
    /// shape never breaks the field — a consumer counting tool calls filters on
    /// it (see [`ActionEvent::is_tool_activity`]).
    pub kind: String,
    /// Normalized tool name where knowable (e.g. `bash`, `Edit`); `null` when
    /// the harness did not name the tool. A `tool_result` carries the name of
    /// the `tool_call` it answers (matched by `tool_call_id`) when that call
    /// was seen earlier in the same run, else `null`.
    pub name: Option<String>,
    /// Structured, tool-shaped arguments (the command string, the file path),
    /// so a consumer asserts on specific args without re-parsing; `null` when the
    /// event carries none (e.g. a `tool_result`).
    pub input: Option<Value>,
    /// The result/observation text, when the trace exposes it; `null` otherwise.
    pub output: Option<String>,
    /// Position of this event within the run, so "did X before Y" is
    /// expressible from a stable ordering (also array order). It counts every
    /// kind, so tool calls interleaved with `message`/`reasoning` events do not
    /// carry contiguous indexes; count tool calls by `kind`, not by `index`.
    pub index: usize,
    /// Stable call identity within the session. Present on tool calls and their
    /// matching results when the provider exposes an identity; history fills a
    /// deterministic run-local identity for providers that do not.
    pub tool_call_id: Option<String>,
    /// UTC interval bounds for tool execution, populated on history records.
    pub started_at: Option<String>,
    pub finished_at: Option<String>,
    /// Monotonic elapsed tool time. `None` means no terminal boundary was seen.
    pub duration_ms: Option<u128>,
    /// Terminal tool state, populated on history tool-call events. On a
    /// `tool_result` it is the outcome the harness reported for the call:
    /// `failed` when the result is marked an error (a Claude Code / Qwen
    /// `is_error: true` block), else `completed`; `null` when the harness
    /// reports no error state for its results, and in a history record with
    /// no tool timing, which makes no tool-state claim on any event.
    pub status: Option<ToolCallStatus>,
    /// Provenance for the tool interval. Omitted when timing is unavailable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    // llmlint: ignore[invalid_states_unrepresentable] ActionEvent is a stable additive wire shape shared by live in-progress and terminal events; lifecycle combinations are necessarily optional and producer/history validation governs which fields are available at each phase.
    pub timing_source: Option<TimingSource>,
}

impl ActionEvent {
    /// Whether this event records tool activity (`tool_call` / `tool_result`)
    /// rather than the agent's own `message` or `reasoning` text.
    #[must_use]
    pub fn is_tool_activity(&self) -> bool {
        matches!(self.kind.as_str(), "tool_call" | "tool_result")
    }
}

/// How a normalized tool interval was obtained.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TimingSource {
    /// Derived from provider lifecycle records with explicit request boundaries.
    ProviderMeasured,
    /// Derived from JSON records as they crossed oneharness's stdout pipe.
    StdoutObserved,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ToolCallStatus {
    Completed,
    Failed,
    Timeout,
    Interrupted,
}

/// A recovered event list plus the method that produced it (e.g.
/// `json:opencode-parts`, `stream-json:content-blocks`), parallel to
/// [`crate::domain::normalize::Extracted::source`].
#[derive(Debug, Clone, PartialEq)]
pub struct EventsReading {
    pub events: Vec<ActionEvent>,
    pub source: String,
}

/// Where a recognized record sits in its item's lifecycle: `Open` is a start or
/// an update of an item still running (codex `item.started` / `item.updated`,
/// an opencode part still `running`), `Finished` is the record that ends it.
/// Only a finished item is delivered; an open one is held so its fields can
/// fill in whatever the finishing record leaves out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    Open,
    Finished,
}

/// One event before it is assigned its ordering index; the recognizers produce
/// these and [`EventStream`] numbers them in the order they finish.
#[derive(Debug)]
struct PartialEvent {
    kind: &'static str,
    name: Option<String>,
    input: Option<Value>,
    output: Option<String>,
    tool_call_id: Option<String>,
    status: Option<ToolCallStatus>,
    phase: Phase,
    /// The id a finished item is delivered under at most once: an OpenCode
    /// prose part, which a stream may send again under the same id.
    item_id: Option<String>,
}

impl PartialEvent {
    /// A finished `message` or `reasoning` item carrying `text`.
    fn text(kind: &'static str, text: String) -> Self {
        PartialEvent {
            kind,
            name: None,
            input: None,
            output: Some(text),
            tool_call_id: None,
            status: None,
            phase: Phase::Finished,
            item_id: None,
        }
    }

    fn into_event(self, index: usize) -> ActionEvent {
        ActionEvent {
            kind: self.kind.to_string(),
            name: self.name,
            input: self.input,
            output: self.output,
            index,
            tool_call_id: self.tool_call_id,
            started_at: None,
            finished_at: None,
            duration_ms: None,
            status: self.status,
            timing_source: None,
        }
    }

    /// `self` (the later record) with every field it left empty filled from
    /// `earlier`, the open record of the same call.
    fn over(self, earlier: PartialEvent) -> PartialEvent {
        PartialEvent {
            name: self.name.or(earlier.name),
            input: self.input.or(earlier.input),
            output: self.output.or(earlier.output),
            status: self.status.or(earlier.status),
            ..self
        }
    }
}

/// Best-effort normalized events from a harness's stdout. Scans every JSON
/// candidate (the whole document, each array element, or each parseable line) in
/// order, collecting events from whichever known shape each candidate matches.
/// `None` when no candidate is recognized, so the consumer can distinguish
/// "unsupported" from "used no tools" via the absent `events_source`. `fmt` only
/// labels the source's provenance prefix.
///
/// This is the same fold a streaming run makes line by line (`EventStream`),
/// so a streamed event and the report's event for the same item carry the same
/// `index`. Items are ordered by when they finished; a call that started and
/// never finished (the run timed out or was cancelled mid-call) follows them.
pub fn extract_events(stdout: &str, fmt: OutputFormat) -> Option<EventsReading> {
    let mut stream = EventStream::default();
    let mut events = Vec::new();
    for value in json_candidates(stdout) {
        events.extend(stream.push(&value));
    }
    let recognizer = stream.recognizer?;
    events.extend(stream.finish());
    Some(EventsReading {
        source: format!("{}:{recognizer}", format_prefix(fmt)),
        events,
    })
}

/// Extract the **finished** normalized events from a single already-parsed JSON
/// value (one stream line or document), numbering them from `start_index`.
/// Empty when the value carries no recognizable finished event — including an
/// open lifecycle record such as codex's `item.started`, whose call is
/// delivered once, by the record that completes it. A caller that also wants
/// an open record's fields merged into its completion folds lines through
/// [`extract_events`] instead.
pub fn events_from_value(value: &Value, start_index: usize) -> Vec<ActionEvent> {
    match recognize(value) {
        Some((_, partials)) => partials
            .into_iter()
            .filter(|partial| partial.phase == Phase::Finished)
            .enumerate()
            .map(|(i, pe)| pe.into_event(start_index + i))
            .collect(),
        None => Vec::new(),
    }
}

/// The incremental extractor behind both a streaming run's live events and
/// [`extract_events`]: each line is pushed as it arrives and yields the events
/// it *finished*, numbered in delivery order. An open item (a codex call's
/// `item.started`) is held by its call id until the record that completes it
/// arrives, so a consumer relaying every event sees each call once.
#[derive(Debug, Default)]
pub(crate) struct EventStream {
    open: Vec<PartialEvent>,
    /// `(tool_call_id, name)` of every delivered tool call, so the result that
    /// answers one can name the tool it ran.
    call_names: Vec<(String, String)>,
    next_index: usize,
    recognizer: Option<&'static str>,
    /// The `item_id` of every delivered item that carries one.
    delivered_items: Vec<String>,
}

impl EventStream {
    /// Feed one JSON line; returns the events it finished, already numbered.
    pub(crate) fn push(&mut self, value: &Value) -> Vec<ActionEvent> {
        let Some((label, partials)) = recognize(value) else {
            return Vec::new();
        };
        // Cursor's assistant messages share the Anthropic content-block shape,
        // so a turn that speaks before its first tool call matches the generic
        // reading first; the harness-specific shape names the transcript.
        if self.recognizer.is_none() || self.recognizer == Some(GENERIC_RECOGNIZER) {
            self.recognizer = Some(label);
        }
        let mut finished = Vec::new();
        for partial in partials {
            match partial.phase {
                Phase::Open => self.hold(partial),
                Phase::Finished => {
                    if let Some(id) = &partial.item_id {
                        if self.delivered_items.contains(id) {
                            continue;
                        }
                        self.delivered_items.push(id.clone());
                    }
                    let partial = self.release(partial);
                    let partial = self.named(partial);
                    finished.push(self.number(partial));
                }
            }
        }
        finished
    }

    /// The calls that were started and never finished, numbered after every
    /// delivered event. A streaming run never delivers these live; they reach
    /// the report (and history) only.
    pub(crate) fn finish(mut self) -> Vec<ActionEvent> {
        let open = std::mem::take(&mut self.open);
        open.into_iter()
            .map(|partial| self.number(partial))
            .collect()
    }

    /// A finished call recorded by id, or a result named after the call it
    /// answers.
    fn named(&mut self, mut partial: PartialEvent) -> PartialEvent {
        let Some(id) = partial.tool_call_id.clone() else {
            return partial;
        };
        match (partial.kind, &partial.name) {
            ("tool_call", Some(name)) => self.call_names.push((id, name.clone())),
            ("tool_result", None) => {
                partial.name = self
                    .call_names
                    .iter()
                    .rev()
                    .find(|(call, _)| *call == id)
                    .map(|(_, name)| name.clone());
            }
            _ => {}
        }
        partial
    }

    fn number(&mut self, partial: PartialEvent) -> ActionEvent {
        let event = partial.into_event(self.next_index);
        self.next_index += 1;
        event
    }

    fn position(&self, partial: &PartialEvent) -> Option<usize> {
        let id = partial.tool_call_id.as_deref()?;
        self.open
            .iter()
            .position(|open| open.kind == partial.kind && open.tool_call_id.as_deref() == Some(id))
    }

    fn hold(&mut self, update: PartialEvent) {
        match self.position(&update) {
            Some(at) => {
                let earlier = self.open.remove(at);
                self.open.insert(at, update.over(earlier));
            }
            None => self.open.push(update),
        }
    }

    fn release(&mut self, finished: PartialEvent) -> PartialEvent {
        match self.position(&finished) {
            Some(at) => finished.over(self.open.remove(at)),
            None => finished,
        }
    }
}

/// The recognizer label shared by more than one harness's transcript; any
/// harness-specific label seen in the same transcript replaces it.
const GENERIC_RECOGNIZER: &str = "content-blocks";

/// Try each known harness transcript shape against one JSON value, returning the
/// recognizer label (for `events_source`) and the events it yielded, or `None`.
/// The shapes are mutually exclusive in practice (each keys off a distinct field
/// layout), so the first match wins.
fn recognize(value: &Value) -> Option<(&'static str, Vec<PartialEvent>)> {
    // OpenCode `tool` part (self-contained: name + input + output), or a
    // completed `text` / `reasoning` part.
    if let Some(pe) = opencode_part_event(value) {
        return Some(("opencode-parts", vec![pe]));
    }
    // Cursor `type:"tool_call"` with a nested `<name>ToolCall` payload.
    if let Some(pe) = cursor_tool_call(value) {
        return Some(("cursor-tool-calls", vec![pe]));
    }
    // Codex executable, message and reasoning items; a call's lifecycle
    // records share its item id and are delivered once, on completion.
    if let Some(pe) = codex_item(value) {
        return Some(("codex-items", vec![pe]));
    }
    // A controlled Codex turn uses app-server JSON-RPC notifications instead
    // of `exec --json`. Recognize even non-tool item notifications so a
    // tool-free controlled transcript is an empty reading, not an absent one.
    if let Some(pe) = codex_app_server_item(value) {
        return Some(("codex-app-server-items", pe.into_iter().collect()));
    }
    // Anthropic content blocks (Claude Code / Qwen): tool_use + tool_result,
    // and an assistant message's text and thinking blocks.
    let blocks = content_block_events(value);
    if !blocks.is_empty() {
        return Some((GENERIC_RECOGNIZER, blocks));
    }
    None
}

/// One OpenCode part → at most one event. A `tool` part is a single
/// `tool_call` event carrying its input and output: one still
/// `pending`/`running` is open; any other state (or none) finishes it. A `text`
/// part is a `message` event and a `reasoning` part a `reasoning` event,
/// carrying the part's text and keyed by the part id so a part sent again is
/// delivered once; one whose `time` carries no numeric `end` is unfinished, and
/// one with blank text is no event. `None` for any other part (`step-start`, …).
fn opencode_part_event(value: &Value) -> Option<PartialEvent> {
    let part = value.get("part").and_then(Value::as_object)?;
    match part.get("type").and_then(Value::as_str) {
        Some("tool") => {}
        Some("text") => return opencode_prose(part, OpenCodeProse::Text),
        Some("reasoning") => return opencode_prose(part, OpenCodeProse::Reasoning),
        _ => return None,
    }
    let state = part.get("state").and_then(Value::as_object);
    let phase = match state.and_then(|s| s.get("status")).and_then(Value::as_str) {
        Some("pending" | "running") => Phase::Open,
        _ => Phase::Finished,
    };
    Some(PartialEvent {
        kind: "tool_call",
        name: part.get("tool").and_then(Value::as_str).map(str::to_string),
        input: state.and_then(|s| s.get("input").cloned()),
        output: state
            .and_then(|s| s.get("output"))
            .and_then(Value::as_str)
            .map(str::to_string),
        tool_call_id: part
            .get("callID")
            .or_else(|| part.get("id"))
            .and_then(Value::as_str)
            .map(str::to_string),
        status: None,
        phase,
        item_id: None,
    })
}

#[derive(Clone, Copy)]
enum OpenCodeProse {
    Text,
    Reasoning,
}

impl OpenCodeProse {
    fn kind(self) -> &'static str {
        match self {
            Self::Text => "message",
            Self::Reasoning => "reasoning",
        }
    }
}

/// A finished OpenCode `text` / `reasoning` part as its event, keyed by its
/// part id; `None` while its `time` carries no numeric `end`, or for blank text.
fn opencode_prose(
    part: &serde_json::Map<String, Value>,
    prose: OpenCodeProse,
) -> Option<PartialEvent> {
    if part
        .get("time")
        .is_some_and(|time| !time.get("end").is_some_and(Value::is_number))
    {
        return None;
    }
    let text = part
        .get("text")
        .and_then(Value::as_str)
        .filter(|text| !text.trim().is_empty())?;
    Some(PartialEvent {
        item_id: part.get("id").and_then(Value::as_str).map(str::to_string),
        ..PartialEvent::text(prose.kind(), text.to_string())
    })
}

/// Anthropic-style content blocks (Claude Code / Qwen / Cursor `stream-json`): a
/// message whose `content` array (under `message.content`, or top-level
/// `content`) holds `tool_use` blocks (→ `tool_call`), `tool_result` blocks (→
/// `tool_result`) and, on an **assistant** message, non-empty `text` (→
/// `message`) and `thinking` (→ `reasoning`) blocks. Each such line is a
/// finished content block — the token deltas stream as separate `stream_event`
/// lines, which carry no `content` array and are never an event. Empty when the
/// value is not such a message or carries no recognized block.
fn content_block_events(value: &Value) -> Vec<PartialEvent> {
    let message = value.get("message");
    let blocks = message
        .and_then(|m| m.get("content"))
        .or_else(|| value.get("content"))
        .and_then(Value::as_array);
    let Some(blocks) = blocks else {
        return Vec::new();
    };
    let assistant = value.get("type").and_then(Value::as_str) == Some("assistant")
        || message.and_then(|m| m.get("role")).and_then(Value::as_str) == Some("assistant");
    let block_text = |obj: &serde_json::Map<String, Value>, key: &str| {
        obj.get(key)
            .and_then(Value::as_str)
            .filter(|text| !text.trim().is_empty())
            .map(str::to_string)
    };
    let mut out = Vec::new();
    for block in blocks {
        let Some(obj) = block.as_object() else {
            continue;
        };
        match obj.get("type").and_then(Value::as_str) {
            Some("tool_use") => out.push(PartialEvent {
                kind: "tool_call",
                name: obj.get("name").and_then(Value::as_str).map(str::to_string),
                input: obj.get("input").cloned(),
                output: None,
                tool_call_id: obj.get("id").and_then(Value::as_str).map(str::to_string),
                status: None,
                phase: Phase::Finished,
                item_id: None,
            }),
            Some("tool_result") => out.push(PartialEvent {
                kind: "tool_result",
                name: None,
                input: None,
                output: tool_result_text(obj.get("content")),
                tool_call_id: obj
                    .get("tool_use_id")
                    .and_then(Value::as_str)
                    .map(str::to_string),
                status: Some(
                    if obj.get("is_error").and_then(Value::as_bool) == Some(true) {
                        ToolCallStatus::Failed
                    } else {
                        ToolCallStatus::Completed
                    },
                ),
                phase: Phase::Finished,
                item_id: None,
            }),
            Some("text") if assistant => {
                if let Some(text) = block_text(obj, "text") {
                    out.push(PartialEvent::text("message", text));
                }
            }
            Some("thinking") if assistant => {
                if let Some(text) = block_text(obj, "thinking") {
                    out.push(PartialEvent::text("reasoning", text));
                }
            }
            _ => {}
        }
    }
    out
}

/// Cursor's `stream-json` tool event: a top-level `{"type":"tool_call", ...}`
/// whose `tool_call` object holds a nested `<name>ToolCall` payload (e.g.
/// `shellToolCall`) with `args` (the input) and, once complete, a
/// `result.success` (the observation). The tool identity is the payload *key*,
/// not a string field — so the name is that key with its `ToolCall` suffix
/// stripped (`shellToolCall` → `shell`). Emitted only on the `completed` subtype
/// (which carries the result); the paired `started` event is skipped so a call
/// is counted once. `None` for any other line. Sourced from a real cursor-agent
/// transcript, not guessed.
fn cursor_tool_call(value: &Value) -> Option<PartialEvent> {
    let obj = value.as_object()?;
    if obj.get("type").and_then(Value::as_str) != Some("tool_call") {
        return None;
    }
    // Only the terminal event carries the result; skip `started` to avoid dupes.
    if obj.get("subtype").and_then(Value::as_str) != Some("completed") {
        return None;
    }
    let tool_call = obj.get("tool_call").and_then(Value::as_object)?;
    // The payload key ends in `ToolCall` (e.g. `shellToolCall`), distinct from the
    // sibling `toolCallId` metadata; its value is the tool object.
    let (key, payload) = tool_call
        .iter()
        .find(|(k, v)| k.ends_with("ToolCall") && v.is_object())?;
    let name = key.strip_suffix("ToolCall").unwrap_or(key).to_string();
    let payload = payload.as_object()?;
    Some(PartialEvent {
        kind: "tool_call",
        name: Some(name),
        input: payload.get("args").cloned(),
        // The observation lives under result.success (shape varies per tool); pull
        // a stdout string when present, else leave null rather than fabricating.
        output: payload
            .get("result")
            .and_then(|r| r.get("success"))
            .and_then(|s| s.get("stdout"))
            .and_then(Value::as_str)
            .map(str::to_string),
        tool_call_id: obj
            .get("call_id")
            .or_else(|| tool_call.get("toolCallId"))
            .and_then(Value::as_str)
            .map(str::to_string),
        status: None,
        phase: Phase::Finished,
        item_id: None,
    })
}

/// Codex `exec --json` items. A tool item's started, updated, and completed
/// records share `item.id`; the started/updated ones are open and only the
/// completed one finishes the call, carrying its terminal `status` and — for a
/// command — the `exit_code` codex reported, inside `input`. The allow-list
/// follows Codex's typed exec interface. `file_change` items are included:
/// codex-cli 0.145.0 emits them with both `item.started` (status `in_progress`)
/// and `item.completed` (status `completed`), so they carry a real execution
/// boundary. A `file_change` seen only as `item.completed` (an older,
/// completion-only shape) still normalizes but lacks a start, so timing stays
/// honestly incomplete rather than being fabricated. A completed
/// `agent_message` is a `message` event and a completed `reasoning` item a
/// `reasoning` event (codex exec emits both only on completion).
fn codex_item(value: &Value) -> Option<PartialEvent> {
    let obj = value.as_object()?;
    let phase = match obj.get("type").and_then(Value::as_str) {
        Some("item.started" | "item.updated") => Phase::Open,
        Some("item.completed") => Phase::Finished,
        _ => return None,
    };
    let item = obj.get("item").and_then(Value::as_object)?;
    let item_type = item.get("type").and_then(Value::as_str)?;
    if let Some(kind) = codex_text_kind(item_type) {
        if phase == Phase::Open {
            return None;
        }
        let text = item
            .get("text")
            .and_then(Value::as_str)
            .filter(|text| !text.trim().is_empty())?;
        return Some(PartialEvent::text(kind, text.to_string()));
    }
    if !is_codex_tool_type(item_type) {
        return None;
    }
    let (name, input, output) = match item_type {
        "command_execution" => (
            "command_execution".to_string(),
            item.get("command").map(|command| {
                command_input(
                    command.clone(),
                    (phase == Phase::Finished)
                        .then(|| item.get("exit_code"))
                        .flatten(),
                )
            }),
            item.get("aggregated_output")
                .and_then(Value::as_str)
                .map(str::to_string),
        ),
        "file_change" => (
            "file_change".to_string(),
            item.get("changes")
                .map(|changes| serde_json::json!({ "changes": changes.clone() })),
            None,
        ),
        "mcp_tool_call" => (
            item.get("tool")
                .and_then(Value::as_str)
                .unwrap_or("mcp_tool_call")
                .to_string(),
            item.get("arguments").cloned(),
            item.get("error")
                .and_then(|error| error.get("message"))
                .and_then(Value::as_str)
                .map(str::to_string)
                .or_else(|| item.get("result").map(Value::to_string)),
        ),
        "collab_tool_call" => (
            item.get("tool")
                .and_then(Value::as_str)
                .unwrap_or("collab_tool_call")
                .to_string(),
            Some(Value::Object(
                item.iter()
                    .filter(|(key, _)| {
                        matches!(
                            key.as_str(),
                            "prompt" | "receiver_thread_ids" | "sender_thread_id"
                        )
                    })
                    .map(|(key, value)| (key.clone(), value.clone()))
                    .collect(),
            )),
            None,
        ),
        "web_search" => (
            "web_search".to_string(),
            item.get("query")
                .map(|query| serde_json::json!({ "query": query.clone() })),
            None,
        ),
        _ => return None,
    };
    Some(PartialEvent {
        kind: "tool_call",
        name: Some(name),
        input,
        output,
        tool_call_id: item.get("id").and_then(Value::as_str).map(str::to_string),
        status: (phase == Phase::Finished)
            .then(|| codex_tool_status(item))
            .flatten(),
        phase,
        item_id: None,
    })
}

/// A command's normalized `input`: `{"command": …}`, plus the `exit_code` the
/// harness reported for it once it finished (never a guessed one).
fn command_input(command: Value, exit_code: Option<&Value>) -> Value {
    let mut input = serde_json::json!({ "command": command });
    if let Some(code) = exit_code.filter(|code| code.is_i64()) {
        input["exit_code"] = code.clone();
    }
    input
}

/// The event kind a codex text item becomes, in either protocol's spelling.
fn codex_text_kind(item_type: &str) -> Option<&'static str> {
    match item_type {
        "agent_message" | "agentMessage" => Some("message"),
        "reasoning" => Some("reasoning"),
        _ => None,
    }
}

/// Codex app-server item notifications captured from real controlled turns
/// (`tests/fixtures/codex-app-server-*.jsonl`). The protocol spells both the
/// lifecycle method and item type differently from `exec --json`; the
/// normalized event intentionally matches that sibling reading. A message's
/// token deltas arrive as separate `item/agentMessage/delta` notifications,
/// which are not item records and never an event: the `item/completed` record
/// carries the finished text once.
fn codex_app_server_item(value: &Value) -> Option<Option<PartialEvent>> {
    let obj = value.as_object()?;
    let phase = match obj.get("method").and_then(Value::as_str) {
        Some("item/started") => Phase::Open,
        Some("item/completed") => Phase::Finished,
        _ => return None,
    };
    let item = obj
        .get("params")
        .and_then(|params| params.get("item"))
        .and_then(Value::as_object)?;
    let item_type = item.get("type").and_then(Value::as_str);
    if let Some(kind) = item_type.and_then(codex_text_kind) {
        if phase == Phase::Open {
            return Some(None);
        }
        let text = match kind {
            "message" => item.get("text").and_then(Value::as_str).map(str::to_string),
            // A reasoning item carries its summary and raw content as lists of
            // strings; the summary is what a reader is shown when both exist.
            _ => ["summary", "content"].iter().find_map(|key| {
                let joined = item
                    .get(*key)
                    .and_then(Value::as_array)?
                    .iter()
                    .filter_map(Value::as_str)
                    .collect::<Vec<_>>()
                    .join("\n");
                (!joined.trim().is_empty()).then_some(joined)
            }),
        };
        return Some(
            text.filter(|text| !text.trim().is_empty())
                .map(|text| PartialEvent::text(kind, text)),
        );
    }
    if item_type != Some("commandExecution") {
        return Some(None);
    }
    Some(Some(PartialEvent {
        kind: "tool_call",
        name: Some("command_execution".to_string()),
        input: item.get("command").and_then(Value::as_str).map(|command| {
            command_input(
                Value::String(command.to_string()),
                (phase == Phase::Finished)
                    .then(|| item.get("exitCode"))
                    .flatten(),
            )
        }),
        output: item
            .get("aggregatedOutput")
            .and_then(Value::as_str)
            .map(str::to_string),
        tool_call_id: item.get("id").and_then(Value::as_str).map(str::to_string),
        status: (phase == Phase::Finished)
            .then(|| codex_tool_status(item))
            .flatten(),
        phase,
        item_id: None,
    }))
}

fn is_codex_tool_type(item_type: &str) -> bool {
    matches!(
        item_type,
        "command_execution" | "file_change" | "mcp_tool_call" | "collab_tool_call" | "web_search"
    )
}

/// Normalize a `tool_result` block's `content` (a bare string, or an array of
/// `{type:"text","text":…}` blocks — the two Anthropic shapes) to a single
/// string; `None` when neither yields text.
fn tool_result_text(content: Option<&Value>) -> Option<String> {
    match content? {
        Value::String(s) => Some(s.clone()),
        Value::Array(items) => {
            let joined = items
                .iter()
                .filter_map(|it| it.get("text").and_then(Value::as_str))
                .collect::<Vec<_>>()
                .join("\n");
            (!joined.is_empty()).then_some(joined)
        }
        _ => None,
    }
}

/// The provenance prefix for the source string, matching `text_source`'s scheme.
fn format_prefix(fmt: OutputFormat) -> &'static str {
    match fmt {
        OutputFormat::Text => "text",
        OutputFormat::Json => "json",
        OutputFormat::StreamJson => "stream-json",
    }
}

/// Candidate JSON objects in `stdout`: the whole document when it parses (a
/// top-level array is flattened to its elements — Qwen's `json` mode emits one
/// JSON array of message objects), else each parseable line (stream-json /
/// JSONL). Document order preserved so event ordering reflects the transcript.
fn json_candidates(stdout: &str) -> Vec<Value> {
    let trimmed = stdout.trim();
    if trimmed.is_empty() {
        return Vec::new();
    }
    if let Ok(value) = serde_json::from_str::<Value>(trimmed) {
        return match value {
            Value::Array(items) => items,
            other => vec![other],
        };
    }
    stdout
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line.trim()).ok())
        .collect()
}

/// Timing derived from provider events as they crossed the stdout pipe. The
/// monotonic offsets are runner observations; UTC strings are labels captured
/// from the same boundaries. Unknown boundaries remain absent.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TimingReading {
    pub model_ms: Option<u128>,
    pub tool_ms: Option<u128>,
    pub time_to_first_token_ms: Option<u128>,
    pub trace_complete: bool,
}

/// Provider trace grammar advertised by a harness adapter. A format alone is
/// insufficient: several CLIs use `json` for a compact terminal object that has
/// no provider request boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TelemetryTrace {
    CodexJson,
    OpenCodeJson,
}

#[derive(Debug, Clone)]
struct Boundary {
    id: String,
    start: Option<(u128, String)>,
    finish: Option<(u128, String)>,
    status: Option<ToolCallStatus>,
}

/// Enrich normalized calls using paired provider identities and the runner's
/// actual output observations. Returns measured model/tool totals; overlapping
/// tool intervals are merged before summing.
pub fn apply_observed_timing(
    events: &mut [ActionEvent],
    observations: &[OutputObservation],
    run_status: Status,
    duration_ms: Option<u128>,
    trace: TelemetryTrace,
) -> TimingReading {
    apply_timing(
        events,
        observations,
        run_status,
        duration_ms,
        Some(trace),
        TimingSource::ProviderMeasured,
    )
    .0
}

/// Enrich tool calls from matching start/result records observed on stdout.
///
/// This intentionally does not infer provider/model timing. The returned union
/// is absent when no tool start was observed, preserving unknown rather than
/// manufacturing a zero.
pub fn apply_stdout_observed_tool_timing(
    events: &mut [ActionEvent],
    observations: &[OutputObservation],
    run_status: Status,
    duration_ms: Option<u128>,
) -> Option<u128> {
    apply_timing(
        events,
        observations,
        run_status,
        duration_ms,
        None,
        TimingSource::StdoutObserved,
    )
    .1
}

fn apply_timing(
    events: &mut [ActionEvent],
    observations: &[OutputObservation],
    run_status: Status,
    duration_ms: Option<u128>,
    trace: Option<TelemetryTrace>,
    timing_source: TimingSource,
) -> (TimingReading, Option<u128>) {
    let lines = observed_json_lines(observations);
    let mut boundaries: Vec<Boundary> = Vec::new();
    let mut request_start = None;
    let mut first_token = None;
    let mut model_start = None;
    let mut last_model_byte = None;
    let mut saw_model_boundary = false;
    let mut model_intervals = Vec::new();
    for (value, offset, utc) in &lines {
        if trace.is_some_and(|trace| is_provider_start(value, trace)) {
            request_start.get_or_insert(*offset);
            model_start.get_or_insert(*offset);
        }
        if is_model_token(value) {
            first_token.get_or_insert(*offset);
            last_model_byte = Some(*offset);
            saw_model_boundary = true;
        }
        if is_tool_start(value) {
            if let Some(start) = model_start.take() {
                model_intervals.push((start, *offset));
            }
            last_model_byte = None;
            saw_model_boundary = true;
        }
        if is_tool_finish(value) && request_start.is_some() {
            model_start = Some(*offset);
            last_model_byte = None;
        }
        if trace.is_some_and(|trace| is_provider_finish(value, trace)) {
            if let (Some(start), Some(finish)) = (model_start.take(), last_model_byte.take()) {
                model_intervals.push((start, finish.max(start)));
            }
        }
        observe_boundaries(value, *offset, utc, &mut boundaries);
    }
    if !matches!(run_status, Status::Ok | Status::Nonzero) {
        if let (Some(start), Some(finish)) = (model_start.take(), last_model_byte.take()) {
            model_intervals.push((start, finish.max(start)));
        }
    }
    for event in events.iter_mut().filter(|event| event.kind == "tool_call") {
        let Some(id) = event.tool_call_id.as_deref() else {
            continue;
        };
        let Some(boundary) = boundaries.iter().find(|boundary| boundary.id == id) else {
            continue;
        };
        event.started_at = boundary.start.as_ref().map(|(_, utc)| utc.clone());
        event.finished_at = boundary.finish.as_ref().map(|(_, utc)| utc.clone());
        event.duration_ms = boundary
            .start
            .as_ref()
            .zip(boundary.finish.as_ref())
            .map(|((start, _), (finish, _))| finish.saturating_sub(*start));
        event.status = boundary.status.or_else(|| {
            (boundary.start.is_some() && boundary.finish.is_none()).then(|| {
                if run_status == Status::Timeout {
                    ToolCallStatus::Timeout
                } else {
                    ToolCallStatus::Interrupted
                }
            })
        });
        if boundary.start.is_some() && timing_source == TimingSource::StdoutObserved {
            event.timing_source = Some(timing_source);
        }
    }
    let saw_tool_start = boundaries.iter().any(|boundary| boundary.start.is_some());
    let mut intervals = boundaries
        .iter()
        .filter_map(|boundary| {
            let start = boundary.start.as_ref()?.0;
            let finish = boundary
                .finish
                .as_ref()
                .map(|finish| finish.0)
                .or(duration_ms)?;
            Some((start, finish.max(start)))
        })
        .collect::<Vec<_>>();
    intervals.sort_unstable();
    let mut union = 0;
    let mut current: Option<(u128, u128)> = None;
    for (start, finish) in intervals {
        current = match current {
            Some((left, right)) if start <= right => Some((left, right.max(finish))),
            Some((left, right)) => {
                union += right.saturating_sub(left);
                Some((start, finish))
            }
            None => Some((start, finish)),
        };
    }
    if let Some((left, right)) = current {
        union += right.saturating_sub(left);
    }
    let reading = TimingReading {
        model_ms: request_start.map(|_| interval_union(&mut model_intervals)),
        tool_ms: request_start.map(|_| union),
        time_to_first_token_ms: request_start
            .zip(first_token)
            .map(|(start, token)| token.saturating_sub(start)),
        trace_complete: request_start.is_some()
            && (lines
                .iter()
                .any(|(value, _, _)| trace.is_some_and(|trace| is_provider_finish(value, trace)))
                || !matches!(run_status, Status::Ok | Status::Nonzero))
            && saw_model_boundary
            && events
                .iter()
                .filter(|event| event.kind == "tool_call")
                .all(|event| {
                    event.tool_call_id.is_some()
                        && event.started_at.is_some()
                        && event.status.is_some()
                        && (!matches!(
                            event.status,
                            Some(ToolCallStatus::Completed | ToolCallStatus::Failed)
                        ) || (event.finished_at.is_some() && event.duration_ms.is_some()))
                }),
    };
    (reading, saw_tool_start.then_some(union))
}

fn interval_union(intervals: &mut [(u128, u128)]) -> u128 {
    intervals.sort_unstable();
    let mut total = 0;
    let mut current: Option<(u128, u128)> = None;
    for &(start, finish) in intervals.iter() {
        current = match current {
            Some((left, right)) if start <= right => Some((left, right.max(finish))),
            Some((left, right)) => {
                total += right.saturating_sub(left);
                Some((start, finish))
            }
            None => Some((start, finish)),
        };
    }
    if let Some((left, right)) = current {
        total += right.saturating_sub(left);
    }
    total
}

fn observed_json_lines(observations: &[OutputObservation]) -> Vec<(Value, u128, String)> {
    let mut pending = Vec::new();
    let mut out = Vec::new();
    for observation in observations {
        pending.extend_from_slice(&observation.bytes);
        while let Some(newline) = pending.iter().position(|byte| *byte == b'\n') {
            let line: Vec<u8> = pending.drain(..=newline).collect();
            if let Ok(value) = serde_json::from_slice::<Value>(&line) {
                out.push((
                    value,
                    observation.offset_ms,
                    observation.observed_at.clone(),
                ));
            }
        }
    }
    if !pending.is_empty() {
        if let (Some(last), Ok(value)) = (
            observations.last(),
            serde_json::from_slice::<Value>(&pending),
        ) {
            out.push((value, last.offset_ms, last.observed_at.clone()));
        }
    }
    out
}

fn observe_boundaries(value: &Value, offset: u128, utc: &str, out: &mut Vec<Boundary>) {
    if let Some((method, item)) = app_server_item(value) {
        if item.get("type").and_then(Value::as_str) == Some("commandExecution") {
            if let Some(id) = item.get("id").and_then(Value::as_str) {
                let boundary = boundary(out, id);
                if method == "item/started" {
                    boundary.start = Some((offset, utc.to_string()));
                } else {
                    boundary.finish = Some((offset, utc.to_string()));
                    boundary.status = codex_tool_status(item);
                }
            }
        }
        return;
    }
    if let Some(item) = value.get("item").and_then(Value::as_object) {
        if item
            .get("type")
            .and_then(Value::as_str)
            .is_some_and(is_codex_tool_type)
        {
            let Some(id) = item.get("id").and_then(Value::as_str) else {
                return;
            };
            let boundary = boundary(out, id);
            match value.get("type").and_then(Value::as_str) {
                Some("item.started") => boundary.start = Some((offset, utc.to_string())),
                Some("item.completed") => {
                    boundary.finish = Some((offset, utc.to_string()));
                    boundary.status = codex_tool_status(item);
                }
                _ => {}
            }
        }
    }
    if value.get("type").and_then(Value::as_str) == Some("tool_call") {
        if let Some(id) = value
            .get("call_id")
            .or_else(|| value.pointer("/tool_call/toolCallId"))
            .and_then(Value::as_str)
        {
            let boundary = boundary(out, id);
            match value.get("subtype").and_then(Value::as_str) {
                Some("started") => boundary.start = Some((offset, utc.to_string())),
                Some("completed") => {
                    boundary.finish = Some((offset, utc.to_string()));
                    boundary.status = Some(if value.pointer("/tool_call/error").is_some() {
                        ToolCallStatus::Failed
                    } else {
                        ToolCallStatus::Completed
                    });
                }
                _ => {}
            }
        }
    }
    let blocks = value
        .pointer("/message/content")
        .or_else(|| value.get("content"));
    if let Some(blocks) = blocks.and_then(Value::as_array) {
        for block_value in blocks {
            let Some(block) = block_value.as_object() else {
                continue;
            };
            match block.get("type").and_then(Value::as_str) {
                Some("tool_use") => {
                    if let Some(id) = block.get("id").and_then(Value::as_str) {
                        boundary(out, id).start = Some((offset, utc.to_string()));
                    }
                }
                Some("tool_result") => {
                    if let Some(id) = block.get("tool_use_id").and_then(Value::as_str) {
                        let boundary = boundary(out, id);
                        boundary.finish = Some((offset, utc.to_string()));
                        boundary.status = Some(
                            if block.get("is_error").and_then(Value::as_bool) == Some(true) {
                                ToolCallStatus::Failed
                            } else {
                                ToolCallStatus::Completed
                            },
                        );
                    }
                }
                _ => {}
            }
        }
    }
    if let Some(part) = value.get("part").and_then(Value::as_object) {
        if part.get("type").and_then(Value::as_str) == Some("tool") {
            if let Some(id) = part
                .get("callID")
                .or_else(|| part.get("id"))
                .and_then(Value::as_str)
            {
                let state = part.get("state").and_then(Value::as_object);
                let boundary = boundary(out, id);
                let start_epoch = state
                    .and_then(|state| state.get("time"))
                    .and_then(|time| time.get("start"))
                    .and_then(Value::as_u64)
                    .map(u128::from);
                let end_epoch = state
                    .and_then(|state| state.get("time"))
                    .and_then(|time| time.get("end"))
                    .and_then(Value::as_u64)
                    .map(u128::from);
                if let Some(start_epoch) = start_epoch {
                    let start_offset = end_epoch
                        .map(|end| offset.saturating_sub(end.saturating_sub(start_epoch)))
                        .unwrap_or(offset);
                    boundary.start = Some((
                        start_offset,
                        crate::domain::history::format_rfc3339_millis(start_epoch),
                    ));
                }
                if let Some(end_epoch) = end_epoch {
                    boundary.finish = Some((
                        offset,
                        crate::domain::history::format_rfc3339_millis(end_epoch),
                    ));
                    boundary.status = Some(
                        match state
                            .and_then(|state| state.get("status"))
                            .and_then(Value::as_str)
                        {
                            Some("error" | "failed") => ToolCallStatus::Failed,
                            _ => ToolCallStatus::Completed,
                        },
                    );
                }
            }
        }
    }
}

fn boundary<'a>(boundaries: &'a mut Vec<Boundary>, id: &str) -> &'a mut Boundary {
    if let Some(index) = boundaries.iter().position(|boundary| boundary.id == id) {
        return &mut boundaries[index];
    }
    boundaries.push(Boundary {
        id: id.to_string(),
        start: None,
        finish: None,
        status: None,
    });
    boundaries.last_mut().expect("just pushed")
}

fn codex_tool_status(item: &serde_json::Map<String, Value>) -> Option<ToolCallStatus> {
    let status = item.get("status").and_then(Value::as_str);
    if matches!(
        status,
        Some("cancelled" | "canceled" | "interrupted" | "aborted")
    ) {
        return Some(ToolCallStatus::Interrupted);
    }
    if matches!(status, Some("timeout" | "timed_out")) {
        return Some(ToolCallStatus::Timeout);
    }
    if item.get("error").is_some_and(|error| !error.is_null())
        || matches!(status, Some("failed" | "error" | "declined"))
    {
        return Some(ToolCallStatus::Failed);
    }
    if matches!(status, Some("completed" | "success" | "succeeded")) {
        return Some(ToolCallStatus::Completed);
    }
    if matches!(
        item.get("type").and_then(Value::as_str),
        Some("command_execution" | "commandExecution")
    ) {
        return match item
            .get("exit_code")
            .or_else(|| item.get("exitCode"))
            .and_then(Value::as_i64)
        {
            Some(0) => Some(ToolCallStatus::Completed),
            Some(_) => Some(ToolCallStatus::Failed),
            None => None,
        };
    }
    None
}

fn is_model_token(value: &Value) -> bool {
    let assistant_content = value
        .get("type")
        .and_then(Value::as_str)
        .filter(|kind| *kind == "assistant")
        .and_then(|_| {
            value
                .pointer("/message/content")
                .or_else(|| value.get("content"))
        })
        .and_then(Value::as_array)
        .is_some_and(|blocks| {
            blocks.iter().any(|block| {
                matches!(
                    block.get("type").and_then(Value::as_str),
                    Some("text" | "reasoning" | "thinking")
                ) && block
                    .get("text")
                    .or_else(|| block.get("thinking"))
                    .and_then(Value::as_str)
                    .is_some_and(|text| !text.is_empty())
            })
        });
    assistant_content
        || value
            .pointer("/part/type")
            .and_then(Value::as_str)
            .is_some_and(|kind| matches!(kind, "text" | "reasoning"))
            && value
                .pointer("/part/text")
                .and_then(Value::as_str)
                .is_some_and(|text| !text.is_empty())
        || value.pointer("/item/type").and_then(Value::as_str) == Some("agent_message")
            && value
                .pointer("/item/text")
                .and_then(Value::as_str)
                .is_some_and(|text| !text.is_empty())
        || value.pointer("/item/type").and_then(Value::as_str) == Some("reasoning")
            && value
                .pointer("/item/text")
                .and_then(Value::as_str)
                .is_some_and(|text| !text.is_empty())
        // The app-server streams an answer as deltas before its item completes.
        || app_server_method(value) == Some("item/agentMessage/delta")
            && value
                .pointer("/params/delta")
                .and_then(Value::as_str)
                .is_some_and(|delta| !delta.is_empty())
        || app_server_item(value).is_some_and(|(method, item)| {
            method == "item/completed"
                && item.get("type").and_then(Value::as_str) == Some("agentMessage")
                && item
                    .get("text")
                    .and_then(Value::as_str)
                    .is_some_and(|text| !text.is_empty())
        })
}

/// The JSON-RPC method of a codex app-server notification — the stream a
/// controlled codex turn is driven over — or `None` for any other record.
fn app_server_method(value: &Value) -> Option<&str> {
    value.get("method").and_then(Value::as_str)
}

/// The item a codex app-server `item/started` / `item/completed` notification
/// carries, when `value` is one.
fn app_server_item(value: &Value) -> Option<(&str, &serde_json::Map<String, Value>)> {
    let method = app_server_method(value)
        .filter(|method| matches!(*method, "item/started" | "item/completed"))?;
    let item = value.pointer("/params/item").and_then(Value::as_object)?;
    Some((method, item))
}

/// Whether `value` is an app-server notification about a tool the app-server
/// event reading normalizes (`codex_app_server_item`: a `commandExecution`),
/// at `method`'s point in its life.
fn is_app_server_tool(value: &Value, method: &str) -> bool {
    app_server_item(value).is_some_and(|(seen, item)| {
        seen == method && item.get("type").and_then(Value::as_str) == Some("commandExecution")
    })
}

fn is_provider_start(value: &Value, trace: TelemetryTrace) -> bool {
    match trace {
        // `codex exec --json`'s turn, or the app-server's for a controlled one.
        TelemetryTrace::CodexJson => {
            value.get("type").and_then(Value::as_str) == Some("turn.started")
                || app_server_method(value) == Some("turn/started")
        }
        TelemetryTrace::OpenCodeJson => {
            value.get("type").and_then(Value::as_str) == Some("step_start")
                || value.pointer("/part/type").and_then(Value::as_str) == Some("step-start")
        }
    }
}

fn is_provider_finish(value: &Value, trace: TelemetryTrace) -> bool {
    match trace {
        TelemetryTrace::CodexJson => {
            value.get("type").and_then(Value::as_str) == Some("turn.completed")
                || app_server_method(value) == Some("turn/completed")
        }
        TelemetryTrace::OpenCodeJson => {
            value.get("type").and_then(Value::as_str) == Some("step_finish")
                || value.pointer("/part/type").and_then(Value::as_str) == Some("step-finish")
        }
    }
}

fn is_tool_start(value: &Value) -> bool {
    is_app_server_tool(value, "item/started")
        || value.get("type").and_then(Value::as_str) == Some("tool_call")
            && value.get("subtype").and_then(Value::as_str) == Some("started")
        || value.get("type").and_then(Value::as_str) == Some("item.started")
            && value
                .pointer("/item/type")
                .and_then(Value::as_str)
                .is_some_and(is_codex_tool_type)
        || value
            .pointer("/message/content")
            .or_else(|| value.get("content"))
            .and_then(Value::as_array)
            .is_some_and(|blocks| {
                blocks
                    .iter()
                    .any(|block| block.get("type").and_then(Value::as_str) == Some("tool_use"))
            })
        || value.pointer("/part/type").and_then(Value::as_str) == Some("tool")
            && value.pointer("/part/state/time/start").is_some()
            && value.pointer("/part/state/time/end").is_none()
}

fn is_tool_finish(value: &Value) -> bool {
    is_app_server_tool(value, "item/completed")
        || value.get("type").and_then(Value::as_str) == Some("tool_call")
            && value.get("subtype").and_then(Value::as_str) == Some("completed")
        || value.get("type").and_then(Value::as_str) == Some("item.completed")
            && value
                .pointer("/item/type")
                .and_then(Value::as_str)
                .is_some_and(is_codex_tool_type)
        || value
            .pointer("/message/content")
            .or_else(|| value.get("content"))
            .and_then(Value::as_array)
            .is_some_and(|blocks| {
                blocks
                    .iter()
                    .any(|block| block.get("type").and_then(Value::as_str) == Some("tool_result"))
            })
        || value.pointer("/part/type").and_then(Value::as_str) == Some("tool")
            && value.pointer("/part/state/time/end").is_some()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn opencode_tool_parts_become_ordered_tool_calls() {
        // A real `opencode run --format json` tool-using turn: a text part, a
        // `tool` part carrying input+output, then a step-finish. The text part
        // is a `message` event; the tool part a `tool_call` carrying the
        // normalized name/input/output.
        let raw = concat!(
            r#"{"type":"text","part":{"type":"text","text":"I'll run that."}}"#,
            "\n",
            r#"{"type":"tool_use","part":{"id":"p2","type":"tool","tool":"bash","state":{"status":"completed","input":{"command":"git commit -m x"},"output":"HELLO-FROM-TOOL"}}}"#,
            "\n",
            r#"{"type":"step_finish","part":{"type":"step-finish","cost":0.01}}"#,
            "\n",
        );
        let got = extract_events(raw, OutputFormat::Json).unwrap();
        assert_eq!(got.source, "json:opencode-parts");
        assert_eq!(got.events.len(), 2);
        let prose = &got.events[0];
        assert_eq!(prose.kind, "message");
        assert_eq!(prose.output.as_deref(), Some("I'll run that."));
        assert_eq!(prose.index, 0);
        let ev = &got.events[1];
        assert_eq!(ev.kind, "tool_call");
        assert_eq!(ev.name.as_deref(), Some("bash"));
        assert_eq!(ev.input, Some(json!({"command": "git commit -m x"})));
        assert_eq!(ev.output.as_deref(), Some("HELLO-FROM-TOOL"));
        assert_eq!(ev.index, 1);
    }

    #[test]
    fn opencode_text_and_reasoning_parts_become_one_event_per_part() {
        // The recorded `opencode run --format json` text part shape (id,
        // messageID, text, time.{start,end}); reasoning in the same shape.
        let raw = concat!(
            r#"{"type":"step_start","part":{"id":"p0","type":"step-start"}}"#,
            "\n",
            r#"{"type":"reasoning","part":{"id":"p1","messageID":"m","type":"reasoning","text":"weighing it","time":{"start":1,"end":2}}}"#,
            "\n",
            // Not finished yet: no event.
            r#"{"type":"text","part":{"id":"p2","messageID":"m","type":"text","text":"PING","time":{"start":3}}}"#,
            "\n",
            r#"{"type":"text","part":{"id":"p2","messageID":"m","type":"text","text":"PING-123","time":{"start":3,"end":4}}}"#,
            "\n",
            // The same part sent again: still one event.
            r#"{"type":"text","part":{"id":"p2","messageID":"m","type":"text","text":"PING-123","time":{"start":3,"end":4}}}"#,
            "\n",
            // A `time.end` that is no timestamp is not a finish: no event.
            r#"{"type":"text","part":{"id":"p4","messageID":"m","type":"text","text":"NULL-END","time":{"start":5,"end":null}}}"#,
            "\n",
            // Whitespace-only text: no event.
            r#"{"type":"text","part":{"id":"p3","messageID":"m","type":"text","text":"  \n","time":{"start":5,"end":6}}}"#,
            "\n",
        );
        let got = extract_events(raw, OutputFormat::Json).unwrap();
        assert_eq!(got.source, "json:opencode-parts");
        let seen: Vec<(&str, Option<&str>, Option<&str>, usize)> = got
            .events
            .iter()
            .map(|e| {
                (
                    e.kind.as_str(),
                    e.output.as_deref(),
                    e.tool_call_id.as_deref(),
                    e.index,
                )
            })
            .collect();
        assert_eq!(
            seen,
            vec![
                ("reasoning", Some("weighing it"), None, 0),
                ("message", Some("PING-123"), None, 1),
            ]
        );
        assert!(got.events.iter().all(|e| !e.is_tool_activity()));
        // Streamed line by line, each part is delivered once, as it finishes.
        let (per_line, _) = stream_lines(raw);
        let counts: Vec<usize> = per_line.iter().map(Vec::len).collect();
        assert_eq!(counts, vec![0, 1, 0, 1, 0, 0, 0]);
        // A single finished line read on its own is its event too.
        let line: Value = serde_json::from_str(
            r#"{"part":{"id":"p9","type":"reasoning","text":"hm","time":{"start":1,"end":2}}}"#,
        )
        .unwrap();
        let evs = events_from_value(&line, 3);
        assert_eq!(evs.len(), 1);
        assert_eq!((evs[0].kind.as_str(), evs[0].index), ("reasoning", 3));
    }

    #[test]
    fn opencode_multiple_tool_parts_are_indexed_in_order() {
        let raw = concat!(
            r#"{"part":{"type":"tool","tool":"read","state":{"input":{"path":"a.rs"}}}}"#,
            "\n",
            r#"{"part":{"type":"tool","tool":"bash","state":{"input":{"command":"ls"},"output":"a.rs"}}}"#,
            "\n",
        );
        let got = extract_events(raw, OutputFormat::Json).unwrap();
        assert_eq!(got.events.len(), 2);
        assert_eq!(got.events[0].name.as_deref(), Some("read"));
        assert_eq!(got.events[0].index, 0);
        assert_eq!(got.events[0].output, None); // no output field → null, not ""
        assert_eq!(got.events[1].name.as_deref(), Some("bash"));
        assert_eq!(got.events[1].index, 1);
        assert_eq!(got.events[1].output.as_deref(), Some("a.rs"));
    }

    #[test]
    fn opencode_running_and_completed_updates_collapse_by_call_id() {
        // Captured `opencode run --format json` behavior: the same callID is
        // emitted first with a running state and later as completed.
        let raw = concat!(
            r#"{"type":"tool_use","part":{"id":"part_1","callID":"call_1","type":"tool","tool":"bash","state":{"status":"running","input":{"command":"pwd"},"time":{"start":1773878400000}}}}"#,
            "\n",
            r#"{"type":"tool_use","part":{"id":"part_1","callID":"call_1","type":"tool","tool":"bash","state":{"status":"completed","input":{"command":"pwd"},"output":"/repo\n","time":{"start":1773878400000,"end":1773878400100}}}}"#,
            "\n",
        );
        let got = extract_events(raw, OutputFormat::Json).unwrap();
        assert_eq!(got.events.len(), 1);
        assert_eq!(got.events[0].tool_call_id.as_deref(), Some("call_1"));
        assert_eq!(got.events[0].input, Some(json!({"command": "pwd"})));
        assert_eq!(got.events[0].output.as_deref(), Some("/repo\n"));
    }

    #[test]
    fn a_claude_tool_result_carries_its_error_state_and_its_calls_name() {
        // Claude Code `stream-json`: a failed call's `tool_result` block carries
        // `is_error: true`; a completed one carries `false` or omits it.
        let raw = concat!(
            r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"tool_use","id":"toolu_bad","name":"Bash","input":{"command":"cargo test"}}]}}"#,
            "\n",
            r#"{"type":"user","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"toolu_bad","is_error":true,"content":"error: could not compile"}]}}"#,
            "\n",
            r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"tool_use","id":"toolu_ok","name":"Read","input":{"file_path":"a.rs"}},{"type":"tool_use","id":"toolu_quiet","name":"Grep","input":{"pattern":"x"}}]}}"#,
            "\n",
            r#"{"type":"user","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"toolu_ok","is_error":false,"content":"fn main() {}"},{"type":"tool_result","tool_use_id":"toolu_quiet","content":"a.rs"},{"type":"tool_result","tool_use_id":"toolu_unseen","is_error":true,"content":"boom"}]}}"#,
            "\n",
        );
        let results: Vec<(Option<String>, Option<ToolCallStatus>)> = stream_lines(raw)
            .0
            .into_iter()
            .flatten()
            .filter(|event| event.kind == "tool_result")
            .map(|event| (event.name, event.status))
            .collect();
        assert_eq!(
            results,
            vec![
                (Some("Bash".to_string()), Some(ToolCallStatus::Failed)),
                (Some("Read".to_string()), Some(ToolCallStatus::Completed)),
                (Some("Grep".to_string()), Some(ToolCallStatus::Completed)),
                // A result whose call this run never delivered stays unnamed.
                (None, Some(ToolCallStatus::Failed)),
            ]
        );
        // The buffered report folds the same lines through the same stream.
        let report = extract_events(raw, OutputFormat::StreamJson).unwrap();
        assert_eq!(report.events[1].status, Some(ToolCallStatus::Failed));
        assert_eq!(report.events[1].name.as_deref(), Some("Bash"));
    }

    #[test]
    fn anthropic_content_blocks_yield_call_and_result_events() {
        // Claude Code / Cursor stream-json: an assistant message with a `tool_use`
        // block, then a user message with the `tool_result` observation.
        let raw = concat!(
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"ok"},{"type":"tool_use","id":"toolu_1","name":"Bash","input":{"command":"echo hi"}}]}}"#,
            "\n",
            r#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"toolu_1","content":"hi\n"}]}}"#,
            "\n",
            r#"{"type":"result","result":"done"}"#,
            "\n",
        );
        let got = extract_events(raw, OutputFormat::StreamJson).unwrap();
        assert_eq!(got.source, "stream-json:content-blocks");
        assert_eq!(got.events.len(), 3);
        // The assistant's own text block precedes its tool call, in block order.
        assert_eq!(got.events[0].kind, "message");
        assert_eq!(got.events[0].output.as_deref(), Some("ok"));
        assert_eq!(got.events[1].kind, "tool_call");
        assert_eq!(got.events[1].name.as_deref(), Some("Bash"));
        assert_eq!(got.events[1].input, Some(json!({"command": "echo hi"})));
        assert_eq!(got.events[1].output, None);
        assert_eq!(got.events[2].kind, "tool_result");
        assert_eq!(got.events[2].name.as_deref(), Some("Bash"));
        assert_eq!(got.events[2].status, Some(ToolCallStatus::Completed));
        assert_eq!(got.events[2].input, None);
        assert_eq!(got.events[2].output.as_deref(), Some("hi\n"));
        assert_eq!(got.events[2].index, 2);
    }

    #[test]
    fn tool_result_content_array_is_joined() {
        // The other Anthropic `tool_result` shape: `content` is an array of text
        // blocks rather than a bare string; they are joined into one observation.
        let raw = r#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"t","content":[{"type":"text","text":"line one"},{"type":"text","text":"line two"}]}]}}"#;
        let got = extract_events(raw, OutputFormat::StreamJson).unwrap();
        assert_eq!(got.events.len(), 1);
        assert_eq!(got.events[0].output.as_deref(), Some("line one\nline two"));
    }

    #[test]
    fn top_level_content_array_without_message_wrapper() {
        // Some emitters put the content array at the top level rather than under
        // `message`; both are accepted.
        let raw = r#"{"content":[{"type":"tool_use","name":"read","input":{"path":"x"}}]}"#;
        let got = extract_events(raw, OutputFormat::StreamJson).unwrap();
        assert_eq!(got.events.len(), 1);
        assert_eq!(got.events[0].name.as_deref(), Some("read"));
    }

    #[test]
    fn cursor_tool_call_completed_event() {
        // Real cursor-agent stream-json: a `tool_call` `started` then `completed`.
        // The tool name is the `shellToolCall` key minus its `ToolCall` suffix;
        // input is `.args`, output is `.result.success.stdout`. Only the completed
        // event yields a normalized event (started is skipped to avoid a dupe).
        let raw = concat!(
            r#"{"type":"tool_call","subtype":"started","call_id":"c1","tool_call":{"shellToolCall":{"args":{"command":"echo hi"}},"toolCallId":"c1","startedAtMs":"1"}}"#,
            "\n",
            r#"{"type":"tool_call","subtype":"completed","call_id":"c1","tool_call":{"shellToolCall":{"args":{"command":"echo hi"},"result":{"success":{"command":"echo hi","exitCode":0,"stdout":"hi\n","stderr":""}}},"toolCallId":"c1","completedAtMs":"2"}}"#,
            "\n",
            r#"{"type":"result","subtype":"success","result":"done"}"#,
            "\n",
        );
        let got = extract_events(raw, OutputFormat::StreamJson).unwrap();
        assert_eq!(got.source, "stream-json:cursor-tool-calls");
        assert_eq!(got.events.len(), 1);
        assert_eq!(got.events[0].kind, "tool_call");
        assert_eq!(got.events[0].name.as_deref(), Some("shell"));
        assert_eq!(got.events[0].input, Some(json!({"command": "echo hi"})));
        assert_eq!(got.events[0].output.as_deref(), Some("hi\n"));
    }

    #[test]
    fn cursor_turn_that_speaks_before_its_tool_call_is_labelled_cursor() {
        // Live cursor-agent 2026-10: an assistant text record (content-block
        // shape) precedes the `tool_call` pair; the transcript is still Cursor's.
        let raw = concat!(
            r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"I'll print that now."}]}}"#,
            "\n",
            r#"{"type":"tool_call","subtype":"completed","call_id":"c1","tool_call":{"shellToolCall":{"args":{"command":"echo hi"},"result":{"success":{"command":"echo hi","exitCode":0,"stdout":"hi\n","stderr":""}}},"toolCallId":"c1"}}"#,
            "\n",
            r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"Done."}]}}"#,
            "\n",
        );
        let got = extract_events(raw, OutputFormat::StreamJson).unwrap();
        assert_eq!(got.source, "stream-json:cursor-tool-calls");
        let kinds: Vec<_> = got.events.iter().map(|e| e.kind.as_str()).collect();
        assert_eq!(kinds, ["message", "tool_call", "message"]);
    }

    #[test]
    fn codex_command_execution_item_event() {
        // Real codex `exec --json`: an `item.started` then `item.completed` for a
        // `command_execution`. The completed item folds in the result; only it
        // yields an event. Name is the item type (codex has no tool-name field),
        // input is the run command, output is the aggregated output. A trailing
        // `agent_message` item is the final text, not a tool call.
        let raw = concat!(
            r#"{"type":"thread.started","thread_id":"th_1"}"#,
            "\n",
            r#"{"type":"item.started","item":{"id":"item_0","type":"command_execution","command":"/bin/bash -lc 'echo hi'","aggregated_output":"","exit_code":null,"status":"in_progress"}}"#,
            "\n",
            r#"{"type":"item.completed","item":{"id":"item_0","type":"command_execution","command":"/bin/bash -lc 'echo hi'","aggregated_output":"hi\n","exit_code":0,"status":"completed"}}"#,
            "\n",
            r#"{"type":"item.completed","item":{"id":"item_1","type":"agent_message","text":"I ran it."}}"#,
            "\n",
        );
        let got = extract_events(raw, OutputFormat::Json).unwrap();
        assert_eq!(got.source, "json:codex-items");
        assert_eq!(got.events.len(), 2);
        assert_eq!(got.events[0].kind, "tool_call");
        assert_eq!(got.events[0].name.as_deref(), Some("command_execution"));
        assert_eq!(
            got.events[0].input,
            Some(json!({"command": "/bin/bash -lc 'echo hi'", "exit_code": 0}))
        );
        assert_eq!(got.events[0].output.as_deref(), Some("hi\n"));
        assert_eq!(got.events[0].status, Some(ToolCallStatus::Completed));
        assert_eq!(got.events[1].kind, "message");
        assert_eq!(got.events[1].output.as_deref(), Some("I ran it."));
    }

    /// Every line of `raw` pushed through one [`EventStream`], as a streaming
    /// run pushes them: each line's delivered events, in order, and the
    /// never-finished remainder the report appends.
    fn stream_lines(raw: &str) -> (Vec<Vec<ActionEvent>>, Vec<ActionEvent>) {
        let mut stream = EventStream::default();
        let per_line = raw
            .lines()
            .map(|line| stream.push(&serde_json::from_str(line).expect("recorded line is JSON")))
            .collect();
        (per_line, stream.finish())
    }

    fn kinds(events: &[ActionEvent]) -> Vec<&str> {
        events.iter().map(|event| event.kind.as_str()).collect()
    }

    /// The real `codex exec --json` recording (codex-cli 0.157.1), with an
    /// `item.updated` record for its first command spliced in after that
    /// command's `item.started` — built from the recording's own started record
    /// plus its first output — because that CLI emits `item.updated` for plan
    /// items but not for a command, and the property under test is what a
    /// call's full started/updated/completed lifecycle delivers.
    fn codex_exec_turn_with_update() -> String {
        let raw = include_str!("../../../../tests/fixtures/codex-exec-turn.jsonl");
        let mut lines: Vec<String> = raw.lines().map(str::to_string).collect();
        let started = lines
            .iter()
            .position(|line| line.contains(r#""type":"item.started""#))
            .expect("the recording starts a command");
        let mut update: Value = serde_json::from_str(&lines[started]).unwrap();
        update["type"] = json!("item.updated");
        update["item"]["aggregated_output"] = json!("hel");
        lines.insert(started + 1, update.to_string());
        lines.join("\n")
    }

    #[test]
    fn recorded_codex_turn_delivers_each_call_once_at_completion() {
        let raw = codex_exec_turn_with_update();
        let (per_line, unfinished) = stream_lines(&raw);
        // The started and updated records of a call deliver nothing; only the
        // completing record does.
        for (line, delivered) in raw.lines().zip(&per_line) {
            if line.contains(r#""type":"item.started""#)
                || line.contains(r#""type":"item.updated""#)
            {
                assert!(delivered.is_empty(), "{line} delivered {delivered:?}");
            }
        }
        assert!(unfinished.is_empty());
        let live: Vec<ActionEvent> = per_line.into_iter().flatten().collect();
        assert_eq!(
            kinds(&live),
            [
                "message",
                "reasoning",
                "tool_call",
                "tool_call",
                "tool_call",
                "message"
            ]
        );
        // One event per call, never repeated under the same id.
        let ids: Vec<_> = live
            .iter()
            .filter(|event| event.kind == "tool_call")
            .map(|event| event.tool_call_id.clone().unwrap())
            .collect();
        assert_eq!(ids, ["item_2", "item_3", "item_4"]);
        // The live fold is the report's reading, index for index.
        let report = extract_events(&raw, OutputFormat::Json).unwrap();
        assert_eq!(report.events, live);
        assert_eq!(
            live.iter().map(|event| event.index).collect::<Vec<_>>(),
            [0, 1, 2, 3, 4, 5]
        );
        // The completed command carries its terminal status and exit code.
        let failed = &live[4];
        assert_eq!(failed.status, Some(ToolCallStatus::Failed));
        assert_eq!(
            failed.input,
            Some(json!({"command": "/usr/bin/bash -lc 'ls does-not-exist'", "exit_code": 2}))
        );
        assert_eq!(live[2].status, Some(ToolCallStatus::Completed));
        assert_eq!(live[2].output.as_deref(), Some("hello\n"));
        assert_eq!(
            live[1].output.as_deref(),
            Some("**Proceeding with 391–399**")
        );
        assert!(live[0]
            .output
            .as_deref()
            .unwrap()
            .starts_with("I’ll compare"));
    }

    #[test]
    fn a_call_cut_short_reaches_the_report_but_never_the_live_stream() {
        let raw = concat!(
            r#"{"type":"item.started","item":{"id":"item_1","type":"command_execution","command":"sleep 99","aggregated_output":"","exit_code":null,"status":"in_progress"}}"#,
            "\n",
            r#"{"type":"item.completed","item":{"id":"item_2","type":"agent_message","text":"waiting"}}"#,
            "\n",
        );
        let (per_line, unfinished) = stream_lines(raw);
        assert_eq!(kinds(&per_line.concat()), ["message"]);
        assert_eq!(kinds(&unfinished), ["tool_call"]);
        assert_eq!(unfinished[0].index, 1);
        assert_eq!(unfinished[0].input, Some(json!({"command": "sleep 99"})));
        let report = extract_events(raw, OutputFormat::Json).unwrap();
        assert_eq!(kinds(&report.events), ["message", "tool_call"]);
        // A stateless reading of the started record alone delivers nothing.
        let started: Value = serde_json::from_str(raw.lines().next().unwrap()).unwrap();
        assert!(events_from_value(&started, 0).is_empty());
    }

    #[test]
    fn recorded_codex_app_server_deltas_become_one_message_per_item() {
        let raw = include_str!("../../../../tests/fixtures/codex-app-server-turn.jsonl");
        let deltas = raw
            .lines()
            .filter(|line| line.contains(r#""method":"item/agentMessage/delta""#))
            .count();
        assert!(deltas > 10, "the recording streams its message as deltas");
        let (per_line, unfinished) = stream_lines(raw);
        for (line, delivered) in raw.lines().zip(&per_line) {
            if !line.contains(r#""method":"item/completed""#) {
                assert!(delivered.is_empty(), "{line} delivered {delivered:?}");
            }
        }
        assert!(unfinished.is_empty());
        let live: Vec<ActionEvent> = per_line.into_iter().flatten().collect();
        assert_eq!(
            kinds(&live),
            ["message", "tool_call", "tool_call", "message"]
        );
        assert_eq!(
            live[0].output.as_deref(),
            Some("I’ll compare the products, then run both commands.")
        );
        assert!(live[3]
            .output
            .as_deref()
            .unwrap()
            .starts_with("19 × 21 = 399"));
        assert_eq!(live[2].status, Some(ToolCallStatus::Failed));
        assert_eq!(live[2].input.as_ref().unwrap()["exit_code"], json!(2));
        assert_eq!(
            extract_events(raw, OutputFormat::Json).unwrap().events,
            live
        );
    }

    #[test]
    fn recorded_claude_deltas_become_one_event_per_finished_block() {
        let raw = include_str!("../../../../tests/fixtures/claude-stream-json-turn.jsonl");
        for delta in ["thinking_delta", "text_delta"] {
            assert!(
                raw.matches(&format!(r#""type":"{delta}""#)).count() > 5,
                "the recording streams {delta}s"
            );
        }
        let (per_line, unfinished) = stream_lines(raw);
        for (line, delivered) in raw.lines().zip(&per_line) {
            if line.starts_with(r#"{"type":"stream_event""#) {
                assert!(delivered.is_empty(), "{line} delivered {delivered:?}");
            }
        }
        assert!(unfinished.is_empty());
        let live: Vec<ActionEvent> = per_line.into_iter().flatten().collect();
        assert_eq!(
            kinds(&live),
            [
                "reasoning",
                "message",
                "tool_call",
                "tool_result",
                "tool_call",
                "tool_result",
                "reasoning",
                "message"
            ]
        );
        assert!(live[0]
            .output
            .as_deref()
            .unwrap()
            .starts_with("The user is asking me to:"));
        assert_eq!(
            live[2].input.as_ref().unwrap()["command"],
            json!("cat note.txt")
        );
        assert_eq!(
            extract_events(raw, OutputFormat::StreamJson)
                .unwrap()
                .events,
            live
        );
    }

    #[test]
    fn codex_app_server_capture_yields_the_exec_compatible_event_shape() {
        let raw =
            include_str!("../../../../tests/fixtures/codex-app-server-command-execution.jsonl");
        let got = extract_events(raw, OutputFormat::Json).unwrap();
        assert_eq!(got.source, "json:codex-app-server-items");
        assert_eq!(got.events.len(), 1);
        assert_eq!(got.events[0].name.as_deref(), Some("command_execution"));
        assert_eq!(
            got.events[0].input,
            Some(json!({
                "command": "/usr/bin/bash -lc 'printf OHCAPTURE12345'",
                "exit_code": 0
            }))
        );
        assert_eq!(got.events[0].output.as_deref(), Some("OHCAPTURE12345"));
        assert_eq!(got.events[0].status, Some(ToolCallStatus::Completed));
    }

    /// A captured controlled turn, replayed at the instants the server stamped
    /// on each frame (`emittedAtMs`), as the runner would have observed it.
    fn app_server_observations(raw: &str) -> Vec<OutputObservation> {
        let first = raw
            .lines()
            .find_map(|line| serde_json::from_str::<Value>(line).ok()?["emittedAtMs"].as_u64())
            .unwrap();
        raw.lines()
            .map(|line| {
                let at = serde_json::from_str::<Value>(line).unwrap()["emittedAtMs"]
                    .as_u64()
                    .unwrap_or(first);
                OutputObservation {
                    offset_ms: u128::from(at - first),
                    observed_at: format!("2026-10-20T00:00:{:06.3}Z", (at - first) as f64 / 1000.0),
                    bytes: format!("{line}\n").into_bytes(),
                }
            })
            .collect()
    }

    /// A controlled codex turn's app-server stream is the same provider trace
    /// an exec turn's is: `turn/started` … `turn/completed` bound it, streamed
    /// answer text is model output, and each `commandExecution` item times its
    /// tool call — so the run measures a complete trace.
    #[test]
    fn a_codex_app_server_turn_is_a_complete_provider_trace() {
        let raw = include_str!("../../../../tests/fixtures/codex-app-server-turn.jsonl");
        let mut events = extract_events(raw, OutputFormat::Json).unwrap().events;
        let observations = app_server_observations(raw);
        let duration = observations.last().unwrap().offset_ms;
        let reading = apply_observed_timing(
            &mut events,
            &observations,
            Status::Ok,
            Some(duration),
            TelemetryTrace::CodexJson,
        );
        assert!(reading.trace_complete, "{reading:?}");
        let tools: Vec<_> = events.iter().filter(|e| e.kind == "tool_call").collect();
        assert_eq!(tools.len(), 2);
        for tool in &tools {
            assert!(
                tool.started_at.is_some() && tool.duration_ms.is_some(),
                "{tool:?}"
            );
        }
        assert_eq!(tools[1].status, Some(ToolCallStatus::Failed));
        let tool_ms = reading.tool_ms.unwrap();
        let model_ms = reading.model_ms.unwrap();
        assert!(
            model_ms > 0 && model_ms + tool_ms <= duration,
            "{reading:?}"
        );
        assert!(reading.time_to_first_token_ms.unwrap() > 0);

        // Without its closing `turn/completed`, a clean exit's trace is not
        // complete — the turn's end was never observed.
        let cut: String = raw
            .lines()
            .filter(|line| !line.contains(r#""method":"turn/completed""#))
            .map(|line| format!("{line}\n"))
            .collect();
        let mut events = extract_events(&cut, OutputFormat::Json).unwrap().events;
        let reading = apply_observed_timing(
            &mut events,
            &app_server_observations(&cut),
            Status::Ok,
            Some(duration),
            TelemetryTrace::CodexJson,
        );
        assert!(!reading.trace_complete, "{reading:?}");
    }

    #[test]
    fn codex_app_server_tool_free_item_is_an_empty_reading() {
        let raw = r#"{"method":"item/completed","params":{"item":{"type":"userMessage","id":"user-1","content":[]}}}"#;
        let got = extract_events(raw, OutputFormat::Json).unwrap();
        assert_eq!(got.source, "json:codex-app-server-items");
        assert!(got.events.is_empty());
    }

    #[test]
    fn codex_app_server_agent_message_is_one_message_event() {
        let raw = r#"{"method":"item/completed","params":{"item":{"type":"agentMessage","id":"message-1","text":"done"}}}"#;
        let got = extract_events(raw, OutputFormat::Json).unwrap();
        assert_eq!(got.events.len(), 1);
        assert_eq!(got.events[0].kind, "message");
        assert_eq!(got.events[0].output.as_deref(), Some("done"));
        assert_eq!(got.events[0].tool_call_id, None);
    }

    #[test]
    fn codex_app_server_rejects_a_non_string_command_argument() {
        let raw = r#"{"method":"item/completed","params":{"item":{"type":"commandExecution","id":"command-1","command":{"unexpected":"shape"},"aggregatedOutput":"done"}}}"#;
        let got = extract_events(raw, OutputFormat::Json).unwrap();
        assert_eq!(got.source, "json:codex-app-server-items");
        assert_eq!(got.events.len(), 1);
        assert_eq!(got.events[0].input, None);
        assert_eq!(got.events[0].output.as_deref(), Some("done"));
    }

    #[test]
    fn codex_file_change_item_is_a_measured_tool_call() {
        // codex-cli 0.145.0 emits `file_change` items with a started/completed
        // lifecycle sharing one id — a real execution boundary — so they must
        // normalize to a `file_change` tool call (input carries the `changes`),
        // not be silently dropped as the older completion-only shape was.
        let raw = concat!(
            r#"{"type":"item.started","item":{"id":"item_2","type":"file_change","changes":[{"path":"note.txt","kind":"add"}],"status":"in_progress"}}"#,
            "\n",
            r#"{"type":"item.completed","item":{"id":"item_2","type":"file_change","changes":[{"path":"note.txt","kind":"add"}],"status":"completed"}}"#,
            "\n",
        );
        let got = extract_events(raw, OutputFormat::Json).unwrap();
        assert_eq!(got.events.len(), 1);
        assert_eq!(got.events[0].kind, "tool_call");
        assert_eq!(got.events[0].name.as_deref(), Some("file_change"));
        assert_eq!(got.events[0].tool_call_id.as_deref(), Some("item_2"));
        assert_eq!(
            got.events[0].input,
            Some(json!({"changes": [{"path": "note.txt", "kind": "add"}]}))
        );
    }

    #[test]
    fn codex_mcp_item_updates_collapse_and_preserve_failure() {
        // `codex exec --json`'s official exec item shape uses the same id for
        // the started and completed MCP lifecycle records.
        let raw = concat!(
            r#"{"type":"item.started","item":{"id":"item_mcp","type":"mcp_tool_call","server":"minimal","tool":"count","arguments":{"n":2},"status":"in_progress"}}"#,
            "\n",
            r#"{"type":"item.completed","item":{"id":"item_mcp","type":"mcp_tool_call","server":"minimal","tool":"count","arguments":{"n":2},"result":null,"error":{"message":"user cancelled MCP tool call"},"status":"failed"}}"#,
            "\n",
        );
        let got = extract_events(raw, OutputFormat::Json).unwrap();
        assert_eq!(got.events.len(), 1);
        assert_eq!(got.events[0].tool_call_id.as_deref(), Some("item_mcp"));
        assert_eq!(got.events[0].name.as_deref(), Some("count"));
        assert_eq!(got.events[0].input, Some(json!({"n": 2})));
        assert_eq!(
            got.events[0].output.as_deref(),
            Some("user cancelled MCP tool call")
        );
    }

    #[test]
    fn qwen_content_blocks_stream_and_json_array() {
        // Qwen uses the Anthropic content-block shape. Under stream-json it is
        // NDLJSON (one message per line); under json it is a single JSON *array*
        // of the same message objects — json_candidates flattens the array so both
        // yield the same normalized events.
        let call = r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"call_1","name":"run_shell_command","input":{"command":"echo hi"}}]}}"#;
        let result = r#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"call_1","is_error":false,"content":"hi"}]}}"#;
        let ndjson = format!("{call}\n{result}\n");
        let got = extract_events(&ndjson, OutputFormat::StreamJson).unwrap();
        assert_eq!(got.source, "stream-json:content-blocks");
        assert_eq!(got.events.len(), 2);
        assert_eq!(got.events[0].name.as_deref(), Some("run_shell_command"));
        assert_eq!(got.events[1].output.as_deref(), Some("hi"));

        let array = format!("[{call},{result}]");
        let got = extract_events(&array, OutputFormat::Json).unwrap();
        assert_eq!(got.source, "json:content-blocks");
        assert_eq!(got.events.len(), 2);
        assert_eq!(got.events[0].name.as_deref(), Some("run_shell_command"));
    }

    #[test]
    fn events_from_value_numbers_from_start_index() {
        // The streaming entry point: per-line extraction that numbers events from
        // a running offset, so the incremental stream matches the batch indices.
        let line: Value = serde_json::from_str(
            r#"{"part":{"type":"tool","tool":"bash","state":{"input":{"command":"ls"}}}}"#,
        )
        .unwrap();
        let evs = events_from_value(&line, 5);
        assert_eq!(evs.len(), 1);
        assert_eq!(evs[0].index, 5);
        assert_eq!(evs[0].name.as_deref(), Some("bash"));
        // A non-event line yields nothing.
        let noise: Value = serde_json::from_str(r#"{"type":"step_start"}"#).unwrap();
        assert!(events_from_value(&noise, 0).is_empty());
    }

    #[test]
    fn no_tool_events_yields_none() {
        // Claude Code's single-document `json` result carries no transcript, so
        // there is nothing to extract — events stays absent, never fabricated.
        assert!(extract_events(r#"{"type":"result","result":"hi"}"#, OutputFormat::Json).is_none());
        // Prose is a `message` event, not an absent reading: an opencode text
        // part and a Claude assistant text block alike.
        let opencode_prose = r#"{"type":"text","part":{"type":"text","text":"just prose"}}"#;
        let got = extract_events(opencode_prose, OutputFormat::Json).unwrap();
        assert_eq!(got.events.len(), 1);
        assert_eq!(got.events[0].kind, "message");
        assert_eq!(got.events[0].output.as_deref(), Some("just prose"));
        assert!(!got.events[0].is_tool_activity());
        let claude_prose =
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"hi"}]}}"#;
        let got = extract_events(claude_prose, OutputFormat::Json).unwrap();
        assert_eq!(got.events.len(), 1);
        assert_eq!(got.events[0].kind, "message");
        assert!(!got.events[0].is_tool_activity());
        assert!(extract_events("not json", OutputFormat::Text).is_none());
        assert!(extract_events("", OutputFormat::Json).is_none());
    }

    #[test]
    fn skips_noise_lines_and_still_recovers_events() {
        // Blank, non-JSON, and irrelevant lines are ignored without crashing or
        // mis-numbering the real events that follow.
        let raw = concat!(
            "\n",
            "   \n",
            "not json at all\n",
            r#"{"type":"step_start","part":{"type":"step-start"}}"#,
            "\n",
            r#"{"part":{"type":"tool","tool":"bash","state":{"input":{"command":"ls"}}}}"#,
            "\n",
        );
        let got = extract_events(raw, OutputFormat::Json).unwrap();
        assert_eq!(got.events.len(), 1);
        assert_eq!(got.events[0].index, 0);
        assert_eq!(got.events[0].name.as_deref(), Some("bash"));
    }

    #[test]
    fn tool_part_without_state_still_emits_call_with_null_fields() {
        // A tool part missing its `state` (e.g. captured mid-call) is still a
        // real call: the name surfaces; input/output stay null rather than faked.
        let raw = r#"{"part":{"type":"tool","tool":"webfetch"}}"#;
        let got = extract_events(raw, OutputFormat::Json).unwrap();
        assert_eq!(got.events[0].name.as_deref(), Some("webfetch"));
        assert_eq!(got.events[0].input, None);
        assert_eq!(got.events[0].output, None);
    }

    #[test]
    fn tool_result_with_non_text_content_yields_null_output() {
        // A `tool_result` whose content is neither a string nor text blocks (an
        // object, or an array of non-text items) has no recoverable observation.
        let raw = r#"{"message":{"content":[{"type":"tool_result","content":{"weird":true}}]}}"#;
        let got = extract_events(raw, OutputFormat::StreamJson).unwrap();
        assert_eq!(got.events[0].kind, "tool_result");
        assert_eq!(got.events[0].output, None);
    }

    #[test]
    fn source_prefix_tracks_output_format() {
        // The provenance prefix mirrors `text_source`: same shape, different
        // format label. OpenCode parts arrive under Json; content blocks under
        // stream-json — the prefix reflects whichever format the run used.
        let oc = r#"{"part":{"type":"tool","tool":"bash","state":{"input":{}}}}"#;
        assert_eq!(
            extract_events(oc, OutputFormat::Json).unwrap().source,
            "json:opencode-parts"
        );
        let cb = r#"{"message":{"content":[{"type":"tool_use","name":"x","input":{}}]}}"#;
        assert_eq!(
            extract_events(cb, OutputFormat::Text).unwrap().source,
            "text:content-blocks"
        );
    }

    #[test]
    fn anthropic_tool_use_is_not_a_first_content_token() {
        // Captured Claude/Qwen/Cursor stream-json content blocks use the same
        // assistant envelope. A tool-only message is model activity, but the
        // telemetry contract's TTFT boundary is the first non-empty user-visible
        // text/reasoning block.
        assert!(!is_model_token(&json!({
            "type": "assistant",
            "message": {"content": [{
                "type": "tool_use", "id": "toolu_1", "name": "Bash", "input": {}
            }]}
        })));
        assert!(!is_model_token(&json!({
            "type": "assistant",
            "message": {"content": [{"type": "text", "text": ""}]}
        })));
        assert!(is_model_token(&json!({
            "type": "assistant",
            "message": {"content": [{"type": "thinking", "thinking": "inspect"}]}
        })));
        assert!(is_model_token(&json!({
            "type": "assistant",
            "message": {"content": [{"type": "text", "text": "done"}]}
        })));
    }
}
