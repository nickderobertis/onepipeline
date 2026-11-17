//! Best-effort extraction of normalized signals — token/cost usage, the session
//! id, and a coarse failure reason — from a harness's raw output. Pure: no I/O.
//!
//! Like `text`, every signal here is a convenience: it is `null`/empty when it
//! cannot be found, is **never fabricated**, and (where there is more than one
//! possible method) records how it was found, so a consumer can tell a real
//! reading from a guess. The execution envelope stays the guaranteed contract;
//! these only enrich it. An absent signal is the honest answer, not an error.
//!
//! Coverage is keyed off each harness's real output shape, sourced from its docs,
//! not guessed:
//! - Claude Code (`--output-format json`): a terminal `result` event with a
//!   top-level `usage` block (including `cache_read_input_tokens` /
//!   `cache_creation_input_tokens` for prompt-cache reads/writes), `total_cost_usd`,
//!   and `session_id`.
//! - OpenCode (`run --format json`): JSONL `step_finish` events that report
//!   *per-step* tokens/cost under `part` (cache reads/writes under
//!   `part.tokens.cache.{read,write}`), and a camelCase `sessionID`. The run
//!   total is the sum across steps (taking the last would undercount).
//! - Cursor (`--output-format stream-json`): NDJSON whose events carry a
//!   snake_case `session_id`; it does not emit token usage today, so usage stays
//!   absent rather than fabricated.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::domain::fallback::RunWork;

/// Normalized token/cost accounting. Every field is best-effort and independently
/// nullable: a harness may report tokens but not dollar cost (cost is commonly
/// absent on subscription auth), or report nothing at all (plain-text harnesses).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Usage {
    /// Prompt/input tokens billed, when the harness reports them.
    pub input_tokens: Option<u64>,
    /// Completion/output tokens billed, when the harness reports them.
    pub output_tokens: Option<u64>,
    /// Prompt tokens served from the provider's prompt cache (a cheap read of a
    /// previously-written prefix), when the harness reports them. `None` when the
    /// harness does not surface cache counts — never `0` as a guess.
    pub cache_read_tokens: Option<u64>,
    /// Prompt tokens written to the provider's prompt cache (a.k.a. cache
    /// creation), when the harness reports them. `None` when not surfaced.
    pub cache_write_tokens: Option<u64>,
    /// Total cost in USD, when the harness reports it (often absent on
    /// subscription auth, where there is no per-call dollar figure).
    pub cost_usd: Option<f64>,
}

impl Usage {
    fn is_empty(&self) -> bool {
        self.input_tokens.is_none()
            && self.output_tokens.is_none()
            && self.cache_read_tokens.is_none()
            && self.cache_write_tokens.is_none()
            && self.cost_usd.is_none()
    }

    /// Whether this accounting says the provider **billed real work**: any
    /// non-zero token count (prompt-cache counts included) or a non-zero dollar
    /// cost. Absent accounting is deliberately not work — see the private
    /// `record_work_evidence` reader, which reads this off a raw harness
    /// record, and [`RunWork::from_result`], which reads it off a normalized
    /// result. Both share this one definition — and answer in the one
    /// [`RunWork`] type — so the quota classifier and a fallback chain's
    /// stop/fall-through verdict can never disagree about what counts as
    /// billed.
    pub fn reports_billed_work(&self) -> bool {
        [
            self.input_tokens,
            self.output_tokens,
            self.cache_read_tokens,
            self.cache_write_tokens,
        ]
        .into_iter()
        .flatten()
        .any(|tokens| tokens > 0)
            || self.cost_usd.is_some_and(|cost| cost > 0.0)
    }
}

/// A usage reading plus the method that produced it (e.g. `json`).
#[derive(Debug, Clone, PartialEq)]
pub struct UsageReading {
    pub usage: Usage,
    pub source: String,
}

/// The normalized, closed set of failure reasons oneharness can classify from a
/// harness's output. It is the single source for the `failure_kind` contract
/// value: serialized as the snake_case token a consumer reads in the report
/// (`auth`, `rate_limit`, `model_not_found`, `quota`, `session_not_found`,
/// `tool_deferred`, `server_overloaded`, `app_server_lost`), so the wire shape is unchanged — modeling it as an enum
/// keeps a misspelled or invalid kind unrepresentable and gives every
/// producer/consumer (classifier, `is_failure`, the fallback fall-through rule,
/// the report, history) one definition to share instead of scattered string
/// literals.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum FailureKind {
    /// Authentication / authorization rejected the request (401/403, missing or
    /// invalid credentials).
    Auth,
    /// Rate limited (429, too many requests) — a transient condition of an
    /// otherwise working, authenticated harness. A `429` that shows no sign of
    /// having done work is `quota` instead, whatever wording it carries.
    RateLimit,
    /// The requested model was not found / is invalid — a configuration mistake.
    ModelNotFound,
    /// Out of quota / credits, or a billing problem — a provisioning failure.
    Quota,
    /// The session this run asked to continue does not exist for the identity
    /// it ran as, so the harness refused before doing any work (see the private
    /// `unknown_session_rejection` rule). Distinct from every other kind
    /// because the task itself is fine: another identity — or a fresh session
    /// on this one — can still run it.
    SessionNotFound,
    /// The harness deferred a builtin tool call instead of executing it, so a
    /// clean-exit run did no useful work (Claude Code bridge deployments; issue
    /// #1114). The only kind that can appear on a `status: ok` run.
    ToolDeferred,
    /// The harness refused to operate in the directory it was pointed at
    /// (Codex's `Not inside a trusted directory and --skip-git-repo-check was
    /// not specified`, which exits within a few hundred milliseconds). A
    /// **precondition** refusal like [`FailureKind::InputTooLarge`]: the check
    /// runs before the request is, so nothing of the task was attempted and
    /// another identity — one whose trust list covers the directory — can still
    /// run it.
    UntrustedDirectory,
    /// The request exceeded the input size the provider accepts and was refused
    /// before the model was called (Codex reports the machine-readable
    /// `input_error_code: input_too_large` with `max_chars`/`actual_chars`). The
    /// other precondition refusal: no tokens were spent, and a candidate with a
    /// larger window can still run the task.
    InputTooLarge,
    /// The harness reported it would run the conversation under a model other
    /// than the one requested, and oneharness refused the turn before it
    /// started (codex's app-server names the thread's model on its
    /// `thread/start` / `thread/resume` response, before any token is spent).
    /// The third precondition refusal: no work was done, no token was spent, and
    /// the next identity may honour the model.
    ModelMismatch,
    /// Codex's provider reported that serving capacity was exhausted. When the
    /// result has no answer, tool event, or billed usage, oneharness may retry
    /// this identity briefly and then try a fallback identity; work evidence
    /// preserves the classified failure without retry or fallback.
    ServerOverloaded,
    /// The shared Codex app-server a pooled dispatch's turn ran on went away
    /// under it: the connection reached EOF, was closed, or refused a write
    /// (`EPIPE`) while oneharness's own record showed a turn started and not
    /// completed. The server crashed or was restarted, not the turn — so it is
    /// never a turn failure, and a fallback chain moves on to its next
    /// candidate (`"app-server-lost"`).
    AppServerLost,
}

impl FailureKind {
    /// Every kind, for the callers that must reason over the closed set — the
    /// history version gates and the schema generator that mirrors them. Pinned
    /// against the enum's own generated schema (`sdk_schema`), so a kind added
    /// without being listed here fails the gate rather than shipping an SDK that
    /// accepts it at a version whose reader refuses it.
    pub const ALL: [FailureKind; 11] = [
        FailureKind::Auth,
        FailureKind::RateLimit,
        FailureKind::ModelNotFound,
        FailureKind::Quota,
        FailureKind::SessionNotFound,
        FailureKind::ToolDeferred,
        FailureKind::UntrustedDirectory,
        FailureKind::InputTooLarge,
        FailureKind::ModelMismatch,
        FailureKind::ServerOverloaded,
        FailureKind::AppServerLost,
    ];

    /// Whether this kind asserts the harness or its provider **refused to run
    /// the task** — the claim a run's own completed, billed envelope
    /// contradicts, and the one
    /// [`RunResult::with_work_evidence`][wwe] declines to stamp there.
    ///
    /// Every kind but [`FailureKind::ToolDeferred`] and
    /// [`FailureKind::AppServerLost`] is a refusal: each says the
    /// request was rejected (`auth`, `rate_limit`, `quota`, `model_not_found`)
    /// or never made at all (`session_not_found`, `untrusted_directory`,
    /// `input_too_large`, `model_mismatch`). `ToolDeferred` is the odd one out because it is not a
    /// refusal at all: the turn completed, and the finding is about *what it
    /// produced* — a deferred builtin tool call instead of an executed one — so
    /// a clean, billed exit is exactly the shape it describes rather than one
    /// that contradicts it. `AppServerLost` is not one either: the turn was
    /// accepted and running when the shared server under it went away, so work
    /// the turn billed before that is what happened, not a contradiction.
    ///
    /// Matched exhaustively on purpose: a new kind is a deliberate answer to
    /// "does a completed, billed run refute this?", not an inherited default.
    ///
    /// [wwe]: crate::domain::report::RunResult::with_work_evidence
    #[must_use]
    pub fn is_refusal(self) -> bool {
        match self {
            FailureKind::Auth
            | FailureKind::RateLimit
            | FailureKind::ModelNotFound
            | FailureKind::Quota
            | FailureKind::SessionNotFound
            | FailureKind::UntrustedDirectory
            | FailureKind::InputTooLarge
            | FailureKind::ModelMismatch
            | FailureKind::ServerOverloaded => true,
            FailureKind::ToolDeferred | FailureKind::AppServerLost => false,
        }
    }

    /// The snake_case token this kind serializes to — for the few call sites
    /// (stderr diagnostics, the fallback reason map) that need the raw string.
    pub fn as_str(self) -> &'static str {
        match self {
            FailureKind::Auth => "auth",
            FailureKind::RateLimit => "rate_limit",
            FailureKind::ModelNotFound => "model_not_found",
            FailureKind::Quota => "quota",
            FailureKind::SessionNotFound => "session_not_found",
            FailureKind::ToolDeferred => "tool_deferred",
            FailureKind::UntrustedDirectory => "untrusted_directory",
            FailureKind::InputTooLarge => "input_too_large",
            FailureKind::ModelMismatch => "model_mismatch",
            FailureKind::ServerOverloaded => "server_overloaded",
            FailureKind::AppServerLost => "app_server_lost",
        }
    }
}

/// A classified failure reason plus where it was read from (`stderr`/`stdout`)
/// and, when the harness stated one, the provider's own machine-readable
/// account of it.
#[derive(Debug, Clone, PartialEq)]
pub struct FailureReading {
    pub kind: FailureKind,
    pub source: String,
    /// The provider's own statement of the cause, verbatim — Codex's
    /// `{"input_error_code":"input_too_large","max_chars":…,"actual_chars":…}`.
    /// `None` for every reading whose text carried no machine-readable detail,
    /// which is most of them. Never paraphrased: a caller that acts on the code
    /// (shard the input, pick a larger window) needs the provider's spelling,
    /// and one that only displays it loses nothing.
    pub detail: Option<String>,
}

/// A classified refusal before it is tagged with the stream it was read from —
/// what every text classifier below returns. Separate from [`FailureReading`]
/// because the source is [`scan_failure`]'s to add, not each classifier's.
#[derive(Debug, Clone, PartialEq)]
struct Refusal {
    kind: FailureKind,
    detail: Option<String>,
}

impl Refusal {
    /// A refusal read from prose alone, with nothing machine-readable in it.
    fn plain(kind: FailureKind) -> Self {
        Refusal { kind, detail: None }
    }
}

/// Adapter-specific failure vocabulary understood by the signal classifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailureDialect {
    Generic,
    ClaudeCode,
    Codex,
}

/// A deferred-tool dead-end detected in a harness's output: the harness ended a
/// turn by *deferring* a builtin tool call instead of executing it, so the run
/// exits cleanly (status `ok`) with an empty result. This is Claude Code's
/// behavior in a bridged/managed deployment (where `tengu_non_deferrable_builtins`
/// is empty), and without detection it masquerades as a schema/output failure —
/// see issue #1114.
#[derive(Debug, Clone, PartialEq)]
pub struct DeferredTool {
    /// The name of the tool the harness deferred (e.g. `Read`), when the output
    /// named it; `None` when the shape signalled a deferral without a name.
    pub tool: Option<String>,
}

/// Best-effort token/cost usage from a harness's stdout. Tries the two known
/// shapes in order: a single terminal event whose totals are self-contained
/// (Claude Code), else a sum across per-step events (OpenCode). `None` when
/// neither yields anything.
pub fn extract_usage(stdout: &str) -> Option<UsageReading> {
    let candidates = json_candidates(stdout);
    // Single-object shape: a terminal event carrying the whole-run `usage` block
    // and/or a top-level dollar cost. Scan from the end so the final event wins
    // for stream output.
    if let Some(reading) = candidates.iter().rev().find_map(single_object_usage) {
        return Some(reading);
    }
    // Per-step shape: each event reports only its own step's tokens/cost, so the
    // run total is their sum.
    summed_step_usage(&candidates)
}

/// Usage from one self-contained event: a top-level `usage` object
/// (`input_tokens`/`output_tokens`) plus a top-level `total_cost_usd`/`cost_usd`.
/// `None` when the event carries no usage. This is Claude Code's `result` shape.
fn single_object_usage(value: &Value) -> Option<UsageReading> {
    let obj = value.as_object()?;
    let mut usage = Usage::default();
    if let Some(u) = obj.get("usage").and_then(Value::as_object) {
        usage.input_tokens = u.get("input_tokens").and_then(Value::as_u64);
        usage.output_tokens = u.get("output_tokens").and_then(Value::as_u64);
        usage.cache_read_tokens = u
            .get("cache_read_input_tokens")
            .or_else(|| u.get("cached_input_tokens"))
            .and_then(Value::as_u64);
        usage.cache_write_tokens = u.get("cache_creation_input_tokens").and_then(Value::as_u64);
    }
    usage.cost_usd = obj
        .get("total_cost_usd")
        .or_else(|| obj.get("cost_usd"))
        .and_then(Value::as_f64);
    (!usage.is_empty()).then(|| UsageReading {
        usage,
        source: "json".to_string(),
    })
}

/// Version of the deliberately small fallback price table. Prices are USD per
/// million tokens and only apply to exact documented model families; an unknown
/// alias stays unpriced rather than being guessed.
pub const MODEL_PRICE_TABLE_VERSION: &str = "openai-2026-07-19";

pub fn apply_model_price(usage: &mut Usage, harness: &str, model: Option<&str>) {
    if usage.cost_usd.is_some() {
        return;
    }
    // Harness identity is the provider boundary available in the normalized
    // result. Never apply an OpenAI table to another provider's coincidentally
    // identical model alias.
    if harness != "codex" {
        return;
    }
    let Some(model) = model else { return };
    let (input_rate, cached_rate, output_rate) = match model {
        // OpenAI API pricing snapshot named by MODEL_PRICE_TABLE_VERSION.
        "gpt-5-codex" => (1.25, 0.125, 10.0),
        _ => return,
    };
    let (Some(input), Some(output)) = (usage.input_tokens, usage.output_tokens) else {
        return;
    };
    let cached = usage.cache_read_tokens.unwrap_or(0).min(input);
    let uncached = input - cached;
    usage.cost_usd = Some(
        (uncached as f64 * input_rate + cached as f64 * cached_rate + output as f64 * output_rate)
            / 1_000_000.0,
    );
}

/// Usage summed across OpenCode `step_finish` events, each of which reports only
/// its own step under `part` (`part.tokens.{input,output}`, `part.cost`). A
/// consumer that wants the run total needs the sum; taking the last step would
/// silently undercount. `None` when no step carried usage.
fn summed_step_usage(candidates: &[Value]) -> Option<UsageReading> {
    let mut usage = Usage::default();
    let mut saw_usage = false;
    for value in candidates {
        let Some(obj) = value.as_object() else {
            continue;
        };
        if obj.get("type").and_then(Value::as_str) != Some("step_finish") {
            continue;
        }
        let Some(part) = obj.get("part").and_then(Value::as_object) else {
            continue;
        };
        if let Some(tokens) = part.get("tokens").and_then(Value::as_object) {
            saw_usage |= add_u64(&mut usage.input_tokens, tokens.get("input"));
            saw_usage |= add_u64(&mut usage.output_tokens, tokens.get("output"));
            if let Some(cache) = tokens.get("cache").and_then(Value::as_object) {
                saw_usage |= add_u64(&mut usage.cache_read_tokens, cache.get("read"));
                saw_usage |= add_u64(&mut usage.cache_write_tokens, cache.get("write"));
            }
        }
        saw_usage |= add_f64(&mut usage.cost_usd, part.get("cost"));
    }
    saw_usage.then(|| UsageReading {
        usage,
        source: "json:summed-steps".to_string(),
    })
}

/// Add `value` (as a u64) into `acc`, returning whether a number was present.
fn add_u64(acc: &mut Option<u64>, value: Option<&Value>) -> bool {
    match value.and_then(Value::as_u64) {
        Some(v) => {
            *acc = Some(acc.unwrap_or(0) + v);
            true
        }
        None => false,
    }
}

/// Add `value` (as an f64) into `acc`, returning whether a number was present.
fn add_f64(acc: &mut Option<f64>, value: Option<&Value>) -> bool {
    match value.and_then(Value::as_f64) {
        Some(v) => {
            *acc = Some(acc.unwrap_or(0.0) + v);
            true
        }
        None => false,
    }
}

/// Best-effort harness session id from stdout (the handle a harness exposes for
/// `--resume`-style continuation). Reads the first non-empty session id string
/// found in the emitted JSON, accepting the snake_case `session_id` (Claude Code,
/// Cursor, Qwen), the camelCase `sessionID` (OpenCode), and Codex's `thread_id`
/// (emitted on its `thread.started` event under `--json`); `None` when absent. A
/// harness that emits no id headlessly (Goose, Copilot) yields `None` — its
/// continuation handle is caller-supplied, never scraped.
pub fn extract_session(stdout: &str) -> Option<String> {
    json_candidates(stdout).into_iter().find_map(|value| {
        let obj = value.as_object()?;
        obj.get("session_id")
            .or_else(|| obj.get("sessionID"))
            .or_else(|| obj.get("thread_id"))
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
    })
}

/// Coarse, best-effort reason a run failed, scanned from its stderr then stdout.
///
/// This is deliberately distinct from `Status`: `Status` describes oneharness's
/// relationship to the process (it exited non-zero), while this hints at *why* —
/// so a consumer can separate retryable conditions (auth, rate limit) from a
/// broken request (unknown model). It never changes exit-code semantics and is
/// `None` when no known signal matches.
pub fn classify_failure(stdout: &str, stderr: &str) -> Option<FailureReading> {
    scan_failure(stdout, stderr, |text| {
        match_failure(text).map(Refusal::plain)
    })
}

/// The first reading `read` yields over stderr then stdout, tagged with the
/// stream it came from. Every text classifier scans that same pair in that same
/// order — stderr first, because a harness that refuses before running says so
/// there — so they share one traversal rather than restating it.
fn scan_failure(
    stdout: &str,
    stderr: &str,
    read: impl Fn(&str) -> Option<Refusal>,
) -> Option<FailureReading> {
    [("stderr", stderr), ("stdout", stdout)]
        .into_iter()
        .find_map(|(source, text)| {
            read(text).map(|refusal| FailureReading {
                kind: refusal.kind,
                source: source.to_string(),
                detail: refusal.detail,
            })
        })
}

/// Harness-aware failure classification for adapter-specific text surfaces.
///
/// Claude Code's subscription exhaustion does not carry the generic `quota`
/// vocabulary: headless runs report a session/weekly limit message instead.
/// Keep those phrases scoped to that adapter so another harness mentioning a
/// "session limit" in an unrelated error is not silently treated as exhausted.
/// Codex is deliberately absent: it declares its usage limit in a structured
/// `turn.failed` event (see [`detect_harness_provider_failure`]), so scanning its
/// whole transcript would risk reading an agent's own mention of a usage limit
/// as exhaustion and silently re-running the task on another account.
///
/// Claude Code's *login* refusal is the second adapter signal, and it is scoped
/// the same way for the same reason — see the private `harness_auth_refusal`
/// rule.
///
/// The adapter signals are checked **before** the generic vocabulary — see the
/// private `harness_quota_failure` rule for why that order is load-bearing —
/// and the precondition refusals (`precondition_refusal`) and the
/// unknown-session one (`unknown_session_rejection`) sit between them, so a
/// rejection that names exactly what it refused is read as that rather than
/// falling to a coarser match.
pub fn classify_harness_failure(
    dialect: FailureDialect,
    stdout: &str,
    stderr: &str,
) -> Option<FailureReading> {
    // The harness's own accounting for the whole run: a limit message printed on
    // stderr says nothing about whether the run got anywhere, so the work
    // evidence has to come from the transcript, not from the matched text.
    let work = stdout_work_evidence(stdout);
    scan_failure(stdout, stderr, |text| {
        harness_quota_failure(dialect, text, work).map(Refusal::plain)
    })
    .or_else(|| {
        scan_failure(stdout, stderr, |text| {
            harness_auth_refusal(dialect, text, work).map(Refusal::plain)
        })
    })
    .or_else(|| scan_failure(stdout, stderr, precondition_refusal))
    .or_else(|| {
        scan_failure(stdout, stderr, |text| {
            unknown_session_rejection(text).map(Refusal::plain)
        })
    })
    .or_else(|| classify_failure(stdout, stderr))
}

/// A refusal a harness reaches **before the request is made at all** — its own
/// precondition check, failed. Two are recognized, both read from real captures:
///
/// - `Not inside a trusted directory and --skip-git-repo-check was not
///   specified` — Codex declining to operate in the cwd it was pointed at. It
///   exits within a few hundred milliseconds, having called no model.
/// - `Input exceeds the maximum length of <n> characters` with the
///   machine-readable `{"input_error_code":"input_too_large","max_chars":…,
///   "actual_chars":…}` beside it — Codex refusing an over-long turn before the
///   model is called.
///
/// Both satisfy [`startup_failure_reason`][sfr]'s own criterion — the candidate
/// *could not run the task at all* — so both fall through to the next candidate,
/// which may well have the room or the trust the refused one lacked. Left
/// unclassified they were plain non-zero task failures that stopped the chain
/// with healthy candidates untried.
///
/// Dialect-agnostic, on [`unknown_session_rejection`]'s reasoning: each phrase
/// says only the thing it says, whichever CLI printed it, and no harness emits
/// another's wording. Over-reading is bounded by the rule that bounds every
/// fall-through kind — a candidate with work evidence never falls through
/// ([`crate::domain::fallback::RunWork`]) — so an agent that merely *wrote* one
/// of these sentences mid-run cannot hand its task on.
///
/// [sfr]: crate::domain::fallback::startup_failure_reason
fn precondition_refusal(text: &str) -> Option<Refusal> {
    let lower = text.to_lowercase();
    if lower.contains("not inside a trusted directory") {
        return Some(Refusal::plain(FailureKind::UntrustedDirectory));
    }
    // The provider's own code is the primary signal and the prose is the
    // fallback, not the other way round: the sentence carries a formatted limit
    // that will be reworded long before the code is renamed.
    if lower.contains(INPUT_TOO_LARGE_CODE) || lower.contains("input exceeds the maximum length") {
        return Some(Refusal {
            kind: FailureKind::InputTooLarge,
            detail: provider_error_data(text),
        });
    }
    None
}

/// Codex's machine-readable input-size code, as it spells it.
const INPUT_TOO_LARGE_CODE: &str = "input_too_large";

/// The most of a provider's own error data that is carried into the report.
/// The observed object is ~80 bytes; this is headroom, and it is a bound rather
/// than a formality because the text it reads is a harness's raw output.
const MAX_DETAIL_BYTES: usize = 512;

/// The provider's machine-readable error object embedded in `text`, verbatim —
/// the `{"input_error_code":"input_too_large","max_chars":1048576,
/// "actual_chars":1168716}` Codex prints beside its refusal sentence. `None`
/// when the text carries no such object.
///
/// It is not on a line of its own (the capture reads `… (code -32602), data:
/// {…}`), so it cannot be recovered by [`json_candidates`]; the span is taken
/// from the brace before the key to the first brace after it. That assumes a
/// **flat** object, which every observed payload is — and a nested one would
/// end the span early, so the parse below rejects it and the caller gets `None`
/// rather than a truncated claim.
///
/// This is a harness's raw output, so the span is bounded, parsed, and required
/// to be an object whose `input_error_code` is the very code that classified
/// the refusal. Handing back text that merely *mentions* the key would let an
/// unrelated (or differently-coded) object be reported as the cause of this
/// one — and the caller acts on it. What survives all three is handed back as
/// the provider wrote it, never re-serialized.
fn provider_error_data(text: &str) -> Option<String> {
    let at = text.find("\"input_error_code\"")?;
    let start = text[..at].rfind('{')?;
    let rest = &text[start..];
    let end = rest.find('}')? + 1;
    let object = rest.get(..end).filter(|o| o.len() <= MAX_DETAIL_BYTES)?;
    let parsed: Value = serde_json::from_str(object).ok()?;
    (parsed.get("input_error_code").and_then(Value::as_str) == Some(INPUT_TOO_LARGE_CODE))
        .then(|| object.to_string())
}

/// A harness's refusal to continue a session it cannot find — the
/// [`FailureKind::SessionNotFound`] rejection a `--resume`/`--session` run gets
/// when the stored token belongs to some *other* identity's session namespace
/// (each Claude Code `CLAUDE_CONFIG_DIR`, each Codex home, is disjoint).
///
/// Dialect-agnostic, like [`is_provider_failure_envelope`]: each phrase below
/// says the same thing — *this conversation does not exist here* — whichever CLI
/// printed it, and no harness emits another harness's wording. Every one was read
/// from a real invocation (a bogus session id resumed against the installed CLI),
/// never guessed:
///
/// - claude-code: `No conversation found with session ID: <id>`
/// - codex: `thread/resume failed: no rollout found for thread id <id>`
/// - opencode: `Error: Session not found`
/// - qwen: `No saved session found with title "<id>"`
///
/// cursor is deliberately absent: its refusal wording was never verified, and a
/// guessed phrase is exactly the drift this list exists to avoid. Its
/// unknown-session run therefore stays unclassified — a chain-stopping real
/// failure, the honest default.
///
/// Checked *after* the quota reading and *before* the generic vocabulary, the
/// specific-first order [`harness_quota_failure`] documents. Over-reading is
/// bounded by the same rule that bounds every fall-through kind: a candidate with
/// work evidence never falls through ([`crate::domain::fallback::RunWork`]), so
/// an agent that merely *wrote* one of these sentences mid-run cannot hand its
/// task to the next candidate.
fn unknown_session_rejection(text: &str) -> Option<FailureKind> {
    let text = text.to_lowercase();
    [
        "no conversation found with session id",
        "no rollout found for thread id",
        "session not found",
        "no saved session found with title",
    ]
    .iter()
    .any(|needle| text.contains(needle))
    .then_some(FailureKind::SessionNotFound)
}

/// The adapter-specific subscription-limit signature in `text` — a [`FailureKind::Quota`]
/// rejection — or `None` when this dialect has none, the text does not carry it,
/// or the harness's evidence is [`RunWork::Done`] — it **did work** before the
/// limit landed.
///
/// Two rules are encoded here, and both are load-bearing.
///
/// **Order.** Every caller must consult this *before* [`match_failure`]. A Claude
/// Code session-limit rejection embeds the HTTP status of the rejection
/// (`"api_error_status":429`) in the same record that carries the limit message,
/// and the generic vocabulary reads any `429` as the coarse
/// [`FailureKind::RateLimit`] — a deliberately *non*-fall-through kind, since a
/// rate limit is a transient hiccup of a working harness. Scanning generic-first
/// therefore classified an exhausted subscription as a transient blip and stranded
/// a configured fallback chain with authenticated candidates untried (issue #1211).
/// The adapter signal is the more specific reading of the same bytes, so it wins.
///
/// **Work done, not error text.** `quota` is the fall-through kind: it means the
/// candidate *could not run the task at all*, so the next one should try. That is
/// only true of a rejection that did no work. A limit that lands mid-run leaves
/// real tokens spent and possibly a partial answer, and falling through it burns
/// the next candidate's quota re-running work that already happened. So the same
/// message with [`RunWork::Done`] reads as an ordinary run: the generic vocabulary still
/// gets its turn (a mid-run `429` lands as `rate_limit`, which stops the chain),
/// and a limit with no generic signal at all stays unclassified — also a stop.
///
/// Scope: this gate covers the *adapter* limit signature only — a text match, so
/// it is the only reading available on a surface with no record to inspect. A
/// rejection that does arrive as a JSON record is also read structurally by
/// [`zero_work_rate_limit_rejection`], which is what keeps a rephrased message
/// from stopping a chain. The generic `insufficient_quota` / `credit balance`
/// vocabulary in [`match_failure`] means the account is out of money rather than
/// out of session, and is left as it was.
/// Codex's [`codex_turn_failure`] needs no gate: its `turn.failed` event carries
/// no accounting to read, and inventing a shape for one would be a guess.
fn harness_quota_failure(
    dialect: FailureDialect,
    text: &str,
    work: RunWork,
) -> Option<FailureKind> {
    (dialect == FailureDialect::ClaudeCode).then_some(())?;
    claude_subscription_limit(text).then_some(())?;
    (work == RunWork::None).then_some(FailureKind::Quota)
}

/// The adapter-specific **authentication refusal** in `text` — a
/// [`FailureKind::Auth`] rejection — or `None` when this dialect has none, the
/// text does not carry it, or the harness's evidence is [`RunWork::Done`] — it
/// **did work** before the sentence appeared.
///
/// Claude Code's unauthenticated answer is `Not logged in · Please run /login`,
/// and not one of those words is in the generic vocabulary [`match_failure`]
/// scans: no `unauthorized`, no `credentials`, no `401`. So the reading fell
/// through to whatever else the same failed run happened to say — and its
/// terminal record carries incidental digits (a `duration_ms` of `429`) that
/// the coarse `429` needle reads as a rate limit. An unauthenticated identity
/// was reported as rate-limited, which sends an operator to wait out a limit
/// that does not exist instead of authenticating the config directory the run
/// named (issue #1290).
///
/// Both of [`harness_quota_failure`]'s rules hold here unchanged: checked
/// before the generic vocabulary, and only of a run whose own accounting
/// reports [`RunWork::None`]. What the second one means for *this* refusal is
/// that a
/// harness which spent tokens was logged in, so the sentence is one its agent
/// wrote rather than one it received.
///
/// Scope: the Claude dialect only, rather than dialect-agnostic like
/// [`unknown_session_rejection`]. That list holds phrases a CLI can only be
/// saying about itself; `not logged in` is ordinary English about any service
/// an agent might touch, so leaving it unscoped would let another harness's
/// unrelated text read as a login refusal.
fn harness_auth_refusal(dialect: FailureDialect, text: &str, work: RunWork) -> Option<FailureKind> {
    (dialect == FailureDialect::ClaudeCode).then_some(())?;
    claude_login_refusal(text).then_some(())?;
    (work == RunWork::None).then_some(FailureKind::Auth)
}

/// Claude Code's login-refusal signature, matched on either half of the
/// captured sentence `Not logged in · Please run /login` so a reworded half
/// does not strand the other. `/login` is anchored to the instruction naming
/// it, never matched bare: that spelling is also the path of an ordinary URL,
/// and a false positive here reports a genuine task failure as an
/// unauthenticated identity.
fn claude_login_refusal(text: &str) -> bool {
    let text = text.to_lowercase();
    text.contains("not logged in") || text.contains("please run /login")
}

/// The run-level [`RunWork`] evidence in `stdout` — [`RunWork::Done`] as soon as
/// any one record in it reports work, on [`record_work_evidence`]'s reading.
fn stdout_work_evidence(stdout: &str) -> RunWork {
    if json_candidates(stdout)
        .iter()
        .any(|record| record_work_evidence(record) == RunWork::Done)
    {
        RunWork::Done
    } else {
        RunWork::None
    }
}

/// This record's own accounting as [`RunWork`] evidence — the raw-record
/// counterpart of [`RunWork::from_result`], answering in the same type so a
/// classifier here and a fall-through verdict there read one value rather than
/// two spellings of it. [`RunWork::Done`] on billed usage
/// ([`Usage::reports_billed_work`], the shared definition) or a non-empty
/// per-model usage map (Claude Code's `modelUsage`, which is `{}` when no model
/// was ever reached — a raw-record witness with no counterpart in the normalized
/// [`Usage`]).
///
/// Absent accounting is deliberately **not** work. A bare `You've hit your
/// session limit` line on stderr carries no usage block at all, and it is still
/// a zero-work rejection; requiring positive proof of zero would strand exactly
/// the callers this rule exists to serve. Only a harness that says it spent
/// something counts as having run.
fn record_work_evidence(value: &Value) -> RunWork {
    let witnessed = single_object_usage(value)
        .is_some_and(|reading| reading.usage.reports_billed_work())
        || value
            .get("modelUsage")
            .and_then(Value::as_object)
            .is_some_and(|models| !models.is_empty());
    if witnessed {
        RunWork::Done
    } else {
        RunWork::None
    }
}

/// Classify a provider-declared failed result even when its CLI exits zero.
///
/// Some harnesses, including Claude Code on Windows, report an API rejection in
/// a terminal JSON record that still exits successfully. Restricting this check
/// to records the harness itself declares as an API failure (see the private
/// `is_provider_failure_envelope` gate) avoids treating incidental warning text
/// in an otherwise successful transcript as failure.
pub fn detect_provider_failure(stdout: &str) -> Option<FailureReading> {
    detect_harness_provider_failure(FailureDialect::Generic, stdout)
}

/// Provider-declared failure classification with adapter-specific quota
/// surfaces. A terminal record the harness declares as an API failure (see the
/// private `is_provider_failure_envelope` gate) is the machine signal; matching
/// its complete JSON record captures provider metadata as well as result text
/// while avoiding unstructured output outside that explicit failure record.
/// Codex declares its failure differently, so it gets its own record shape
/// below.
pub fn detect_harness_provider_failure(
    dialect: FailureDialect,
    stdout: &str,
) -> Option<FailureReading> {
    terminal_provider_refusal(dialect, stdout).map(|(refusal, _)| FailureReading {
        kind: refusal.kind,
        source: "stdout".to_string(),
        detail: refusal.detail,
    })
}

/// The latest record in `stdout` that classifies as a provider failure, with
/// the refusal read from it — the one scan both
/// [`detect_harness_provider_failure`] and [`model_refused_for_account`] read,
/// so a run's kind and its account-refusal verdict come from the same record.
fn terminal_provider_refusal(dialect: FailureDialect, stdout: &str) -> Option<(Refusal, Value)> {
    json_candidates(stdout).into_iter().rev().find_map(|value| {
        let refusal = error_record_failure(dialect, &value)
            .map(Refusal::plain)
            .or_else(|| codex_turn_failure(dialect, &value))?;
        Some((refusal, value))
    })
}

/// Whether `stdout` carries Codex's refusal of the requested model **for the
/// account it is signed in with** — the reading that classified a run
/// [`FailureKind::ModelNotFound`] from its terminal `400 invalid_request_error`
/// ("… is not supported when using Codex with …") rather than by the generic
/// vocabulary.
///
/// The fallback verdict needs the difference and the kind alone cannot carry
/// it: an unknown model is a configuration mistake every candidate shares, so
/// on a single-model chain it stops; a model *this account* may not use is a
/// property of the identity, so the next candidate — another harness or
/// another account — can still run the task. Only the record that produced the
/// classification is read, so an earlier account refusal cannot carry a later
/// generic model rejection past a single-model chain.
#[must_use]
pub fn model_refused_for_account(dialect: FailureDialect, stdout: &str) -> bool {
    dialect == FailureDialect::Codex
        && terminal_provider_refusal(dialect, stdout).is_some_and(|(_, value)| {
            codex_exec_turn_error(&value)
                .or_else(|| codex_app_server_turn_error(&value))
                .is_some_and(|error| codex_model_refused_for_account(error).is_some())
        })
}

fn error_record_failure(dialect: FailureDialect, value: &Value) -> Option<FailureKind> {
    is_provider_failure_envelope(value).then_some(())?;
    let serialized = value.to_string();
    // Adapter signal first, and only when this record did no work: see
    // `harness_quota_failure` for both rules. Claude's terminal record carries
    // whole-run totals, so its own accounting is the run's accounting.
    let work = record_work_evidence(value);
    harness_quota_failure(dialect, &serialized, work)
        .or_else(|| harness_auth_refusal(dialect, &serialized, work))
        .or_else(|| zero_work_rate_limit_rejection(value, work))
        .or_else(|| provider_auth_rejection(value))
        .or_else(|| match_failure(&serialized))
}

/// The **structural** quota rule: a provider rejection whose record declares
/// `api_error_status: 429` and whose own accounting says it did nothing is a
/// [`FailureKind::Quota`] rejection, whatever prose it carries.
///
/// The structure is the rule and [`claude_subscription_limit`] is a fast path
/// over surfaces with no record to read, because the errors are asymmetric: an
/// unrecognized wording strands the whole chain (it has been one short twice —
/// issue #1211), while over-reading a transient 429 merely hands the task on,
/// which is what a fallback chain is for.
///
/// Both halves are load-bearing. **`429` specifically**, not any rejection: it
/// says *this identity may not run right now*, the one condition another
/// identity can serve — a zero-work `500` is a provider fault the next candidate
/// hits too, and `401`/`403` already fall through as [`FailureKind::Auth`]
/// ([`provider_auth_rejection`]).
/// **Zero work**, on the [`record_work_evidence`] reading the adapter path uses
/// so the two can never disagree: a 429 that landed after real tokens describes
/// a run, and falling through it pays the next candidate to redo it, so that
/// record keeps its chain-stopping [`FailureKind::RateLimit`].
///
/// Dialect-agnostic like [`is_provider_failure_envelope`]: `api_error_status`
/// states what the provider did, whichever harness emits it. The *prose* is what
/// stays dialect-scoped.
fn zero_work_rate_limit_rejection(value: &Value, work: RunWork) -> Option<FailureKind> {
    (work == RunWork::None).then_some(())?;
    (value.get("api_error_status").and_then(Value::as_u64) == Some(429))
        .then_some(FailureKind::Quota)
}

/// The **structural** auth rule: a provider rejection whose record declares
/// `api_error_status: 401` or `403` is [`FailureKind::Auth`], whatever else the
/// record says.
///
/// The generic vocabulary cannot be trusted with this record, because it scans
/// the whole serialized record and reads `rate_limit` before `auth`: Claude
/// Code's invalid-key answer (`"api_error_status":401`, `Invalid API key · Fix
/// external API key`) also carries random UUIDs and millisecond timings, and
/// whenever one of them happens to contain `429` the coarse needle reported the
/// invalid key as a rate limit. That stopped a fallback chain an auth failure
/// should have handed on, and intermittently failed the live `same-harness auth
/// fallback` probe. The status code is what the provider did, so it decides
/// before any wording does.
///
/// Dialect-agnostic for the same reason as [`zero_work_rate_limit_rejection`].
fn provider_auth_rejection(value: &Value) -> Option<FailureKind> {
    matches!(
        value.get("api_error_status").and_then(Value::as_u64),
        Some(401 | 403)
    )
    .then_some(FailureKind::Auth)
}

/// Whether the harness itself declares this terminal record an **API failure** —
/// the gate that separates a provider rejection from incidental wording in a
/// healthy transcript. Three equivalent declarations, each read from a real
/// capture rather than guessed: `is_error: true`, `terminal_reason:
/// "api_error"`, or a numeric `api_error_status`.
///
/// `is_error` alone is **not** a sufficient gate: a Claude Code session-limit
/// rejection that did no work omits it entirely and reports `subtype: "success"`
/// beside the other two, so an `is_error`-only gate skipped the exact record a
/// fallback chain exists to route around (issue #1211). Dialect-agnostic — these
/// fields say the provider rejected the turn, whichever harness emits them —
/// while what is *read out* of the record stays dialect-scoped.
fn is_provider_failure_envelope(value: &Value) -> bool {
    value.get("is_error").and_then(Value::as_bool) == Some(true)
        || value.get("terminal_reason").and_then(Value::as_str) == Some("api_error")
        || value.get("api_error_status").is_some_and(Value::is_number)
}

/// Codex reports usage exhaustion, serving overload and an account's refusal of
/// the requested model inside its stdout stream — never on stderr — and the
/// process can still exit zero. Only an error the harness attached to a terminal
/// turn is read, and only when it carries one of those machine-readable
/// signatures; ordinary failed turns stay unclassified.
///
/// Both transports are read because the *run* picks one, not the caller: an
/// ordinary dispatch gets `codex exec`'s event stream, a `--control` dispatch
/// gets the `codex app-server` protocol.
fn codex_turn_failure(dialect: FailureDialect, value: &Value) -> Option<Refusal> {
    (dialect == FailureDialect::Codex).then_some(())?;
    let error = codex_exec_turn_error(value).or_else(|| codex_app_server_turn_error(value))?;
    if codex_server_overloaded_error(error) {
        Some(Refusal::plain(FailureKind::ServerOverloaded))
    } else if codex_usage_limit_error(error) {
        Some(Refusal::plain(FailureKind::Quota))
    } else {
        codex_model_refused_for_account(error).map(|detail| Refusal {
            kind: FailureKind::ModelNotFound,
            detail: Some(detail),
        })
    }
}

/// The provider's own error object when a Codex terminal error says the
/// requested model **is not available to the account Codex is signed in
/// with** — captured verbatim from `codex exec -m gpt-5.5` under a ChatGPT
/// sign-in (`tests/fixtures/codex-model-unsupported-for-account.jsonl`):
///
/// ```text
/// {"type":"error","status":400,"error":{"type":"invalid_request_error",
///  "message":"The 'gpt-5.5' model is not supported when using Codex with a ChatGPT account."}}
/// ```
///
/// That object arrives as the *string* value of the error's `message`, so it is
/// parsed and matched on its stable parts only: status `400`, type
/// `invalid_request_error`, and the `not supported when using Codex with`
/// wording. The model name and the account kind are deliberately not part of
/// the signature — the refusal means the same for any model and any plan. Every
/// other `400` (a bad parameter, a malformed request) is a request this
/// identity's neighbour would refuse too, so it stays unclassified and stops the
/// chain. Handed back as the provider wrote it, bounded like
/// [`provider_error_data`], so the cause travels with the fall-through record.
// llmlint: ignore-block[contracts_have_one_source_or_a_drift_gate] The authoritative surface is OpenAI's answer to a Codex account of one particular kind asking for a model that kind may not use, which neither the deterministic gate nor the paid live suite can produce on demand; the signature is held to a byte-for-byte capture of the real refusal (tests/fixtures/codex-model-unsupported-for-account.jsonl, codex-cli 0.159.2), as tests/fixtures/AGENTS.md requires, and it fails closed: a reworded or restructured refusal is left unclassified and stops the chain exactly as before this reading existed, never a false fall-through.
fn codex_model_refused_for_account(error: &Value) -> Option<String> {
    let message = error
        .get("message")
        .and_then(Value::as_str)
        .filter(|m| m.len() <= MAX_DETAIL_BYTES)?;
    let provider: Value = serde_json::from_str(message).ok()?;
    (provider.get("status").and_then(Value::as_u64) == Some(400)).then_some(())?;
    let inner = provider.get("error")?;
    (inner.get("type").and_then(Value::as_str) == Some("invalid_request_error")).then_some(())?;
    inner
        .get("message")
        .and_then(Value::as_str)
        .is_some_and(|m| {
            m.to_lowercase()
                .contains("not supported when using codex with")
        })
        .then(|| message.to_string())
}
// llmlint: ignore-end[contracts_have_one_source_or_a_drift_gate]

/// Whether a Codex terminal error carries its stable overload code. Both field
/// spellings are emitted by supported Codex surfaces: app-server uses camel
/// case while rollout/exec captures expose snake case.
fn codex_server_overloaded_error(error: &Value) -> bool {
    ["codexErrorInfo", "codex_error_info", "input_error_code"]
        .iter()
        .filter_map(|key| error.get(key).and_then(Value::as_str))
        .any(|code| matches!(code, "serverOverloaded" | "server_overloaded"))
}

/// The error object `codex exec` attaches to its `turn.failed` stream event.
fn codex_exec_turn_error(value: &Value) -> Option<&Value> {
    (value.get("type").and_then(Value::as_str) == Some("turn.failed")).then_some(())?;
    value.get("error")
}

/// The error object a `codex app-server` frame attaches to a turn that is not
/// going to complete — either frame that carries one, since a transcript may
/// have kept only one of them. Both shapes are verbatim from a controlled run
/// against an exhausted account
/// (`tests/fixtures/codex-app-server-usage-limit.jsonl`).
///
/// Two conditions are not visible in the shapes themselves.
/// **`willRetry: true` is not a refusal**: the server said it would try again,
/// so the turn may still succeed, and a `quota` verdict for a run that worked is
/// the opposite of what the caller asked for. Only a positive `true`
/// disqualifies the frame, because an absent field states no intention.
/// **`turn/completed` is emitted for a turn that succeeded too**, so its status
/// is read rather than assumed — which is also what lets a retry that then
/// failed on the limit still classify.
fn codex_app_server_turn_error(value: &Value) -> Option<&Value> {
    let params = value.get("params")?;
    match value.get("method").and_then(Value::as_str)? {
        "error" => {
            (params.get("willRetry").and_then(Value::as_bool) != Some(true)).then_some(())?;
            params.get("error")
        }
        "turn/completed" => {
            let turn = params.get("turn")?;
            (turn.get("status").and_then(Value::as_str) == Some("failed")).then_some(())?;
            turn.get("error")
        }
        _ => None,
    }
}

/// Whether a codex error object says the account is out of usage.
///
/// The machine-readable [`CODEX_USAGE_LIMIT_INFO`] code is the signal wherever
/// the frame carries it, and it is matched **exactly**: a false positive here
/// silently re-runs a paid task on another account, so a frame that names some
/// *other* cause is not a rejection this chain may fall through, whatever prose
/// it happens to quote. The phrasing is read only where there is no code to
/// read — the one reading `codex exec`'s own `turn.failed` offers.
fn codex_usage_limit_error(error: &Value) -> bool {
    match error.get("codexErrorInfo").and_then(Value::as_str) {
        Some(info) => info == CODEX_USAGE_LIMIT_INFO,
        None => error
            .get("message")
            .and_then(Value::as_str)
            .is_some_and(codex_usage_limit),
    }
}

/// Codex's machine-readable exhausted-usage code, as its app-server spells it.
const CODEX_USAGE_LIMIT_INFO: &str = "usageLimitExceeded";

/// Codex's usage-limit signature, matched on the stable phrasing only: the reset
/// date and the purchase URL in the real message both vary.
fn codex_usage_limit(text: &str) -> bool {
    let text = text.to_lowercase();
    [
        "hit your usage limit",
        "reached your usage limit",
        "usage limit reached",
        "usage limit exceeded",
    ]
    .iter()
    .any(|needle| text.contains(needle))
}

/// Claude's subscription-limit phrasing — the fast path over the text surfaces
/// [`zero_work_rate_limit_rejection`] cannot read (a bare limit line on stderr, a
/// limit reported without a status code). Two families:
///
/// - `hit your <qualifier> limit`, with the qualifier matched as a **slot**
///   rather than enumerated. That word is the one that keeps moving — the list
///   was one wording short for `session limit` (#1211) and again for `weekly
///   limit` — and matching the frame instead of every fill costs nothing
///   and survives the next rename. The apostrophe variants fall out for free:
///   `you've`/`you’ve`/`you have` all end at the same two words.
/// - the completed-sentence `… limit reached` forms, still enumerated on
///   purpose: widening the word before `limit` here would swallow `rate limit
///   reached`, a different and deliberately non-fall-through condition.
fn claude_subscription_limit(text: &str) -> bool {
    let text = text.to_lowercase();
    hit_your_limit(&text)
        || ["weekly limit reached", "usage limit reached"]
            .iter()
            .any(|needle| text.contains(needle))
}

/// Whether `text` (already lowercased) contains `hit your` as whole words with
/// `limit` at most one qualifier word later — so `hit your limit` and `hit your
/// weekly limit` match while an unrelated sentence that merely mentions both
/// cannot match across the gap.
///
/// **Both edges of the needle need a word boundary**, or a longer word spelling
/// it would match: `prohibit your limit` on the left, `hit yourself a limit` on
/// the right. A false positive here reads a genuine task failure as a quota
/// rejection, which silently re-runs the task on another account — the hazard
/// [`codex_turn_failure`] guards against too.
///
/// A boundary is any **non-alphanumeric** character, the same definition the
/// qualifier split uses, because callers pass a serialized JSON record as often
/// as a printed line: there the message ends `limit","total_cost_usd":0` with no
/// space after it, and whitespace splitting would fuse the qualifier to the rest
/// of the document.
fn hit_your_limit(text: &str) -> bool {
    text.match_indices("hit your").any(|(at, needle)| {
        let rest = &text[at + needle.len()..];
        !text[..at]
            .chars()
            .next_back()
            .is_some_and(char::is_alphanumeric)
            && !rest.chars().next().is_some_and(char::is_alphanumeric)
            && rest
                .split(|c: char| !c.is_alphanumeric())
                .filter(|word| !word.is_empty())
                .take(2)
                .any(|word| word == "limit")
    })
}

/// Match the first known failure signal in `text` (case-insensitive). Ordered
/// most-specific first so a 429 reads as `rate_limit`, not `auth`.
fn match_failure(text: &str) -> Option<FailureKind> {
    let h = text.to_lowercase();
    const SIGNALS: &[(FailureKind, &[&str])] = &[
        (
            FailureKind::ModelNotFound,
            &[
                "model not found",
                "model_not_found",
                "unknown model",
                "no such model",
                "invalid model",
                "is not a valid model",
            ],
        ),
        (
            FailureKind::RateLimit,
            &[
                "rate limit",
                "rate-limit",
                "ratelimit",
                "too many requests",
                "429",
            ],
        ),
        (
            FailureKind::Quota,
            &[
                "insufficient_quota",
                "quota",
                "credit balance",
                "out of credits",
                "billing",
            ],
        ),
        (
            FailureKind::Auth,
            &[
                "unauthorized",
                "authentication",
                "not authenticated",
                "invalid api key",
                "missing api key",
                "no api key",
                "credentials",
                "forbidden",
                "401",
                "403",
            ],
        ),
    ];
    SIGNALS
        .iter()
        .find(|(_, needles)| needles.iter().any(|n| h.contains(n)))
        .map(|(kind, _)| *kind)
}

/// Best-effort detection of a **deferred-tool dead-end** in `stdout`: a run that
/// ended a turn by deferring a builtin tool call rather than executing it, so it
/// completed with an empty result and no useful work done. This is distinct from
/// [`classify_failure`] because a deferred run exits *cleanly* (status `ok`), so
/// it needs its own detection independent of the exit code.
///
/// Two equivalent signals of the same Claude Code shape (bridged/managed
/// deployments), scanned terminal-event-first so a stream's final event wins:
/// a `stop_reason`/`terminal_reason` of `"tool_deferred"`, or a non-empty
/// `deferred_tool_use` object alongside an empty/absent `result`. The tool name
/// is lifted from `deferred_tool_use.name` when present, never fabricated.
/// `None` when neither signal is found (the ordinary case for every harness).
pub fn detect_deferred_tool(stdout: &str) -> Option<DeferredTool> {
    json_candidates(stdout)
        .iter()
        .rev()
        .find_map(deferred_from_object)
}

/// Detect the deferred-tool shape in one JSON object. `None` unless the object
/// carries a `"tool_deferred"` reason or a `deferred_tool_use` with an empty
/// result — so a normal `result` document (which has neither) never matches.
fn deferred_from_object(value: &Value) -> Option<DeferredTool> {
    let obj = value.as_object()?;
    let reason_deferred = ["stop_reason", "terminal_reason"]
        .iter()
        .any(|k| obj.get(*k).and_then(Value::as_str) == Some("tool_deferred"));
    let deferred_use = obj.get("deferred_tool_use").and_then(Value::as_object);
    // A `deferred_tool_use` present with an empty/absent `result` is the same
    // dead-end even when the reason field is absent; a normal answer carries a
    // non-empty `result` and no `deferred_tool_use`, so it never trips this.
    let result_empty = obj
        .get("result")
        .and_then(Value::as_str)
        .is_none_or(str::is_empty);
    if !(reason_deferred || (deferred_use.is_some() && result_empty)) {
        return None;
    }
    let tool = deferred_use
        .and_then(|d| d.get("name"))
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    Some(DeferredTool { tool })
}

/// Candidate JSON objects in `stdout`: the whole document when it parses, else
/// each parseable line (stream-json). In document order, so a caller can prefer
/// the terminal event by scanning from the end.
fn json_candidates(stdout: &str) -> Vec<Value> {
    let trimmed = stdout.trim();
    if trimmed.is_empty() {
        return Vec::new();
    }
    if let Ok(value) = serde_json::from_str::<Value>(trimmed) {
        return vec![value];
    }
    stdout
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line.trim()).ok())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn usage_from_claude_shaped_json() {
        let raw = r#"{"type":"result","result":"hi","session_id":"abc","total_cost_usd":0.0095,
            "usage":{"input_tokens":1234,"output_tokens":56,"cache_read_input_tokens":7,
            "cache_creation_input_tokens":89}}"#;
        let got = extract_usage(raw).unwrap();
        assert_eq!(got.usage.input_tokens, Some(1234));
        assert_eq!(got.usage.output_tokens, Some(56));
        assert_eq!(got.usage.cache_read_tokens, Some(7));
        assert_eq!(got.usage.cache_write_tokens, Some(89));
        assert_eq!(got.usage.cost_usd, Some(0.0095));
        assert_eq!(got.source, "json");
    }

    #[test]
    fn usage_cache_only_counts_as_usage() {
        // A run that reports only cache tokens (no fresh input/output, no cost) is
        // still a real reading — cache fields count toward "has usage".
        let raw = r#"{"type":"result","usage":{"cache_read_input_tokens":42}}"#;
        let got = extract_usage(raw).unwrap();
        assert_eq!(got.usage.cache_read_tokens, Some(42));
        assert_eq!(got.usage.input_tokens, None);
        assert_eq!(got.usage.cache_write_tokens, None);
    }

    #[test]
    fn usage_without_cache_fields_yields_none_cache() {
        // Harnesses/shapes that don't report cache counts leave the cache fields
        // null — never fabricated as 0.
        let raw = r#"{"type":"result","usage":{"input_tokens":9,"output_tokens":3}}"#;
        let got = extract_usage(raw).unwrap();
        assert_eq!(got.usage.input_tokens, Some(9));
        assert_eq!(got.usage.cache_read_tokens, None);
        assert_eq!(got.usage.cache_write_tokens, None);
    }

    #[test]
    fn codex_turn_usage_and_versioned_price_are_populated_without_guessing_unknown_models() {
        let raw = r#"{"type":"turn.completed","usage":{"input_tokens":1000,"cached_input_tokens":400,"output_tokens":100}}"#;
        let mut known = extract_usage(raw).unwrap().usage;
        apply_model_price(&mut known, "codex", Some("gpt-5-codex"));
        assert_eq!(known.input_tokens, Some(1000));
        assert_eq!(known.cache_read_tokens, Some(400));
        assert!((known.cost_usd.unwrap() - 0.0018).abs() < 1e-12);

        let mut unknown = extract_usage(raw).unwrap().usage;
        apply_model_price(&mut unknown, "codex", Some("provider-alias"));
        assert_eq!(unknown.cost_usd, None);

        let mut other_provider = extract_usage(raw).unwrap().usage;
        apply_model_price(&mut other_provider, "claude-code", Some("gpt-5-codex"));
        assert_eq!(other_provider.cost_usd, None);

        let mut native = extract_usage(raw).unwrap().usage;
        native.cost_usd = Some(0.42);
        apply_model_price(&mut native, "codex", Some("gpt-5-codex"));
        assert_eq!(native.cost_usd, Some(0.42));
    }

    #[test]
    fn malformed_usage_values_are_unknown_not_zero() {
        let got = extract_usage(
            r#"{"type":"turn.completed","usage":{"input_tokens":-1,"output_tokens":"4","cached_input_tokens":null}}"#,
        );
        assert!(got.is_none());
    }

    #[test]
    fn usage_cost_only_still_reported() {
        // Tokens absent but a cost present (or vice versa) is a non-empty reading.
        let got = extract_usage(r#"{"cost_usd":0.5}"#).unwrap();
        assert_eq!(got.usage.cost_usd, Some(0.5));
        assert_eq!(got.usage.input_tokens, None);
    }

    #[test]
    fn usage_absent_yields_none() {
        assert!(extract_usage(r#"{"result":"hi"}"#).is_none());
        assert!(extract_usage("not json").is_none());
        assert!(extract_usage("").is_none());
    }

    #[test]
    fn usage_prefers_terminal_stream_event() {
        let raw = concat!(
            "{\"type\":\"system\",\"session_id\":\"s\"}\n",
            "{\"type\":\"result\",\"usage\":{\"input_tokens\":9},\"total_cost_usd\":0.01}\n",
        );
        let got = extract_usage(raw).unwrap();
        assert_eq!(got.usage.input_tokens, Some(9));
        assert_eq!(got.usage.cost_usd, Some(0.01));
    }

    #[test]
    fn session_id_extracted_when_present() {
        assert_eq!(
            extract_session(r#"{"session_id":"sess-123","result":"x"}"#),
            Some("sess-123".to_string())
        );
        assert_eq!(extract_session(r#"{"result":"x"}"#), None);
        assert_eq!(extract_session(r#"{"session_id":""}"#), None);
    }

    #[test]
    fn session_id_read_from_camelcase_and_codex_thread_id() {
        // OpenCode's camelCase handle.
        assert_eq!(
            extract_session(r#"{"sessionID":"ses_abc","type":"text"}"#),
            Some("ses_abc".to_string())
        );
        // Codex's `thread.started` event under `--json` carries `thread_id`.
        assert_eq!(
            extract_session(r#"{"type":"thread.started","thread_id":"0199-xyz"}"#),
            Some("0199-xyz".to_string())
        );
    }

    #[test]
    fn usage_summed_across_opencode_step_finish_events() {
        // OpenCode JSONL: per-step tokens/cost under `part`; the run total is the
        // sum, and the method is recorded distinctly from the single-event shape.
        let raw = concat!(
            "{\"type\":\"step_start\",\"sessionID\":\"ses_1\",\"part\":{}}\n",
            "{\"type\":\"step_finish\",\"sessionID\":\"ses_1\",\"part\":{\"cost\":0.001,\
             \"tokens\":{\"input\":671,\"output\":8,\"reasoning\":0,\
             \"cache\":{\"read\":21415,\"write\":100}}}}\n",
            "{\"type\":\"step_finish\",\"sessionID\":\"ses_1\",\"part\":{\"cost\":0.002,\
             \"tokens\":{\"input\":12,\"output\":34,\"cache\":{\"read\":5,\"write\":3}}}}\n",
        );
        let got = extract_usage(raw).unwrap();
        assert_eq!(got.usage.input_tokens, Some(683));
        assert_eq!(got.usage.output_tokens, Some(42));
        assert_eq!(got.usage.cache_read_tokens, Some(21420));
        assert_eq!(got.usage.cache_write_tokens, Some(103));
        assert!((got.usage.cost_usd.unwrap() - 0.003).abs() < 1e-9);
        assert_eq!(got.source, "json:summed-steps");
    }

    #[test]
    fn session_from_opencode_camelcase_sessionid() {
        let raw = r#"{"type":"step_start","sessionID":"ses_494719016ffe85","part":{}}"#;
        assert_eq!(extract_session(raw), Some("ses_494719016ffe85".to_string()));
    }

    #[test]
    fn cursor_session_from_stream_json_lines_without_usage() {
        // cursor-agent stream-json: snake_case session_id in NDJSON events; it does
        // not emit token usage today, so usage is honestly absent (not fabricated).
        let raw = concat!(
            "{\"type\":\"system\",\"subtype\":\"init\",\
             \"session_id\":\"11111111-2222-3333-4444-555555555555\",\"model\":\"x\"}\n",
            "{\"type\":\"result\",\"subtype\":\"success\",\"is_error\":false,\
             \"result\":\"pong\",\"session_id\":\"11111111-2222-3333-4444-555555555555\"}\n",
        );
        assert_eq!(
            extract_session(raw),
            Some("11111111-2222-3333-4444-555555555555".to_string())
        );
        assert!(extract_usage(raw).is_none());
    }

    #[test]
    fn classify_distinguishes_common_failures() {
        assert_eq!(
            classify_failure("", "Error: 401 Unauthorized")
                .unwrap()
                .kind,
            FailureKind::Auth
        );
        assert_eq!(
            classify_failure("", "HTTP 429: rate limit exceeded")
                .unwrap()
                .kind,
            FailureKind::RateLimit
        );
        assert_eq!(
            classify_failure("", "model not found: gpt-9").unwrap().kind,
            FailureKind::ModelNotFound
        );
        assert_eq!(
            classify_failure("", "insufficient_quota; check billing")
                .unwrap()
                .kind,
            FailureKind::Quota
        );
    }

    #[test]
    fn failure_kind_serializes_to_its_snake_case_token() {
        // The wire contract: the enum must serialize to exactly the strings a
        // consumer reads in `failure_kind`, and `as_str` must agree with serde.
        for kind in FailureKind::ALL {
            let json = serde_json::to_string(&kind).unwrap();
            assert_eq!(json, format!("\"{}\"", kind.as_str()));
        }
        assert_eq!(FailureKind::ToolDeferred.as_str(), "tool_deferred");
        assert_eq!(FailureKind::SessionNotFound.as_str(), "session_not_found");
        assert_eq!(
            FailureKind::UntrustedDirectory.as_str(),
            "untrusted_directory"
        );
        assert_eq!(FailureKind::InputTooLarge.as_str(), "input_too_large");
        assert_eq!(FailureKind::ModelMismatch.as_str(), "model_mismatch");
    }

    #[test]
    fn an_unknown_session_refusal_classifies_as_session_not_found() {
        // Every phrase is a real capture: each installed CLI was asked to resume
        // a session id it had never minted. All four exit non-zero with an empty
        // stdout, so the reading has to come from stderr.
        for (dialect, stderr) in [
            (
                FailureDialect::ClaudeCode,
                "No conversation found with session ID: 00000000-0000-0000-0000-000000000000",
            ),
            (
                FailureDialect::Codex,
                "Error: thread/resume: thread/resume failed: no rollout found for thread id \
                 00000000-0000-0000-0000-000000000000 (code -32600)",
            ),
            (FailureDialect::Generic, "Error: Session not found"),
            (
                FailureDialect::Generic,
                "No saved session found with title \"00000000-0000-0000-0000-000000000000\".",
            ),
        ] {
            let got = classify_harness_failure(dialect, "", stderr)
                .unwrap_or_else(|| panic!("unclassified: {stderr}"));
            assert_eq!(got.kind, FailureKind::SessionNotFound, "{stderr}");
            assert_eq!(got.source, "stderr", "{stderr}");
        }
    }

    #[test]
    fn a_precondition_refusal_is_classified_and_keeps_the_providers_own_code() {
        // Both captures are verbatim from the real CLI. Codex refuses an
        // untrusted cwd within a few hundred milliseconds and an over-long input
        // before the model is called; each exits non-zero with an empty stdout,
        // so the reading comes from stderr.
        let trust = classify_harness_failure(
            FailureDialect::Codex,
            "",
            "Not inside a trusted directory and --skip-git-repo-check was not specified",
        )
        .expect("the trust refusal must be classified");
        assert_eq!(trust.kind, FailureKind::UntrustedDirectory);
        assert_eq!(trust.source, "stderr");
        // Prose only — nothing machine-readable is invented for it.
        assert_eq!(trust.detail, None);

        let over = classify_harness_failure(
            FailureDialect::Codex,
            "",
            "turn/start failed: Input exceeds the maximum length of 1048576 characters. \
             (code -32602), data: {\"input_error_code\":\"input_too_large\",\
             \"max_chars\":1048576,\"actual_chars\":1168716}",
        )
        .expect("the input-size refusal must be classified");
        assert_eq!(over.kind, FailureKind::InputTooLarge);
        // The provider's object, exactly as it wrote it: a caller shards against
        // `max_chars`, so a paraphrase would be oneharness inventing the numbers.
        assert_eq!(
            over.detail.as_deref(),
            Some(
                "{\"input_error_code\":\"input_too_large\",\"max_chars\":1048576,\
                 \"actual_chars\":1168716}"
            )
        );
    }

    #[test]
    fn an_input_size_refusal_without_a_data_object_is_still_classified() {
        // The sentence alone still says the request was refused unmade; the
        // detail is absent rather than guessed at.
        let got = classify_harness_failure(
            FailureDialect::Codex,
            "",
            "Error: Input exceeds the maximum length of 1048576 characters.",
        )
        .expect("the prose alone classifies");
        assert_eq!(got.kind, FailureKind::InputTooLarge);
        assert_eq!(got.detail, None);
        // ...and neither a data object that is not parseable JSON, nor one whose
        // code is some *other* refusal, yields a detail: the cause carried up is
        // the one that classified this failure, never a span that merely sits
        // near the key.
        for text in [
            "data: {\"input_error_code\":\"input_too_large\", \"max_chars\": }",
            "Input exceeds the maximum length. data: {\"input_error_code\":\"input_unreadable\"}",
        ] {
            let ragged = classify_harness_failure(FailureDialect::Codex, "", text)
                .unwrap_or_else(|| panic!("unclassified: {text}"));
            assert_eq!(ragged.kind, FailureKind::InputTooLarge, "{text}");
            assert_eq!(ragged.detail, None, "{text}");
        }
    }

    #[test]
    fn an_ordinary_failure_is_not_read_as_a_precondition_refusal() {
        // The phrases are specific on purpose: a run that merely talks about
        // directories or input size stays unclassified, so the chain stops at it
        // as the real failure it is.
        for stderr in [
            "the directory is not writable",
            "trusted certificate missing",
            "input file too large for the editor",
        ] {
            assert_eq!(
                classify_harness_failure(FailureDialect::Codex, "", stderr),
                None,
                "{stderr} must not read as a precondition refusal"
            );
        }
    }

    #[test]
    fn a_quota_rejection_still_outranks_an_unknown_session_phrase() {
        // Specific-first order: an exhausted subscription that also mentions a
        // missing session must stay `quota`, the reading the whole chain's
        // provisioning logic keys on.
        let got = classify_harness_failure(
            FailureDialect::ClaudeCode,
            "",
            "You've hit your session limit. Session not found",
        )
        .unwrap();
        assert_eq!(got.kind, FailureKind::Quota);
    }

    #[test]
    fn an_ordinary_failure_is_not_read_as_a_missing_session() {
        // The phrases are specific on purpose: a run that merely mentions a
        // session stays unclassified, so the chain stops at it as a real failure.
        for stderr in [
            "the session ended",
            "no conversation history",
            "file not found",
        ] {
            assert!(
                classify_harness_failure(FailureDialect::Generic, "", stderr).is_none(),
                "{stderr} must not read as a missing session"
            );
        }
    }

    #[test]
    fn classify_records_source_and_prefers_stderr() {
        let got = classify_failure("rate limit in stdout", "unauthorized in stderr").unwrap();
        assert_eq!(got.kind, FailureKind::Auth);
        assert_eq!(got.source, "stderr");
        let got = classify_failure("model not found", "").unwrap();
        assert_eq!(got.source, "stdout");
    }

    #[test]
    fn classify_none_when_no_signal() {
        assert!(classify_failure("just some output", "a normal error").is_none());
    }

    /// The capture from issue #1290: an empty Claude config directory answers
    /// `Not logged in · Please run /login` in a zero-token terminal record. Not
    /// one of those words is in the generic vocabulary, so the reading fell to
    /// whatever else the same failed run said — here the record's own
    /// `"duration_ms":429`, which the coarse `429` needle reads as a rate
    /// limit. The refusal is what the record is about, so it decides.
    #[test]
    fn a_claude_login_refusal_is_auth_whatever_else_the_failed_run_says() {
        let capture = include_str!("../../../../tests/fixtures/claude-not-logged-in.jsonl");

        // The record path first, because that is the one a real run takes: the
        // harness declared the failure itself (`is_error`), so this reading is
        // taken before any text scan.
        let got = detect_harness_provider_failure(FailureDialect::ClaudeCode, capture)
            .expect("the login refusal must be classified");
        assert_eq!(got.kind, FailureKind::Auth);
        assert_eq!(got.source, "stdout");

        // The text path, with unrelated warning text on the stream it scans
        // *first*: the refusal still decides, rather than the neighbouring
        // words winning by being read sooner.
        let got = classify_harness_failure(
            FailureDialect::ClaudeCode,
            capture,
            "Warning: this account is approaching its rate limit.",
        )
        .expect("the login refusal must be classified");
        assert_eq!(got.kind, FailureKind::Auth);
        assert_eq!(got.source, "stdout");

        // ...and the bare-line surface, where there is no record to read at all.
        let line = include_str!("../../../../tests/fixtures/claude-not-logged-in.txt");
        let bare = classify_harness_failure(FailureDialect::ClaudeCode, "", line)
            .expect("the bare refusal classifies too");
        assert_eq!(bare.kind, FailureKind::Auth);
        assert_eq!(bare.source, "stderr");

        // Either half of the sentence classifies on its own, so a reworded half
        // does not strand the other.
        for half in ["Not logged in.", "Error: please run /login to continue."] {
            let got = classify_harness_failure(FailureDialect::ClaudeCode, "", half)
                .unwrap_or_else(|| panic!("unclassified: {half}"));
            assert_eq!(got.kind, FailureKind::Auth, "{half}");
        }
    }

    #[test]
    fn the_login_refusal_is_read_for_claude_only_and_not_for_another_harness() {
        let capture = include_str!("../../../../tests/fixtures/claude-not-logged-in.jsonl");
        // The same bytes under another harness's dialect classify exactly as
        // they did before #1290 — on the coarse `429` the duration happens to
        // contain — because `not logged in` is ordinary English about any
        // service an agent touches, not a sentence only this CLI prints.
        let got = detect_harness_provider_failure(FailureDialect::Generic, capture)
            .expect("the record still classifies generically");
        assert_eq!(got.kind, FailureKind::RateLimit);
        // And an unrelated CLI merely saying the words stays unclassified: the
        // honest default, which stops the chain at the real failure.
        assert!(classify_harness_failure(
            FailureDialect::Generic,
            "",
            "the deploy target reported: not logged in",
        )
        .is_none());
    }

    #[test]
    fn a_login_refusal_quoted_by_a_run_that_did_work_is_not_an_auth_refusal() {
        // A harness that spent tokens was logged in, so this sentence is one
        // the agent *wrote* mid-run rather than one it received. The refusal
        // reading is refuted by the harness's own accounting — the same rule a
        // mid-run limit follows — so the run stays unclassified and the chain
        // stops at it.
        let worked = r#"{"type":"result","subtype":"error_during_execution","is_error":true,"result":"the deploy tool answered Not logged in · Please run /login, so the task did not finish","usage":{"input_tokens":4102,"output_tokens":311}}"#;
        assert_eq!(
            detect_harness_provider_failure(FailureDialect::ClaudeCode, worked),
            None
        );
        assert_eq!(
            classify_harness_failure(FailureDialect::ClaudeCode, worked, ""),
            None
        );
    }

    #[test]
    fn claude_subscription_limit_fixtures_classify_as_quota_when_no_work_was_done() {
        // Both zero-work captures: all-zero token counts (the JSON), and a bare
        // limit line with no accounting at all (the stderr text). Absent
        // accounting is not evidence of work, so both stay fall-through.
        let session_json = include_str!("../../../../tests/fixtures/claude-session-limit.json");
        let got =
            detect_harness_provider_failure(FailureDialect::ClaudeCode, session_json).unwrap();
        assert_eq!(got.kind, FailureKind::Quota);
        assert_eq!(got.source, "stdout");

        let session_text = include_str!("../../../../tests/fixtures/claude-session-limit.txt");
        let got = classify_harness_failure(FailureDialect::ClaudeCode, "", session_text).unwrap();
        assert_eq!(got.kind, FailureKind::Quota);
        assert_eq!(got.source, "stderr");
    }

    /// The weekly-limit capture is the counter-case, and it is a **deliberate
    /// change** from when that fixture was first pinned: it reports 17 input +
    /// 800 output tokens, 78k cached prompt tokens, $0.147, and ten turns over 26
    /// seconds. The limit landed mid-run, so the harness ran — `quota` would hand
    /// that task to the next candidate and pay for the same work twice. It stays
    /// unclassified, which stops the chain.
    #[test]
    fn a_claude_limit_that_landed_after_real_work_is_not_a_quota_rejection() {
        let weekly_json = include_str!("../../../../tests/fixtures/claude-weekly-limit.json");
        assert!(detect_harness_provider_failure(FailureDialect::ClaudeCode, weekly_json).is_none());
        assert!(classify_harness_failure(FailureDialect::ClaudeCode, weekly_json, "").is_none());

        // Same limit message, same spent tokens, but the record also carries the
        // rejection's 429: the generic vocabulary gets its turn and reads it as
        // the transient `rate_limit` — which also stops the chain.
        let mid_run = r#"{"type":"result","is_error":true,"api_error_status":429,"result":"You've hit your session limit · resets 1pm","usage":{"input_tokens":4102,"output_tokens":311}}"#;
        assert_eq!(
            detect_harness_provider_failure(FailureDialect::ClaudeCode, mid_run)
                .unwrap()
                .kind,
            FailureKind::RateLimit
        );
        assert_eq!(
            classify_harness_failure(FailureDialect::ClaudeCode, mid_run, "")
                .unwrap()
                .kind,
            FailureKind::RateLimit
        );
    }

    #[test]
    fn work_is_any_spend_the_harness_reports_and_never_an_absent_reading() {
        // Each accounting field on its own is proof the harness got somewhere,
        // including a per-model map with no token totals beside it.
        for spent in [
            r#""usage":{"input_tokens":12,"output_tokens":0}"#,
            r#""usage":{"output_tokens":7}"#,
            r#""usage":{"cache_read_input_tokens":9001}"#,
            r#""usage":{"cache_creation_input_tokens":64}"#,
            r#""total_cost_usd":0.0004"#,
            r#""modelUsage":{"claude-opus-4-6":{"inputTokens":31}}"#,
        ] {
            let record = format!(
                r#"{{"type":"result","is_error":true,{spent},"result":"You've hit your session limit"}}"#
            );
            assert!(
                detect_harness_provider_failure(FailureDialect::ClaudeCode, &record).is_none(),
                "{spent} is work, so the limit must not read as quota"
            );
        }
        // The zero-work counterparts of the same fields, plus the empty
        // `modelUsage` map the real capture carries.
        let idle = r#"{"type":"result","is_error":true,"total_cost_usd":0,"usage":{"input_tokens":0,"output_tokens":0,"cache_read_input_tokens":0,"cache_creation_input_tokens":0},"modelUsage":{},"result":"You've hit your session limit"}"#;
        assert_eq!(
            detect_harness_provider_failure(FailureDialect::ClaudeCode, idle)
                .unwrap()
                .kind,
            FailureKind::Quota
        );
    }

    #[test]
    fn claude_limit_language_is_adapter_scoped_and_requires_a_failure_surface() {
        let captured = include_str!("../../../../tests/fixtures/claude-session-limit.json");
        assert!(detect_harness_provider_failure(FailureDialect::Generic, captured).is_none());
        assert!(classify_harness_failure(
            FailureDialect::Generic,
            "",
            "tool crashed while calculating a session limit"
        )
        .is_none());
        assert!(detect_harness_provider_failure(
            FailureDialect::ClaudeCode,
            r#"{"type":"result","is_error":false,"result":"You've hit your session limit"}"#
        )
        .is_none());
    }

    /// The captured record from issue #1211: a session-limit rejection that did
    /// no work at all. It carries `subtype: "success"` with no `is_error`, and
    /// declares the rejection through `terminal_reason` / `api_error_status`
    /// instead — so both the envelope gate and the classification order have to
    /// be right for it to read as quota rather than a transient `rate_limit`.
    #[test]
    fn claude_session_limit_reported_as_an_api_error_is_quota_not_rate_limit() {
        let captured =
            include_str!("../../../../tests/fixtures/claude-session-limit-api-error.json");
        // The record declares itself an API failure without `is_error`.
        let got = detect_harness_provider_failure(FailureDialect::ClaudeCode, captured).unwrap();
        assert_eq!(got.kind, FailureKind::Quota);
        assert_eq!(got.source, "stdout");
        // The non-zero-exit path reads the same bytes the same way: the embedded
        // `"api_error_status":429` must not out-rank the limit message.
        let got = classify_harness_failure(FailureDialect::ClaudeCode, captured, "").unwrap();
        assert_eq!(got.kind, FailureKind::Quota);
        assert_eq!(got.source, "stdout");
    }

    /// The captured weekly-limit record: the same zero-work shape as the
    /// session-limit capture, one word different (`weekly`), and that word alone
    /// used to be the difference between a fallback chain routing around an
    /// exhausted subscription and dying on it.
    ///
    /// Both readings of the same bytes have to agree, because which one runs
    /// depends only on whether the harness happened to exit non-zero: the record
    /// is read structurally (a zero-work `429`), and the text is read by the
    /// qualifier-slot phrase match.
    #[test]
    fn claude_weekly_limit_reported_as_an_api_error_is_quota_not_rate_limit() {
        let captured =
            include_str!("../../../../tests/fixtures/claude-weekly-limit-api-error.json");
        let got = detect_harness_provider_failure(FailureDialect::ClaudeCode, captured).unwrap();
        assert_eq!(got.kind, FailureKind::Quota);
        assert_eq!(got.source, "stdout");
        let got = classify_harness_failure(FailureDialect::ClaudeCode, captured, "").unwrap();
        assert_eq!(got.kind, FailureKind::Quota);
        assert_eq!(got.source, "stdout");
    }

    /// The structural rule, on prose no phrase list could have anticipated: a
    /// rejection is classified from *what the provider did* (`429`) and *whether
    /// the candidate got anywhere*, so a rewording cannot strand a chain again.
    #[test]
    fn a_zero_work_429_is_quota_whatever_its_prose_says() {
        for prose in [
            "You've hit your monthly limit · resets Sep 1",
            "Usage limit for this plan has been reached",
            "",
        ] {
            let record = serde_json::json!({
                "type": "result",
                "subtype": "success",
                "api_error_status": 429,
                "usage": {"input_tokens": 0, "output_tokens": 0},
                "result": prose,
            })
            .to_string();
            assert_eq!(
                detect_harness_provider_failure(FailureDialect::ClaudeCode, &record)
                    .unwrap()
                    .kind,
                FailureKind::Quota,
                "{prose:?} did no work, so the chain must try the next candidate"
            );
        }
    }

    /// A real Claude Code 2.1.308 invalid-key answer (`ANTHROPIC_API_KEY` set to
    /// a non-key), whose random UUIDs and timings sometimes contain `429`. It is
    /// `auth` either way: verbatim, and with its `duration_ms` landing on the
    /// digits the coarse rate-limit needle reads.
    #[test]
    fn an_invalid_key_is_auth_whatever_digits_its_record_carries() {
        let capture = include_str!("../../../../tests/fixtures/claude-invalid-api-key.json");
        let unlucky = capture.replace(r#""duration_ms":272"#, r#""duration_ms":429"#);
        assert_ne!(unlucky, capture, "the fixture's duration must be rewritten");
        for record in [capture, unlucky.as_str()] {
            let got = detect_harness_provider_failure(FailureDialect::ClaudeCode, record)
                .expect("the invalid key must be classified");
            assert_eq!(got.kind, FailureKind::Auth, "{record}");
            assert_eq!(got.source, "stdout");
        }
        let forbidden = r#"{"type":"result","is_error":true,"api_error_status":403,"duration_ms":1429,"result":"Request not allowed"}"#;
        assert_eq!(
            detect_harness_provider_failure(FailureDialect::Generic, forbidden)
                .unwrap()
                .kind,
            FailureKind::Auth
        );
    }

    /// The discriminator that keeps the structural rule safe, and its two edges.
    /// A `429` that spent tokens is a run, so it keeps the non-fall-through
    /// `rate_limit`; a zero-work rejection that is *not* a `429` is not routed
    /// around either, because another identity cannot serve a provider fault.
    #[test]
    fn the_structural_rule_needs_both_a_429_and_no_work() {
        let worked = r#"{"type":"result","is_error":true,"api_error_status":429,"usage":{"input_tokens":4102,"output_tokens":311},"result":"Something went wrong"}"#;
        assert_eq!(
            detect_harness_provider_failure(FailureDialect::ClaudeCode, worked)
                .unwrap()
                .kind,
            FailureKind::RateLimit
        );
        let server_fault = r#"{"type":"result","is_error":true,"api_error_status":500,"usage":{"input_tokens":0,"output_tokens":0},"result":"Internal server error"}"#;
        assert!(
            detect_harness_provider_failure(FailureDialect::ClaudeCode, server_fault).is_none(),
            "a zero-work 500 is a provider fault the next candidate would hit too"
        );
        // A generic rate limit with no status code of its own is unchanged: the
        // structural rule reads the record's declaration, not its wording.
        let text_only =
            r#"{"type":"result","is_error":true,"result":"429 rate limit, please retry"}"#;
        assert_eq!(
            detect_harness_provider_failure(FailureDialect::ClaudeCode, text_only)
                .unwrap()
                .kind,
            FailureKind::RateLimit
        );
    }

    /// The qualifier slot, on the text surface that has no record to read: any
    /// one word between `hit your` and `limit` is the same rejection, and the
    /// window is narrow enough that a sentence merely containing both words is
    /// not one.
    #[test]
    fn the_limit_phrase_matches_the_frame_not_the_qualifier() {
        for line in [
            "You've hit your weekly limit · resets Aug 6, 7am",
            "You’ve hit your monthly limit",
            "You have hit your limit.",
        ] {
            assert_eq!(
                classify_harness_failure(FailureDialect::ClaudeCode, "", line)
                    .unwrap()
                    .kind,
                FailureKind::Quota,
                "{line:?}"
            );
        }
        assert!(
            classify_harness_failure(
                FailureDialect::ClaudeCode,
                "",
                "the tests you hit your target on exceeded the configured limit"
            )
            .is_none(),
            "the qualifier slot is one word wide, not a whole sentence"
        );
    }

    /// The frame is matched as *words*: a longer word that merely spells `hit`
    /// or `your` is not the phrase. Getting this wrong is not cosmetic — it
    /// reads an ordinary task failure as a quota rejection, and a fallback chain
    /// then re-runs the task on another identity for nothing.
    #[test]
    fn the_limit_phrase_requires_a_word_boundary_around_the_frame() {
        for line in [
            // `hit` as the tail of a longer word, on the left.
            "the license prohibit your limit clause",
            "not a whit your limit changed",
            // `your` as the head of a longer word, on the right: the qualifier
            // slot must not start mid-word.
            "you hit yourself: limit reached",
        ] {
            assert!(
                classify_harness_failure(FailureDialect::ClaudeCode, "", line).is_none(),
                "{line:?} does not say a limit was hit"
            );
        }
        // The true-positive path is unchanged, including the serialized-JSON
        // surface where the message ends with no trailing space.
        for text in [
            "You've hit your limit.",
            "You've hit your weekly limit · resets Aug 6, 7am",
            r#"{"type":"result","result":"You've hit your limit","total_cost_usd":0}"#,
            r#"{"type":"result","result":"You've hit your weekly limit","total_cost_usd":0}"#,
        ] {
            assert_eq!(
                classify_harness_failure(FailureDialect::ClaudeCode, "", text)
                    .unwrap()
                    .kind,
                FailureKind::Quota,
                "{text:?}"
            );
        }
    }

    #[test]
    fn adapter_quota_signal_outranks_an_embedded_rate_limit_status() {
        // Same precedence, in the `is_error` envelope the earlier fixtures use:
        // the specific reading of the record wins over the generic `429` scan.
        let record = r#"{"type":"result","is_error":true,"api_error_status":429,"result":"You've hit your session limit · resets 1pm"}"#;
        assert_eq!(
            detect_harness_provider_failure(FailureDialect::ClaudeCode, record)
                .unwrap()
                .kind,
            FailureKind::Quota
        );
        // A 429 with no limit message reads as `quota` too, and this expectation
        // is a **deliberate reversal**: it used to stay the transient,
        // non-fall-through `rate_limit`. Nothing in this record says the harness
        // got anywhere, and the phrase list that was supposed to catch the ones
        // that did was one wording short twice. See
        // `zero_work_rate_limit_rejection` for why the safe reading wins.
        let plain = r#"{"type":"result","is_error":true,"api_error_status":429,"result":"Rate limit exceeded"}"#;
        assert_eq!(
            detect_harness_provider_failure(FailureDialect::ClaudeCode, plain)
                .unwrap()
                .kind,
            FailureKind::Quota
        );
        // And stderr keeps beating stdout when only the generic signal matches.
        let got = classify_harness_failure(
            FailureDialect::ClaudeCode,
            "429 too many requests",
            "unauthorized",
        )
        .unwrap();
        assert_eq!(got.kind, FailureKind::Auth);
        assert_eq!(got.source, "stderr");
    }

    #[test]
    fn a_provider_failure_envelope_needs_only_one_of_its_three_declarations() {
        // Each declaration alone opens the record to classification, and a record
        // making none of them stays unclassified however it words its result.
        for envelope in [
            r#""is_error":true"#,
            r#""terminal_reason":"api_error""#,
            r#""api_error_status":503"#,
        ] {
            let record = format!(
                r#"{{"type":"result","subtype":"success",{envelope},"result":"insufficient_quota: credit balance exhausted"}}"#
            );
            assert_eq!(
                detect_provider_failure(&record).unwrap().kind,
                FailureKind::Quota,
                "{envelope}"
            );
        }
        assert!(detect_provider_failure(
            r#"{"type":"result","subtype":"success","terminal_reason":"end_turn","result":"insufficient_quota is the error you asked about"}"#
        )
        .is_none());
        // A non-numeric `api_error_status` is not a status — a transcript quoting
        // the field name must not become a failure envelope.
        assert!(detect_provider_failure(
            r#"{"type":"result","api_error_status":null,"result":"quota"}"#
        )
        .is_none());
    }

    #[test]
    fn codex_usage_limit_capture_classifies_as_quota() {
        // The real capture: the limit arrives as a `turn.failed` event on stdout,
        // after `turn.started` — never on stderr — so the event stream is the
        // only surface carrying it.
        let captured = include_str!("../../../../tests/fixtures/codex-usage-limit.jsonl");
        let got = detect_harness_provider_failure(FailureDialect::Codex, captured).unwrap();
        assert_eq!(got.kind, FailureKind::Quota);
        assert_eq!(got.source, "stdout");
        // The reset date and the purchase URL vary, so neither is part of the
        // signature: the phrasing alone still reads as exhausted.
        let other_reset = r#"{"type":"turn.failed","error":{"message":"You’ve hit your usage limit. Try again at Dec 31st, 2027 6:00 AM."}}"#;
        assert_eq!(
            detect_harness_provider_failure(FailureDialect::Codex, other_reset)
                .unwrap()
                .kind,
            FailureKind::Quota
        );
    }

    #[test]
    fn codex_ordinary_turn_failure_is_never_a_quota_rejection() {
        // A real task failure must stay unclassified: treating it as quota would
        // silently re-run the task on another account under `--run-mode fallback`.
        let captured = include_str!("../../../../tests/fixtures/codex-turn-failed.jsonl");
        assert!(detect_harness_provider_failure(FailureDialect::Codex, captured).is_none());
        assert!(classify_harness_failure(FailureDialect::Codex, captured, "").is_none());
        // Codex's limit phrasing is adapter-scoped, and only its own failure
        // event counts: an agent message quoting the limit is not a rejection.
        let limit = include_str!("../../../../tests/fixtures/codex-usage-limit.jsonl");
        assert!(detect_harness_provider_failure(FailureDialect::Generic, limit).is_none());
        assert!(classify_harness_failure(FailureDialect::Codex, limit, "").is_none());
        assert!(detect_harness_provider_failure(
            FailureDialect::Codex,
            r#"{"type":"item.completed","item":{"type":"agent_message","text":"You've hit your usage limit"}}"#
        )
        .is_none());
    }

    #[test]
    fn codex_app_server_usage_limit_capture_classifies_as_quota() {
        // The real capture from a `--control` turn: the app-server states the
        // limit twice — once as an `error` notification, once inside the
        // terminal `turn/completed` — and neither is the `codex exec` event
        // shape, so both frames have to be read here.
        let captured =
            include_str!("../../../../tests/fixtures/codex-app-server-usage-limit.jsonl");
        let got = detect_harness_provider_failure(FailureDialect::Codex, captured).unwrap();
        assert_eq!(got.kind, FailureKind::Quota);
        assert_eq!(got.source, "stdout");
        // Each frame classifies on its own: a transcript truncated to either one
        // still says the identity is exhausted.
        for frame in captured.lines() {
            assert_eq!(
                detect_harness_provider_failure(FailureDialect::Codex, frame)
                    .unwrap_or_else(|| panic!("unclassified: {frame}"))
                    .kind,
                FailureKind::Quota,
                "{frame}"
            );
        }
        // The phrasing alone still reads as exhausted where the frame carries no
        // machine-readable code, exactly as the `codex exec` stream does.
        let uncoded = r#"{"method":"error","params":{"error":{"message":"You've hit your usage limit. Try again at Dec 31st, 2027 6:00 AM."}}}"#;
        assert_eq!(
            detect_harness_provider_failure(FailureDialect::Codex, uncoded)
                .unwrap()
                .kind,
            FailureKind::Quota
        );
    }

    #[test]
    fn codex_account_model_refusal_classifies_as_model_not_found_for_any_model() {
        // The real capture: `codex exec -m gpt-5.5` under a ChatGPT sign-in. The
        // provider's object is the *string* value of `turn.failed`'s message,
        // and it is what the reading hands back as the cause.
        let captured =
            include_str!("../../../../tests/fixtures/codex-model-unsupported-for-account.jsonl");
        let got = detect_harness_provider_failure(FailureDialect::Codex, captured).unwrap();
        assert_eq!(got.kind, FailureKind::ModelNotFound);
        assert_eq!(got.source, "stdout");
        let detail = got.detail.expect("the provider's own object is the cause");
        assert!(
            detail.starts_with(r#"{"type":"error","status":400"#),
            "{detail}"
        );
        assert!(model_refused_for_account(FailureDialect::Codex, captured));
        // The model and the account kind are not the signature, and the
        // app-server's terminal frame carries the same error object.
        for stdout in [
            r#"{"type":"turn.failed","error":{"message":"{\"type\":\"error\",\"status\":400,\"error\":{\"type\":\"invalid_request_error\",\"message\":\"The 'o9-preview' model is not supported when using Codex with an enterprise account.\"}}"}}"#,
            r#"{"method":"turn/completed","params":{"turn":{"status":"failed","error":{"message":"{\"type\":\"error\",\"status\":400,\"error\":{\"type\":\"invalid_request_error\",\"message\":\"The 'gpt-5.5' model is not supported when using Codex with a ChatGPT account.\"}}"}}}}"#,
        ] {
            let got = detect_harness_provider_failure(FailureDialect::Codex, stdout)
                .unwrap_or_else(|| panic!("unclassified: {stdout}"));
            assert_eq!(got.kind, FailureKind::ModelNotFound, "{stdout}");
            assert!(
                model_refused_for_account(FailureDialect::Codex, stdout),
                "{stdout}"
            );
        }
    }

    #[test]
    fn only_codexs_account_model_refusal_reads_as_one() {
        // Each stable part is load-bearing: another 400, another error type,
        // another status, the sentence as prose rather than the provider's
        // object, or an agent merely quoting it — none is the refusal.
        let refusal =
            "The 'gpt-5.5' model is not supported when using Codex with a ChatGPT account.";
        let turn_failed = |status: u16, kind: &str, message: &str| {
            let provider = serde_json::json!({
                "type": "error",
                "status": status,
                "error": {"type": kind, "message": message},
            });
            serde_json::json!({"type": "turn.failed", "error": {"message": provider.to_string()}})
                .to_string()
        };
        for stdout in [
            turn_failed(400, "invalid_request_error", "Unsupported parameter: 'temperature' is not supported with this model."),
            turn_failed(400, "server_error", refusal),
            turn_failed(404, "invalid_request_error", refusal),
            serde_json::json!({"type": "turn.failed", "error": {"message": refusal}}).to_string(),
            serde_json::json!({"type": "item.completed", "item": {"type": "agent_message", "text": refusal}}).to_string(),
        ] {
            assert!(
                detect_harness_provider_failure(FailureDialect::Codex, &stdout).is_none(),
                "{stdout}"
            );
            assert!(!model_refused_for_account(FailureDialect::Codex, &stdout), "{stdout}");
        }
        // The shape is Codex's own, read for Codex only.
        let captured =
            include_str!("../../../../tests/fixtures/codex-model-unsupported-for-account.jsonl");
        assert!(!model_refused_for_account(
            FailureDialect::Generic,
            captured
        ));
        assert!(!model_refused_for_account(
            FailureDialect::ClaudeCode,
            captured
        ));
        // Only the record that classified the run is read: a later generic
        // rejection is the reading, and the earlier refusal does not survive it.
        let generic_after = format!(
            "{captured}{}\n",
            r#"{"type":"result","is_error":true,"result":"model not found: gpt-5.5"}"#
        );
        assert_eq!(
            detect_harness_provider_failure(FailureDialect::Codex, &generic_after)
                .unwrap()
                .kind,
            FailureKind::ModelNotFound
        );
        assert!(!model_refused_for_account(
            FailureDialect::Codex,
            &generic_after
        ));
    }

    #[test]
    fn codex_server_overloaded_codes_classify_on_exec_and_app_server_surfaces() {
        for stdout in [
            r#"{"type":"turn.failed","error":{"codex_error_info":"server_overloaded"}}"#,
            r#"{"type":"turn.failed","error":{"input_error_code":"server_overloaded"}}"#,
            r#"{"method":"turn/completed","params":{"turn":{"status":"failed","error":{"codexErrorInfo":"serverOverloaded"}}}}"#,
        ] {
            let got = detect_harness_provider_failure(FailureDialect::Codex, stdout)
                .unwrap_or_else(|| panic!("unclassified overload: {stdout}"));
            assert_eq!(got.kind, FailureKind::ServerOverloaded, "{stdout}");
        }
    }

    #[test]
    fn codex_app_server_ordinary_failure_is_never_a_quota_rejection() {
        // An app-server turn that failed for some other reason is a real task
        // failure: falling through it would re-run a paid task on another
        // account. The code is matched exactly, so its prose cannot promote it.
        let other = r#"{"method":"error","params":{"error":{"message":"You've hit your usage limit, the model said. Stream disconnected before completion.","codexErrorInfo":"streamDisconnected","additionalDetails":null},"willRetry":false}}"#;
        assert!(detect_harness_provider_failure(FailureDialect::Codex, other).is_none());
        let failed_turn = r#"{"method":"turn/completed","params":{"turn":{"id":"t1","items":[],"status":"failed","error":{"message":"sandbox denied the write","codexErrorInfo":"sandboxDenied"}}}}"#;
        assert!(detect_harness_provider_failure(FailureDialect::Codex, failed_turn).is_none());
        // A turn that SUCCEEDED is not a rejection either, however its answer
        // was worded — the status is read, never assumed from the frame name.
        let succeeded = r#"{"method":"turn/completed","params":{"turn":{"id":"t1","items":[],"status":"completed","error":{"message":"You've hit your usage limit","codexErrorInfo":"usageLimitExceeded"}}}}"#;
        assert!(detect_harness_provider_failure(FailureDialect::Codex, succeeded).is_none());
        // A limit the server says it is going to retry is not a refusal either:
        // the turn may still complete, and reporting `quota` for a run that
        // worked is the opposite of what the caller asked for. Only a positive
        // `willRetry` disqualifies it — an absent field states no intention.
        let retrying = r#"{"method":"error","params":{"error":{"message":"You've hit your usage limit","codexErrorInfo":"usageLimitExceeded"},"willRetry":true}}"#;
        assert!(detect_harness_provider_failure(FailureDialect::Codex, retrying).is_none());
        // ...but the retry that then failed on the same limit still classifies,
        // so the rule above costs a real refusal nothing.
        let retried_then_failed = format!(
            "{retrying}\n{}",
            r#"{"method":"turn/completed","params":{"turn":{"id":"t1","items":[],"status":"failed","error":{"message":"You've hit your usage limit","codexErrorInfo":"usageLimitExceeded"}}}}"#
        );
        assert_eq!(
            detect_harness_provider_failure(FailureDialect::Codex, &retried_then_failed)
                .unwrap()
                .kind,
            FailureKind::Quota
        );
        // ...and an agent that merely quotes the wording mid-turn is not the
        // harness declaring anything: the app-server streams its text as a
        // delta, which carries no error object at all.
        let quoted = r#"{"method":"item/agentMessage/delta","params":{"itemId":"i1","delta":"You've hit your usage limit"}}"#;
        assert!(detect_harness_provider_failure(FailureDialect::Codex, quoted).is_none());
        assert!(classify_harness_failure(FailureDialect::Codex, quoted, "").is_none());
        // The frames stay adapter-scoped, like every codex reading here.
        let captured =
            include_str!("../../../../tests/fixtures/codex-app-server-usage-limit.jsonl");
        assert!(detect_harness_provider_failure(FailureDialect::Generic, captured).is_none());
    }

    #[test]
    fn provider_error_record_is_classified_on_a_clean_process_exit() {
        let stdout = concat!(
            r#"{"type":"system","subtype":"init"}"#,
            "\n",
            r#"{"type":"result","subtype":"success","is_error":true,"api_error_status":401,"result":"Invalid API key · Fix external API key"}"#
        );
        let got = detect_provider_failure(stdout).unwrap();
        assert_eq!(got.kind, FailureKind::Auth);
        assert_eq!(got.source, "stdout");
        assert!(detect_provider_failure(
            r#"{"type":"result","is_error":false,"result":"mentions a 401 example"}"#
        )
        .is_none());
    }

    #[test]
    fn detects_deferred_tool_with_named_tool() {
        // Claude Code's bridge-deployment shape: a clean (exit 0) result whose
        // `stop_reason` is `tool_deferred`, empty `result`, and a named
        // `deferred_tool_use`. The tool name is lifted, never fabricated.
        let raw = r#"{"type":"result","num_turns":1,"stop_reason":"tool_deferred",
            "terminal_reason":"tool_deferred","result":"","permission_denials":[],
            "deferred_tool_use":{"name":"Read","input":{"file_path":"/x/usage.rs"}}}"#;
        let got = detect_deferred_tool(raw).unwrap();
        assert_eq!(got.tool.as_deref(), Some("Read"));
    }

    #[test]
    fn detects_deferred_tool_from_deferred_use_without_reason() {
        // Even without a reason field, a `deferred_tool_use` alongside an empty
        // result is the same dead-end.
        let raw = r#"{"result":"","deferred_tool_use":{"name":"Bash"}}"#;
        assert_eq!(
            detect_deferred_tool(raw).unwrap().tool.as_deref(),
            Some("Bash")
        );
    }

    #[test]
    fn detects_deferred_tool_without_a_name() {
        // A deferral signalled only by the reason, with no tool named, still
        // detects — the name stays `None` rather than being invented.
        let raw = r#"{"stop_reason":"tool_deferred","result":""}"#;
        assert_eq!(detect_deferred_tool(raw), Some(DeferredTool { tool: None }));
    }

    #[test]
    fn no_deferred_tool_on_a_normal_result() {
        // A normal answer (non-empty `result`, no `deferred_tool_use`) never trips
        // the detector, and neither does non-JSON or empty output.
        assert!(detect_deferred_tool(r#"{"type":"result","result":"pong"}"#).is_none());
        assert!(detect_deferred_tool(r#"{"stop_reason":"end_turn","result":"hi"}"#).is_none());
        assert!(detect_deferred_tool("not json").is_none());
        assert!(detect_deferred_tool("").is_none());
    }

    #[test]
    fn deferred_tool_prefers_terminal_stream_event() {
        // In a stream, the terminal event carries the deferral; scanning from the
        // end finds it even after ordinary earlier events.
        let raw = concat!(
            "{\"type\":\"system\",\"session_id\":\"s\"}\n",
            "{\"type\":\"result\",\"stop_reason\":\"tool_deferred\",\"result\":\"\",\
             \"deferred_tool_use\":{\"name\":\"Read\"}}\n",
        );
        assert_eq!(
            detect_deferred_tool(raw).unwrap().tool.as_deref(),
            Some("Read")
        );
    }

    #[test]
    fn summed_usage_skips_partless_and_usageless_step_events() {
        // The per-step sum must ignore events that carry no usage without
        // mis-reading them: a bare JSON value (no object), a `step_finish` with no
        // `part`, and a `step_finish` whose `part` has neither tokens nor cost.
        // Only the final event contributes, so the sum equals exactly that event.
        let raw = concat!(
            "42\n",
            "{\"type\":\"step_finish\",\"sessionID\":\"ses_1\"}\n",
            "{\"type\":\"step_finish\",\"sessionID\":\"ses_1\",\"part\":{}}\n",
            "{\"type\":\"step_finish\",\"sessionID\":\"ses_1\",\"part\":\
             {\"cost\":0.005,\"tokens\":{\"input\":10,\"output\":2}}}\n",
        );
        let got = extract_usage(raw).unwrap();
        assert_eq!(got.usage.input_tokens, Some(10));
        assert_eq!(got.usage.output_tokens, Some(2));
        assert!((got.usage.cost_usd.unwrap() - 0.005).abs() < 1e-9);
        assert_eq!(got.source, "json:summed-steps");
    }

    #[test]
    fn summed_usage_partial_step_reports_only_present_fields() {
        // A `step_finish` whose `part` has a cost but no `tokens` object (and
        // vice versa) yields a reading with the present field set and the absent
        // one left null — `add_u64`/`add_f64` report "no number present" for the
        // missing side rather than defaulting it to zero.
        let cost_only = "{\"type\":\"step_finish\",\"part\":{\"cost\":0.02}}\n";
        let got = extract_usage(cost_only).unwrap();
        assert_eq!(got.usage.cost_usd, Some(0.02));
        assert_eq!(got.usage.input_tokens, None);
        assert_eq!(got.usage.output_tokens, None);

        let tokens_only = "{\"type\":\"step_finish\",\"part\":{\"tokens\":{\"input\":5}}}\n";
        let got = extract_usage(tokens_only).unwrap();
        assert_eq!(got.usage.input_tokens, Some(5));
        assert_eq!(got.usage.output_tokens, None);
        assert_eq!(got.usage.cost_usd, None);
    }

    #[test]
    fn summed_usage_read_only_cache_hit_leaves_write_null() {
        // The headline scenario: a pure cache *hit* reads a cached prefix and
        // writes nothing, so OpenCode's `cache` block carries `read` but no
        // `write`. The read surfaces; `write` stays null rather than defaulting
        // to zero (never-fabricate, even for the half that's absent).
        let raw = "{\"type\":\"step_finish\",\"part\":\
            {\"tokens\":{\"input\":2,\"output\":1,\"cache\":{\"read\":9000}}}}\n";
        let got = extract_usage(raw).unwrap();
        assert_eq!(got.usage.cache_read_tokens, Some(9000));
        assert_eq!(got.usage.cache_write_tokens, None);
        assert_eq!(got.source, "json:summed-steps");
    }
}
