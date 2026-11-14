//! The serializable shapes that make up oneharness's output contract.
//!
//! The JSON report carries a `schema_version`: consumers depend on it, so fields
//! are added, never repurposed or removed, without bumping the version.

use std::fmt;

use schemars::JsonSchema;
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;

use crate::domain::batch::BatchStrategy;
use crate::domain::control::ControlReport;
use crate::domain::events::ActionEvent;
use crate::domain::fallback::{FallThroughReason, RunWork};
use crate::domain::mode::PermissionMode;
use crate::domain::session::SessionPhase;
use crate::domain::signals::{FailureKind, Usage};
use crate::domain::usage::UtcInstant;

/// Bumped when the JSON shape changes in a way consumers must notice.
///
/// Shared by every non-history report on stdout — `run`, `list`, `detect`,
/// `sync`, and `config` — so one number describes the whole surface; the history
/// records carry their own (`domain::history::SCHEMA_VERSION`).
///
/// `0.10` added [`RunResult::observed_model`] — the model the harness itself
/// reported the conversation would run under, read off its own protocol before
/// any token was spent (codex's app-server names it on the `thread/start` /
/// `thread/resume` response) — with the `model_mismatch` [`FailureKind`] and
/// its `"model-mismatch"` fall-through reason: the refusal oneharness answers,
/// before the turn starts, when that report and the requested model differ.
/// Purely additive — every 0.9 field keeps its name, type, and meaning — but a
/// consumer that exhaustively matches `failure_kind` learns of the new value
/// only from the version.
///
/// `0.9` added the work reading a failed run has to show for itself:
/// [`RunResult::work`] on a failure no classifier recognized, and
/// [`FallbackReport::stopped_without_work`] where such a candidate is the one a
/// chain stopped at. Purely additive — every 0.8 field keeps its name, type, and
/// meaning, and no verdict changed with them — but the bump is how a consumer
/// learns that "the run failed and nothing says why" is now a question the report
/// answers rather than one every reader re-derives from `usage` and `events`.
///
/// `0.8` added the two precondition [`FailureKind`]s — `untrusted_directory` and
/// `input_too_large` — with their `"untrusted-directory"` / `"input-too-large"`
/// fall-through reasons, and `detail` on each [`FallThrough`] (the provider's own
/// machine-readable account, `null` when it gave none). Purely additive: every
/// 0.7 field keeps its name, type, and meaning. The new *enum values* are why the
/// bump matters, since a consumer that exhaustively matches `failure_kind` learns
/// from the version that two more now exist.
///
/// `0.12` adds [`FailureKind::AppServerLost`] and its `"app-server-lost"`
/// fallback reason: a pooled Codex dispatch whose shared app-server went away
/// under its live turn. A new enum value, so exhaustive consumers learn of it
/// from the version. The `config` report's codex harness and variant entries
/// gain the resolved `app_server` (`"shared"` or `"per-dispatch"`) beside it,
/// additively.
///
/// `0.11` adds [`FailureKind::ServerOverloaded`] and its
/// `"server-overloaded"` fallback reason. The enum values require the bump so
/// exhaustive consumers can reject rather than misread the new classification.
///
/// `0.7` added the `session_not_found` [`FailureKind`] — the refusal a harness
/// returns when asked to continue a session its identity has never seen — and,
/// with it, the `"session-not-found"` reason a fallback run reports for a
/// candidate it routed around. Purely additive: every 0.6 field keeps its name,
/// type, and meaning. The new *enum value* is why the bump matters, since a
/// consumer that exhaustively matches `failure_kind` learns from the version that
/// a sixth value now exists.
///
/// `0.6` added the `run` report's `control` block (the out-of-band turn-control
/// socket a `--control` run listened on, and every interrupt it served).
/// Additive, so a reader of `0.5` keeps working — the bump is how a consumer
/// *learns* the field exists rather than having to probe for it.
///
/// `0.5` added two things to the `run` report: the measured
/// [`ExecutionTelemetry`] on each result (previously internal, which forced a
/// consumer to re-read the history file for numbers the run already knew), and
/// the `cancelled` [`Status`] a run reaches when the caller — or a SIGINT/SIGTERM
/// on the host — tore the harness tree down before it finished. Both are
/// additive: every 0.4 field keeps its name, type, and meaning. The new *status
/// value* is why that bump mattered, since a consumer that exhaustively matches
/// `status` learns from the version that a sixth value now exists.
///
/// `0.4` added the `config` report's `stream` field (the layered `--stream`
/// value, with its provenance).
pub const SCHEMA_VERSION: &str = "0.12";

/// How a harness emits its result, which decides how `text` is extracted.
///
/// Also accepted as a CLI value (`--output-format`, parsed in the `oneharness`
/// binary) and a config-file value (`output_format`, via `Deserialize`). The
/// CLI parsing lives in the binary so this core crate stays free of `clap`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum OutputFormat {
    /// Plain text on stdout; `text` is the trimmed stdout.
    Text,
    /// A single JSON document on stdout.
    Json,
    /// Line-delimited JSON events on stdout.
    StreamJson,
}

impl OutputFormat {
    /// The stable CLI/config/wire token for this format.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Text => "text",
            Self::Json => "json",
            Self::StreamJson => "stream-json",
        }
    }
}

/// The outcome of attempting to run one harness.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum Status {
    /// Spawned and exited 0.
    Ok,
    /// Spawned and exited non-zero.
    Nonzero,
    /// Killed after exceeding the per-harness timeout.
    Timeout,
    /// Terminated before it finished because the run was **cancelled** — a
    /// library caller flipped its [`crate::io::cancel::CancelToken`], or the host
    /// received SIGINT/SIGTERM while [`crate::io::cancel::install_signal_cancel`]
    /// was in force. Distinct from `timeout` (the harness was given its full
    /// deadline and exceeded it) and from a consumer-driven streaming stop (which
    /// stays `ok`, because the consumer already got what it asked for). Any
    /// output captured before the cancellation is still normalized, exactly as it
    /// is for a timeout.
    Cancelled,
    /// The binary was resolved but could not be executed.
    SpawnError,
    /// The harness was not run. Either the binary was not found (`available:
    /// false`), or it was found but the identity the selection points at is not
    /// provisioned — an `env_from` home directory that is not on disk, reported
    /// as `available: true` with `failure_kind: "auth"`.
    Skipped,
    /// `--print-command`: the command was built but not executed.
    Planned,
}

impl Status {
    /// Every status, for the surfaces that enumerate them.
    pub const ALL: [Status; 7] = [
        Status::Ok,
        Status::Nonzero,
        Status::Timeout,
        Status::Cancelled,
        Status::SpawnError,
        Status::Skipped,
        Status::Planned,
    ];

    /// The JSON token this status serializes to, for prose that names it.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Status::Ok => "ok",
            Status::Nonzero => "nonzero",
            Status::Timeout => "timeout",
            Status::Cancelled => "cancelled",
            Status::SpawnError => "spawn-error",
            Status::Skipped => "skipped",
            Status::Planned => "planned",
        }
    }
}

/// The raw capture from running a subprocess — produced by the io layer,
/// consumed (with extraction) by the command layer. Carries no extraction so
/// the spawn path and the parse path stay independently testable.
#[derive(Debug, Clone)]
pub struct Capture {
    pub status: Status,
    pub exit_code: Option<i32>,
    pub duration_ms: Option<u128>,
    pub stdout: String,
    pub stderr: String,
    pub error: Option<String>,
    /// UTC invocation boundaries observed by the runner (never synthesized by
    /// history serialization).
    pub started_at: String,
    pub finished_at: Option<String>,
    /// Complete stdout chunks paired with their monotonic offset from start.
    pub stdout_observations: Vec<OutputObservation>,
}

#[derive(Debug, Clone)]
pub struct OutputObservation {
    pub offset_ms: u128,
    pub observed_at: String,
    pub bytes: Vec<u8>,
}

/// The exact text [`crate::domain::history::format_rfc3339_millis`] produces:
/// `YYYY-MM-DDTHH:MM:SS.mmmZ`, always 24 characters.
const RUN_INSTANT_LEN: u32 = 24;
const RUN_INSTANT_PATTERN: &str =
    r"^\d{4}-(0[1-9]|1[0-2])-(0[1-9]|[12]\d|3[01])T([01]\d|2[0-3]):[0-5]\d:([0-5]\d|60)\.\d{3}Z$";

/// The error returned when text is not a [`RunInstant`].
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("must be an RFC 3339 UTC instant with milliseconds, e.g. 2026-01-01T00:00:00.000Z")]
pub struct RunInstantError;

/// An RFC 3339 **UTC** instant at millisecond precision — a runner clock read.
///
/// Deliberately *not* [`UtcInstant`], which canonicalizes to whole seconds: the
/// invocation bounds this carries are minted in milliseconds, and a type that
/// silently truncated them would make a sub-second run report as instantaneous.
/// Wrapping the text is what keeps a timestamp that is not UTC, not
/// millisecond-precise, or not a timestamp at all out of a measurement — the same
/// bar `UtcInstant` holds its own field to.
///
/// The `FromStr` rule and the [`JsonSchema`] below state one thing, so the Rust
/// reader and every generated SDK validator accept exactly the same values. The
/// length bound is load-bearing, not decorative: it is what makes a trailing
/// newline fail even where a regex `$` would match before one.
///
/// The schema states everything a JSON Schema can: the shape, the length, and
/// each component's range (month `01`-`12`, hour `00`-`23`, a real `:60` leap
/// second). `FromStr` adds the one thing a regex cannot express — that the date
/// exists at all, so `2026-02-30` is refused. The residual slack is **one-way**
/// and harmless: a generated validator accepts a shape this parser would reject,
/// but oneharness can never emit one, because every value it writes came from a
/// real clock and through here.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct RunInstant(String);

impl RunInstant {
    /// The instant `millis` milliseconds after the UNIX epoch.
    ///
    /// The way a clock read enters this type, mirroring
    /// [`UtcInstant::from_epoch`]: canonical by construction, so the io layer
    /// never formats an instant only to parse it back — a round trip whose
    /// failure arm could only ever be an unreachable panic.
    #[must_use]
    pub fn from_epoch_millis(millis: u128) -> Self {
        Self(crate::domain::history::format_rfc3339_millis(millis))
    }

    /// The canonical millisecond-precision UTC text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::str::FromStr for RunInstant {
    type Err = RunInstantError;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let bytes = text.as_bytes();
        if bytes.len() != RUN_INSTANT_LEN as usize {
            return Err(RunInstantError);
        }
        let shaped = bytes.iter().enumerate().all(|(index, byte)| match index {
            4 | 7 => *byte == b'-',
            10 => *byte == b'T',
            13 | 16 => *byte == b':',
            19 => *byte == b'.',
            23 => *byte == b'Z',
            _ => byte.is_ascii_digit(),
        });
        if !shaped {
            return Err(RunInstantError);
        }
        // Then that it is a real instant, by the same rule the usage parser holds
        // its own timestamps to: every component in range *and* a date that exists.
        // llmlint: ignore[contracts_have_one_source_or_a_drift_gate] The generated SDK validators state this rule as far as a JSON Schema reaches — length, shape, and every component's range, per RUN_INSTANT_PATTERN; only "the date exists" is inexpressible as a regex. The resulting slack runs one way (a validator accepts what this parser rejects, never the reverse), so no consumer is ever told a value is valid that oneharness would refuse to write.
        crate::domain::usage::normalize_timestamp(text)
            .ok_or(RunInstantError)
            .map(|_| Self(text.to_string()))
    }
}

impl fmt::Display for RunInstant {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl<'de> Deserialize<'de> for RunInstant {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        String::deserialize(deserializer)?
            .parse()
            .map_err(serde::de::Error::custom)
    }
}

impl JsonSchema for RunInstant {
    fn inline_schema() -> bool {
        true
    }

    fn schema_name() -> std::borrow::Cow<'static, str> {
        std::borrow::Cow::Borrowed("RunInstant")
    }

    fn json_schema(_generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
        schemars::json_schema!({
            "type": "string",
            "minLength": RUN_INSTANT_LEN,
            "maxLength": RUN_INSTANT_LEN,
            "pattern": RUN_INSTANT_PATTERN,
        })
    }
}

/// Measured execution telemetry for one harness run: when the invocation ran,
/// and — when the harness's transcript supports it — how its wall clock split
/// between provider latency and tool work.
///
/// Carried from the runner/parser to the history writer *and*, since report
/// schema `0.5`, serialized on [`RunResult::telemetry`]. Exposing it there is
/// what lets a consumer read the numbers off the run it just made instead of
/// re-opening the history file the same run wrote.
///
/// Internally tagged by `source`, so the variant is a value a consumer switches
/// on rather than a shape it has to sniff. Every variant states only what was
/// actually measured — there is no variant meaning "no telemetry"; that is a
/// `null` field.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(tag = "source", rename_all = "snake_case")]
pub enum ExecutionTelemetry {
    /// The harness's own transcript carried a complete provider trace, so the
    /// invocation bounds *and* the model/tool split are measured.
    ProviderMeasured {
        started_at: RunInstant,
        finished_at: Option<RunInstant>,
        model_ms: Option<u128>,
        tool_ms: Option<u128>,
        time_to_first_token_ms: Option<u128>,
    },
    /// The invocation bounds the runner observed for a run whose provider trace
    /// never completed, with no model/tool split: a split read out of a
    /// transcript that stopped mid-turn is not a measurement, but when the run
    /// itself started is, and it is what an operator reads off a failure. Typed
    /// as a [`UtcInstant`] so the one thing this variant claims to know cannot be
    /// empty or in some other offset by the time a record renders it.
    PartialInvocation {
        #[schemars(with = "String")]
        started_at: UtcInstant,
    },
    /// The union of tool intervals observed at the stdout pipe, for a harness
    /// whose transcript carries no provider trace to split. Not provider-measured
    /// and with no model-latency counterpart, which is what the `source` tag tells
    /// a consumer; per-event provenance lives on `ActionEvent`.
    StdoutObserved { tool_ms: u128 },
}

impl<'de> Deserialize<'de> for ExecutionTelemetry {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        // Serde's internally-tagged buffer cannot hold a u128, and these durations
        // legitimately are u128 — the same constraint `RunStreamEnvelope` below
        // documents. So the tag is read off a `Value` and each variant's body is
        // then handed to serde_json's number-aware deserializer, whole. Deriving
        // the bodies (rather than picking fields out of the `Value` by hand) is
        // what keeps this reader exactly as strict as the schema generated from
        // the same types: a missing or wrong-typed field is an error, never a
        // silent `None`, and an additive field from a newer producer is ignored.
        let value = Value::deserialize(deserializer)?;
        let source = value.get("source").and_then(Value::as_str).ok_or_else(|| {
            serde::de::Error::custom("execution telemetry is missing string field `source`")
        })?;
        let variant = |name: &str| -> Result<Self, D::Error> {
            match name {
                "provider_measured" => {
                    serde_json::from_value::<ProviderMeasuredWire>(value.clone())
                        .map(|wire| Self::ProviderMeasured {
                            started_at: wire.started_at,
                            finished_at: wire.finished_at,
                            model_ms: wire.model_ms,
                            tool_ms: wire.tool_ms,
                            time_to_first_token_ms: wire.time_to_first_token_ms,
                        })
                        .map_err(serde::de::Error::custom)
                }
                "partial_invocation" => {
                    serde_json::from_value::<PartialInvocationWire>(value.clone())
                        .map(|wire| Self::PartialInvocation {
                            started_at: wire.started_at,
                        })
                        .map_err(serde::de::Error::custom)
                }
                "stdout_observed" => serde_json::from_value::<StdoutObservedWire>(value.clone())
                    .map(|wire| Self::StdoutObserved {
                        tool_ms: wire.tool_ms,
                    })
                    .map_err(serde::de::Error::custom),
                other => Err(serde::de::Error::custom(format!(
                    "unknown execution telemetry source `{other}`"
                ))),
            }
        };
        variant(source)
    }
}

/// The `provider_measured` body, minus its tag. Every field is required and
/// independently nullable, exactly as the generated schema states it.
///
/// `required_nullable` is what makes that true: serde otherwise fills a missing
/// `Option` field with `None`, which would read an absent measurement as "not
/// measured" — a claim the producer never made, and one every generated SDK
/// validator refuses.
#[derive(Deserialize)]
struct ProviderMeasuredWire {
    started_at: RunInstant,
    #[serde(deserialize_with = "required_nullable")]
    finished_at: Option<RunInstant>,
    #[serde(deserialize_with = "required_nullable")]
    model_ms: Option<u128>,
    #[serde(deserialize_with = "required_nullable")]
    tool_ms: Option<u128>,
    #[serde(deserialize_with = "required_nullable")]
    time_to_first_token_ms: Option<u128>,
}

/// Read an `Option<T>` that must be *present* (as a value or an explicit
/// `null`). Naming a `deserialize_with` is what turns off serde's implicit
/// missing-field default for an `Option`; the body is the ordinary decode.
fn required_nullable<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(deserializer)
}

#[derive(Deserialize)]
struct PartialInvocationWire {
    started_at: UtcInstant,
}

#[derive(Deserialize)]
struct StdoutObservedWire {
    tool_ms: u128,
}

/// One harness's entry in the report.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct RunResult {
    // llmlint: ignore[invalid_states_unrepresentable] These three additive wire fields must remain directly addressable for the stable JSON/SDK contract; command construction always derives all three together from one validated composed selection in `apply_result_identity`, and round-trip integration tests pin their consistency.
    /// Canonical harness id (e.g. `claude-code`).
    pub harness: String,
    /// Named preset, when this result came from a composed harness id.
    // llmlint: ignore[invalid_states_unrepresentable] This additive public wire field must remain an Option<String> for generated SDK compatibility; all production constructors set it together with harness/harness_id via `apply_result_identity`, and subprocess tests assert the triplet.
    pub variant: Option<String>,
    /// Base id or `<base>:<variant>`, suitable for selecting the same candidate.
    // llmlint: ignore[invalid_states_unrepresentable] This stable serialized selector remains a String so existing consumers can round-trip it; production code derives it from the validated selection alongside base/variant and integration tests pin consistency.
    pub harness_id: String,
    /// The binary name or path oneharness resolved and would invoke.
    pub bin: String,
    /// Whether that binary was found.
    pub available: bool,
    pub status: Status,
    /// The prompt this result ran, set only on a **batch** run (one harness
    /// fanned over N prompts), where each result has its own prompt. `null` on an
    /// ordinary run, where the single top-level `prompt` applies to every result.
    pub prompt: Option<String>,
    /// The model this result ran with (the value oneharness put on the harness's
    /// model flag), or `null` when no model was requested and the harness used its
    /// own default. On a **model fan-out** run (`RunReport::models`), this is what
    /// distinguishes results that share a harness — each entry is one (harness,
    /// model) pair. The model is also visible in `command`; this field surfaces it
    /// without parsing the argv.
    pub model: Option<String>,
    /// The model the harness **itself** reported the conversation would run
    /// under, read off its own protocol before any token was spent — for codex
    /// over `app-server`, `result.model` of the `thread/start` / `thread/resume`
    /// response. `null` on every path that reports none, which today is every
    /// path but codex's app-server. Never inferred and never copied from
    /// `model`: where both are set they are the requested model and the served
    /// one, and a difference between them is exactly the `model_mismatch`
    /// refusal, which this field then names the other half of.
    pub observed_model: Option<String>,
    /// Process exit code; `null` when not run, timed out, or signalled.
    pub exit_code: Option<i32>,
    /// Wall-clock duration of the run; `null` when not executed.
    pub duration_ms: Option<u128>,
    /// Measured execution telemetry for this run: the invocation bounds and, when
    /// the harness's transcript carried a trace to read them from, the
    /// model/tool split. `null` when nothing was measured — never estimated.
    /// Added in report schema `0.5`; before that a consumer had to re-read the
    /// history file for numbers the run itself already had.
    #[serde(default)]
    pub telemetry: Option<ExecutionTelemetry>,
    /// The exact argv oneharness built; its first element is the binary.
    pub command: Vec<String>,
    pub output_format: OutputFormat,
    /// Best-effort final assistant text; `null` when extraction is impossible.
    pub text: Option<String>,
    /// How `text` was extracted (e.g. `json:result`, `raw`); `null` when absent.
    pub text_source: Option<String>,
    /// Best-effort token/cost accounting; every field is `null` when the harness
    /// does not report it. Always present so consumers can read a stable shape.
    pub usage: Usage,
    /// How `usage` was read (e.g. `json`); `null` when nothing was found.
    pub usage_source: Option<String>,
    /// Best-effort harness session id for continuation; `null` when none is
    /// exposed. Surfaced for a consumer to thread into `--resume`, and consumed
    /// by oneharness itself when `--session` is in play (it is captured into the
    /// session store to back the uniform handle — see [`RunReport::session`]).
    pub session_id: Option<String>,
    /// Best-effort normalized tool-call / action events the harness took (shell
    /// commands, file edits, tool uses), in order — so consumers can assert on
    /// *behavior*, not just the final `text`. `null` when the harness's output
    /// exposes no machine-readable trace (a plain-text harness, or Claude Code's
    /// single-document `json` result), distinct from `[]` — an empty array is not
    /// currently emitted; absence is signalled by `null` + a null `events_source`.
    /// Never fabricated. See [`crate::domain::events`].
    pub events: Option<Vec<ActionEvent>>,
    /// How `events` was recovered (e.g. `json:opencode-parts`,
    /// `stream-json:content-blocks`), parallel to `text_source`; `null` when no
    /// events were found. Lets a consumer tell "harness doesn't support it" from
    /// "no tools were used."
    pub events_source: Option<String>,
    /// Structured-output run only: the JSON value extracted from the final
    /// answer and validated against the requested schema. `null` when no schema
    /// was requested, or when no JSON value could be extracted. Carries the
    /// last-attempted value even when it failed validation, so a consumer can
    /// see what the harness produced.
    pub structured: Option<Value>,
    /// Structured-output run only: whether `structured` conformed to the schema
    /// on the final attempt. `null` when no schema was requested (or the harness
    /// did not run); `false` when a schema was requested but the result never
    /// conformed (including "no JSON found").
    pub schema_valid: Option<bool>,
    /// Structured-output run only: how many times this harness was invoked under
    /// the validate/retry loop (1 + retries). `null` when no schema was
    /// requested or the harness did not run.
    pub schema_attempts: Option<u32>,
    /// Structured-output run only: the validation errors from the final attempt,
    /// joined for display; `null` when valid or no schema was requested.
    pub schema_error: Option<String>,
    /// Best-effort failure reason; `null` when unclassified. Distinct from
    /// `status`, which records oneharness's relationship to the process. Two
    /// families: coarse reasons for a non-zero run (`auth`, `rate_limit`,
    /// `model_not_found`, `quota`, `session_not_found`, `untrusted_directory`,
    /// `input_too_large`, `model_mismatch`, `server_overloaded`), and
    /// `tool_deferred` — a run that only deferred a builtin tool call instead of
    /// executing it (Claude Code bridge/managed deployments), so it did no useful
    /// work. Both `tool_deferred` and `server_overloaded` can accompany a clean
    /// transport exit; either still marks the run as failed for exit-code purposes.
    /// Serialized as its snake_case token (see [`FailureKind`]).
    pub failure_kind: Option<FailureKind>,
    /// What this run has to show for itself when `failure_kind` has nothing to
    /// say: [`RunWork::Done`] if the harness recorded a tool call or billed
    /// usage, [`RunWork::None`] if nothing says it got that far. `null` on every
    /// other run — a success needs no such reading, and a *classified* failure
    /// already names its cause.
    ///
    /// This is the reading the fallback verdict itself consults
    /// ([`crate::domain::fallback::startup_failure_reason`]), published rather
    /// than left to be re-derived: an unclassified failure that did nothing is
    /// otherwise indistinguishable from one that ran the task and failed it, and
    /// the two mean opposite things to whoever reads the run. Publishing it
    /// changes no verdict — a candidate reading `none` still stops a chain, and
    /// one reading `done` still never falls through it.
    // llmlint: ignore[invalid_states_unrepresentable] This additive wire field must stay flat beside `status`/`failure_kind` for the stable JSON/SDK contract that generates every client; the one production path derives all three together in `with_work_evidence`, and the history reader plus the shared contract matrix reject a record whose combination disagrees.
    #[serde(default)]
    pub work: Option<RunWork>,
    /// Where `failure_kind` was read (`stderr`/`stdout`, or `config:env_from`
    /// for a candidate refused before spawning); `null` when absent.
    // llmlint: ignore[invalid_states_unrepresentable] This is a stable serialized string in the JSON/SDK contract, deliberately open so a new reading site is an additive value rather than a generated-type change for every consumer; the values are produced only by `domain::signals` and the pre-spawn refusal, and the report round-trip tests pin them.
    pub failure_kind_source: Option<String>,
    /// Raw captured stdout (empty for skipped/planned).
    pub stdout: String,
    /// Raw captured stderr (empty for skipped/planned).
    pub stderr: String,
    /// Human-readable problem + suggested action; `null` on success.
    pub error: Option<String>,
}

impl RunResult {
    /// Apply this result's own work evidence to its signals, and hand the
    /// result back: drop a refusal classification the finished envelope
    /// contradicts, then fill in the derived [`work`][RunResult::work] reading.
    ///
    /// Every constructor ends here so both are derived in exactly one place,
    /// from the finished envelope rather than from whatever each call site
    /// happened to know: a second derivation is how a published signal and the
    /// verdict that consulted it start disagreeing.
    ///
    /// **The refusal reconciliation.** A `failure_kind` naming a *refusal*
    /// ([`FailureKind::is_refusal`]) is a claim that the harness or its provider
    /// would not run the task. [`completed_run_that_did_work`] is the run's own
    /// envelope refuting that claim, and the envelope wins: the classification
    /// is scanned out of a transcript one record at a time — an intermediate
    /// `is_error` record the harness went on to retry past is enough to produce
    /// one — while the envelope is what the harness said about the whole run.
    ///
    /// This is the sole reconciliation of that contradiction, and it is here
    /// rather than in the history reader's own validity rule
    /// ([`crate::domain::history`]) because history is opt-in and best-effort: a
    /// record refused there warns and disables the store, while the *report* on
    /// stdout — the contract the supervisor actually read — would still carry
    /// the refusal. Declining to stamp it closes it for every consumer at once,
    /// and every history record is derived from a result that already passed
    /// through here.
    ///
    /// It is the same rule
    /// [`startup_failure_reason`][sfr] applies one level down, where work
    /// evidence short-circuits every fall-through reason — lifted to the
    /// classification itself, so a consumer that never reads the fallback block
    /// gets the same answer the chain's verdict would have given.
    ///
    /// [sfr]: crate::domain::fallback::startup_failure_reason
    #[must_use]
    pub fn with_work_evidence(mut self) -> Self {
        self.refresh_work_evidence();
        self
    }

    /// [`Self::with_work_evidence`] in place, for a result whose signals
    /// arrived after it was built (a protocol-driven turn's usage).
    pub fn refresh_work_evidence(&mut self) {
        if self.failure_kind.is_some_and(FailureKind::is_refusal)
            && completed_run_that_did_work(self.status, self.exit_code, RunWork::from_result(self))
        {
            self.failure_kind = None;
            self.failure_kind_source = None;
        }
        self.work = unclassified_failure(self.status, self.failure_kind)
            .then(|| RunWork::from_result(self));
    }
}

/// Whether a finished run's own envelope says it **completed the task and did
/// the work**: the process ended cleanly ([`Status::Ok`] with exit code `0`) and
/// its [`RunWork`] evidence says the harness got that far — usage the provider
/// charged for, *or* a recorded tool call. Either alone is enough, so this is
/// not a question about billing: a harness that reported no accounting at all
/// and ran a tool did the task just as decisively.
///
/// All three halves are load-bearing. A non-zero exit, a timeout or a
/// cancellation is a run that did *not* complete, and there a refusal
/// classification is the best account of why. Work evidence is the same reading
/// [`RunWork::from_result`] gives the fallback verdict, so "did this candidate
/// do anything?" has one answer across the crate rather than one per reader.
#[must_use]
pub fn completed_run_that_did_work(status: Status, exit_code: Option<i32>, work: RunWork) -> bool {
    status == Status::Ok && exit_code == Some(0) && work == RunWork::Done
}

/// Whether a finished run failed with **nothing to classify** — the one case
/// [`RunResult::work`] answers, and the only one where it is published.
///
/// `failure_kind` is the first answer to "why did this fail?", so where it has
/// one there is nothing for a work reading to add. Where it is `null` the record
/// says only that the run failed, and whether the harness did any of the task is
/// the whole difference between a candidate that could not start and one that
/// tried and lost.
#[must_use]
pub fn unclassified_failure(status: Status, failure_kind: Option<FailureKind>) -> bool {
    failure_kind.is_none() && attempted_failure(status)
}

/// Whether `status` is a failure the **harness itself** reached: it was spawned,
/// and what came back was bad. Deliberately narrower than
/// [`history::run_failed`][rf], which also covers the two failures oneharness
/// reaches on the harness's behalf — `skipped` (never spawned) and `spawn_error`
/// (spawned and immediately unspawnable). Those two already say in the status
/// that the candidate never ran, and a `spawn_error` nulls every signal by
/// contract, so a work reading there could only restate the status as a finding.
/// Absence is the honest answer where nothing was observed.
///
/// [rf]: crate::domain::history::run_failed
#[must_use]
pub fn attempted_failure(status: Status) -> bool {
    matches!(
        status,
        Status::Nonzero | Status::Timeout | Status::Cancelled
    )
}

/// The top-level `run` report written to stdout.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct RunReport {
    pub schema_version: String,
    pub oneharness_version: String,
    /// The prompt sent. On an ordinary run this is *the* prompt every result
    /// shares; on a **batch** run (see `batch`) it repeats the first prompt for
    /// back-compat, and each result's own `prompt` field is authoritative.
    pub prompt: String,
    /// The effective top-level model: the first of the fan-out `models` list when
    /// one was given, else the single configured/CLI model, else `null`. Each
    /// result's own `model` is authoritative on a fan-out run.
    pub model: Option<String>,
    /// The model fan-out list this run multiplied over (repeated `--model` /
    /// config `models`), or `null` on an ordinary single-model run. Its presence
    /// is the signal a consumer keys on to read each result's own `model`: in
    /// `parallel` mode `results` holds one entry per (harness, model) pair; in
    /// `fallback` mode the pairs were tried in priority order (harness-major,
    /// model-minor).
    pub models: Option<Vec<String>>,
    /// The session id being continued, when `--resume` was passed; else `null`.
    pub resume: Option<String>,
    /// Whether the resumed session was forked (`--fork`) rather than appended to.
    /// `false` unless `--resume` was given with `--fork`.
    pub fork: bool,
    /// The uniform session handle in play (`--session <name>`), or `null` when
    /// none was requested. Lets a consumer thread one stable name across turns
    /// instead of extracting each harness's native session id. Distinct from the
    /// low-level `resume` field above, which echoes an explicit `--resume` id.
    pub session: Option<SessionReport>,
    /// The normalized approval mode requested for this run (see the README
    /// support matrix). Each harness maps it to its own mechanism.
    pub permission_mode: PermissionMode,
    /// Back-compat convenience: `true` exactly when `permission_mode` is
    /// `bypass`. Retained so existing consumers keep working; new consumers
    /// should read `permission_mode`.
    pub bypass_permissions: bool,
    pub dry_run: bool,
    /// The JSON Schema applied to this run (structured output), or `null` when
    /// none was requested. Echoed so a consumer sees the exact constraint each
    /// result was validated against.
    pub schema: Option<Value>,
    /// Maximum retries allowed per harness under the validate/retry loop; `null`
    /// when no schema was requested.
    pub schema_max_retries: Option<u32>,
    /// Same-prefix batch metadata when this run fanned **one** harness over more
    /// than one prompt; `null` on an ordinary run. Its presence is the signal a
    /// consumer keys on to read each result's own `prompt`.
    pub batch: Option<BatchReport>,
    /// Fallback-mode metadata when this run drove the selected harnesses in
    /// priority order, stopping at the first that ran (`--run-mode fallback`);
    /// `null` on a parallel run (and under `--print-command`, where nothing
    /// executes). Its presence tells a consumer that `results` holds only the
    /// harnesses actually *attempted* — the fallen-through ones in order, then
    /// the one that ran — not every selected harness.
    pub fallback: Option<FallbackReport>,
    /// The parsed `--mock-rules` ruleset this run was intercepted with; `null`
    /// when no mocking was requested. Present so a consumer can tell a mocked
    /// run's report from a clean one without out-of-band state.
    pub mock_rules: Option<Value>,
    /// The spy-log path the mock hook appended tool-call records to (absolute);
    /// `null` when none was requested.
    pub spy_file: Option<String>,
    /// The history session file this run streamed normalized records to
    /// (absolute); `null` when history was not enabled (or under `--print-command`,
    /// where nothing runs). The programmatic handle a consumer captures to read the
    /// session back later with `oneharness history show`.
    pub history_file: Option<String>,
    /// Config files that shaped this run, in layering order (user first,
    /// project last); empty under `--no-config` or when none exist.
    pub config_files: Vec<String>,
    /// Out-of-band turn control for this run (`--control`), or `null` when none
    /// was requested — which is every ordinary run. It records the socket a
    /// separate `oneharness interrupt` process could address, the harness
    /// mechanism behind it, and each request served, so a consumer can tell an
    /// interrupted turn from one that simply ended.
    pub control: Option<ControlReport>,
    pub results: Vec<RunResult>,
}

/// The uniform session handle for a run (`--session`). Present on
/// [`RunReport::session`] only when `--session <name>` was requested.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SessionReport {
    /// The caller's stable handle (`--session <name>`, sanitized for the store).
    pub name: String,
    /// Whether this run created the named session (no prior token) or continued
    /// an existing one.
    pub phase: SessionPhase,
    /// The harness native token now bound to the name: the id resumed on a
    /// continue, or the id captured on a create. `null` only when a create run
    /// exposed no session id (the handle then cannot be continued — a warning is
    /// emitted), or under `--print-command` on a create (nothing ran).
    pub token: Option<String>,
    /// The session store file backing the handle (absolute); the programmatic
    /// handle to the persisted state.
    pub store_file: Option<String>,
}

/// Metadata for a fallback run (harnesses tried in priority order until one
/// runs). Present on [`RunReport::fallback`] only in that mode. The per-harness
/// detail lives in `results`; this block summarizes the outcome so a consumer
/// need not re-derive it from statuses.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct FallbackReport {
    /// The harness that actually ran the task (the run stopped there), or `null`
    /// when no candidate could run at all — every one was a startup failure.
    pub ran: Option<String>,
    /// The candidates fallen through because they could not run the task at all,
    /// in priority order, each with why (`not-installed`, `spawn-error`, `auth`,
    /// `quota`, `session-not-found`, `untrusted-directory`, `input-too-large`,
    /// `model-mismatch`, `rate-limit`, and `model-not-found` — on a model
    /// fan-out, or where the account a harness is signed in with may not use
    /// the model; see
    /// [`crate::domain::fallback::startup_failure_reason`]).
    pub fell_through: Vec<FallThrough>,
    /// Whether the candidate named by `ran` stopped the chain **having shown no
    /// evidence it did the task's work, and with nothing to say why** — its
    /// [`RunResult::work`] read [`RunWork::None`] on a failure no classifier
    /// recognized.
    ///
    /// The chain still stops there, deliberately — decided, not inherited:
    /// empty accounting is the absence of work evidence, not proof nothing was
    /// spent, so handing the candidate on could bill a task twice (the decision
    /// and its reasoning are stated once, at
    /// [`crate::domain::fallback::startup_failure_reason`]). What this flag adds
    /// is the *attribution* — without it a candidate that never got started reads
    /// in the report exactly like one that tried the task and failed it, and the
    /// remaining candidates look untried for a reason nobody can name — and the
    /// failure summary carries the candidate's own words beside it, so the
    /// reader left holding that one line can act on the cause.
    ///
    /// `false` whenever `ran` is `null` (no candidate ran at all — every one is
    /// in `fell_through`, each with its reason).
    // llmlint: ignore[invalid_states_unrepresentable] This additive wire field must stay flat beside `ran` for the stable JSON/SDK contract; both are written together by `fallback_step`, which sets this only where it names the candidate that stopped the chain, and the fallback integration tests pin the pair.
    #[serde(default)]
    pub stopped_without_work: bool,
}

/// One candidate a fallback run fell through, with the reason it could not run.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct FallThrough {
    /// Canonical harness id.
    pub harness: String,
    // Documented on [`FallThroughReason`] itself, not here: a description
    // alongside the `$ref` makes the TypeScript generator inline the union
    // instead of naming the type, and the zod schema then imports a name
    // `contracts.ts` never exported. `status` above carries no field doc for the
    // same reason.
    pub reason: FallThroughReason,
    /// This candidate's own account of why it could not run: its
    /// [`RunResult::error`], copied — one value with one source, not a second
    /// rendering of it. `null` when the candidate said nothing beyond its status.
    ///
    /// So when the provider named the cause in machine-readable terms, what
    /// arrives here is the sentence naming the harness with that object **quoted
    /// inside it** (`… refused the request before running it:
    /// {"input_error_code":"input_too_large",…}`) — the object verbatim, not the
    /// object alone. A reader extracts it; nothing paraphrases it on the way.
    ///
    /// The `reason` above is oneharness's classification and stays a short,
    /// closed token; this is the cause underneath it, carried up so a supervisor
    /// reading only the fallback block never has to re-parse a fallen-through
    /// candidate's raw stdout to learn what the provider already said. Added in
    /// report schema `0.8`.
    #[serde(default)]
    pub detail: Option<String>,
}

/// Metadata for a same-prefix batch run (one harness, N prompts sharing a
/// cacheable prefix). Present on [`RunReport::batch`] only in that mode.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct BatchReport {
    /// How the prompts were scheduled across the parallel runner.
    pub strategy: BatchStrategy,
    /// How many prompts were run (equals `results.len()`).
    pub prompt_count: usize,
    /// Whether the fan-out actually **forked** the warm-up's session to reuse its
    /// cached prefix (`min-tokens` on a fork-capable harness whose warm-up exposed
    /// a session id). `false` for `speed`, for `min-tokens` on a harness that
    /// cannot fork, or when the warm-up exposed no session to fork. When `true`,
    /// the fan-out results' `command` carries the resume/fork flags and their
    /// `usage.cache_read_tokens` reflect the reused prefix.
    pub forked: bool,
}

/// One line of `oneharness run --stream --format json` output (a bare `--stream`
/// prints the readable text view instead).
///
/// Event lines carry normalized actions as they arrive. Exactly one terminal
/// result line carries the complete report unless the consumer closes the
/// stream early. This is an output contract, so deserialization deliberately
/// tolerates additive fields from newer producers.
#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RunStreamEnvelope {
    /// The history address of a candidate before its events.
    #[serde(rename = "run-started")]
    RunStarted(crate::domain::history::RunBegun),
    /// A normalized action observed while the harness is still running.
    Event { event: ActionEvent },
    /// The complete report that terminates a normally consumed stream.
    Result { report: RunReport },
}

impl<'de> Deserialize<'de> for RunStreamEnvelope {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        // Serde's internally-tagged-enum buffer does not support u128, while a
        // RunReport legitimately carries millisecond durations as u128. Decode
        // the small JSON envelope first so the nested report is deserialized by
        // serde_json's number-aware value deserializer instead.
        let value = Value::deserialize(deserializer)?;
        let kind = value.get("type").and_then(Value::as_str).ok_or_else(|| {
            serde::de::Error::custom("run stream envelope is missing string field `type`")
        })?;
        match kind {
            "run-started" => serde_json::from_value(value)
                .map(Self::RunStarted)
                .map_err(serde::de::Error::custom),
            "event" => serde_json::from_value(
                value
                    .get("event")
                    .cloned()
                    .ok_or_else(|| serde::de::Error::custom("event envelope is missing `event`"))?,
            )
            .map(|event| Self::Event { event })
            .map_err(serde::de::Error::custom),
            "result" => {
                serde_json::from_value(value.get("report").cloned().ok_or_else(|| {
                    serde::de::Error::custom("result envelope is missing `report`")
                })?)
                .map(|report| Self::Result { report })
                .map_err(serde::de::Error::custom)
            }
            other => Err(serde::de::Error::custom(format!(
                "unknown run stream envelope type `{other}`"
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_prose_token_is_its_wire_token() {
        // `as_str` exists so a summary can name a status in prose; it must spell
        // the status exactly as the report does, or the sentence names a value a
        // reader cannot find in the JSON beside it. `ALL` is checked against
        // the enum's own generated schema, so a variant added without being
        // listed fails here instead of escaping the token check.
        //
        // A structural assertion, deliberately, in place of an end-to-end one:
        // the summary journey (`crates/oneharness-e2e/tests/cli.rs`, the chain that stops each way)
        // already reads `ok`, `nonzero` and `timeout` off the emitted line, and
        // no chain the CLI can be driven through reports the other four. A
        // `skipped` or `spawn-error` candidate always falls through, named by
        // its fall-through *reason*, never by its status; `planned` is a dry
        // run, which runs no chain; and `cancelled` reaches a summary only from
        // a signal timed into a live chain. Driving that to check the spelling
        // of one token would cost more than the misspelling it prevents, and
        // this check catches the same drift — a token that stops matching the
        // wire — deterministically.
        let rendered = serde_json::to_value(schemars::schema_for!(Status)).expect("serializes");
        let generated: Vec<Value> = rendered["oneOf"]
            .as_array()
            .expect("Status renders as a union of serialized consts")
            .iter()
            .map(|variant| variant["const"].clone())
            .collect();
        let listed: Vec<Value> = Status::ALL
            .into_iter()
            .map(|status| serde_json::to_value(status).expect("a status serializes"))
            .collect();
        assert_eq!(
            listed, generated,
            "Status::ALL must list every variant, in order"
        );
        for status in Status::ALL {
            let wire = serde_json::to_value(status).expect("a status serializes");
            assert_eq!(wire, status.as_str(), "{status:?}");
        }
    }

    fn round_trip(telemetry: ExecutionTelemetry) -> Value {
        let wire = serde_json::to_value(&telemetry).expect("telemetry serializes");
        let back: ExecutionTelemetry =
            serde_json::from_value(wire.clone()).expect("telemetry round-trips");
        assert_eq!(back, telemetry);
        wire
    }

    #[test]
    fn provider_measured_telemetry_round_trips_its_millisecond_splits() {
        // The u128 durations are the reason this enum hand-rolls `Deserialize`:
        // serde's internally-tagged buffer cannot hold one, so a derived impl
        // would serialize fine and then refuse to read its own output back.
        let wire = round_trip(ExecutionTelemetry::ProviderMeasured {
            started_at: "2026-01-01T00:00:00.000Z".parse().unwrap(),
            finished_at: Some("2026-01-01T00:00:12.500Z".parse().unwrap()),
            model_ms: Some(9_000),
            tool_ms: Some(3_500),
            time_to_first_token_ms: Some(420),
        });
        assert_eq!(wire["source"], "provider_measured");
        assert_eq!(wire["model_ms"], 9_000);
        assert_eq!(wire["time_to_first_token_ms"], 420);
    }

    #[test]
    fn the_other_telemetry_variants_round_trip_under_their_own_source() {
        let partial = round_trip(ExecutionTelemetry::PartialInvocation {
            started_at: "2026-01-01T00:00:00Z".parse().expect("canonical instant"),
        });
        assert_eq!(partial["source"], "partial_invocation");
        assert_eq!(partial["started_at"], "2026-01-01T00:00:00Z");

        let observed = round_trip(ExecutionTelemetry::StdoutObserved { tool_ms: 77 });
        assert_eq!(observed["source"], "stdout_observed");
        assert_eq!(observed["tool_ms"], 77);
    }

    #[test]
    fn a_run_instant_accepts_only_millisecond_precision_utc() {
        // The one rule the JsonSchema above also states. Milliseconds are the
        // point: `UtcInstant` would canonicalize these to whole seconds and make
        // a sub-second run read as instantaneous.
        let ok: RunInstant = "2026-01-01T00:00:00.000Z".parse().unwrap();
        assert_eq!(ok.as_str(), "2026-01-01T00:00:00.000Z");
        assert_eq!(ok.to_string(), "2026-01-01T00:00:00.000Z");

        for bad in [
            "",
            "2026-01-01T00:00:00Z",          // no milliseconds
            "2026-01-01T00:00:00.000000Z",   // microseconds: not this shape
            "2026-01-01T00:00:00.000+00:00", // an equivalent offset is still not `Z`
            "2026-01-01T00:00:00.000-05:00", // and a real offset is not UTC at all
            "2026-13-01T00:00:00.000Z",      // shaped like a time, is not one
            "2026-01-01T25:00:00.000Z",
            "2026-01-01 00:00:00.000Z", // the space form loses the length bound
            "2026-01-01T00:00:00.000Z\n", // the trailing newline a `$` would admit
            "2026-01-32T00:00:00.000Z",
            "2026-01-01T00:61:00.000Z",
        ] {
            assert!(
                bad.parse::<RunInstant>().is_err(),
                "accepted {bad:?} as a run instant"
            );
        }
        // A real leap second is a real instant; a date that does not exist is not.
        assert!("2026-06-30T23:59:60.000Z".parse::<RunInstant>().is_ok());
        assert!("2026-02-30T00:00:00.000Z".parse::<RunInstant>().is_err());
        // And the same rule on the deserialization boundary, not just FromStr.
        assert!(serde_json::from_value::<RunInstant>(serde_json::json!("nope")).is_err());
    }

    #[test]
    fn a_clock_read_enters_a_run_instant_already_canonical() {
        // The constructor and `FromStr` have to agree, because this is the
        // arm that never gets parsed: an io-layer clock read goes straight in.
        // If they disagreed, a value no reader would accept could still be
        // written into a measurement, and nothing would say so.
        let minted = RunInstant::from_epoch_millis(1_767_225_600_007);
        assert_eq!(minted.as_str(), "2026-01-01T00:00:00.007Z");
        assert_eq!(minted.as_str().parse::<RunInstant>().unwrap(), minted);

        // Sub-second precision is the whole reason this type is not
        // `UtcInstant`: the milliseconds must survive, not truncate away.
        assert_eq!(
            RunInstant::from_epoch_millis(1_767_225_600_999).as_str(),
            "2026-01-01T00:00:00.999Z"
        );
        assert_eq!(
            RunInstant::from_epoch_millis(0).as_str(),
            "1970-01-01T00:00:00.000Z"
        );
    }

    #[test]
    fn unreadable_telemetry_is_an_error_not_a_guess() {
        // A missing/unknown tag or a variant's own missing field must fail loudly:
        // inventing a source would put a measurement in the report that nothing
        // measured.
        for bad in [
            serde_json::json!({"tool_ms": 5}),
            serde_json::json!({"source": "wall_clock", "tool_ms": 5}),
            serde_json::json!({"source": "provider_measured"}),
            serde_json::json!({"source": "stdout_observed"}),
            serde_json::json!({"source": "partial_invocation", "started_at": "not a time"}),
            serde_json::json!({
                "source": "provider_measured", "started_at": "2026-01-01T00:00:00Z"
            }),
            serde_json::json!({
                "source": "provider_measured",
                "started_at": "2026-01-01T00:00:00.000Z",
                "finished_at": "whenever"
            }),
            // Required-and-nullable, not optional: the schema generated from the
            // same type says so, and a reader that filled these in with `None`
            // would accept telemetry the SDK validators refuse.
            serde_json::json!({
                "source": "provider_measured",
                "started_at": "2026-01-01T00:00:00.000Z",
                "finished_at": null,
                "model_ms": 1
            }),
            serde_json::json!({
                "source": "provider_measured",
                "started_at": "2026-01-01T00:00:00.000Z",
                "finished_at": null,
                "model_ms": "soon",
                "tool_ms": null,
                "time_to_first_token_ms": null
            }),
            serde_json::json!({"source": "stdout_observed", "tool_ms": null}),
        ] {
            assert!(
                serde_json::from_value::<ExecutionTelemetry>(bad.clone()).is_err(),
                "accepted {bad}"
            );
        }
    }

    #[test]
    fn a_result_without_telemetry_still_deserializes() {
        // The field was absent from the wire before schema 0.5, so a report
        // captured by an older producer must still read back.
        let mut wire = serde_json::to_value(sample_result()).expect("result serializes");
        assert!(wire.get("telemetry").is_some(), "0.5 always writes the key");
        wire.as_object_mut().unwrap().remove("telemetry");
        let back: RunResult = serde_json::from_value(wire).expect("pre-0.5 result round-trips");
        assert!(back.telemetry.is_none());
    }

    #[test]
    fn the_cancelled_status_is_spelled_kebab_case_on_the_wire() {
        assert_eq!(
            serde_json::to_value(Status::Cancelled).unwrap(),
            Value::String("cancelled".to_string())
        );
        assert_eq!(
            serde_json::from_value::<Status>(Value::String("cancelled".to_string())).unwrap(),
            Status::Cancelled
        );
    }

    /// A finished run's envelope: how it ended, and what it billed.
    fn finished(status: Status, exit_code: Option<i32>, usage: Usage) -> RunResult {
        RunResult {
            status,
            exit_code,
            usage,
            usage_source: Some("json".to_string()),
            error: None,
            ..sample_result()
        }
    }

    /// The accounting the observed run reported: a 94-minute turn that billed
    /// $12.11.
    fn billed() -> Usage {
        Usage {
            input_tokens: Some(830_514),
            output_tokens: Some(42_233),
            cost_usd: Some(12.11),
            ..Usage::default()
        }
    }

    #[test]
    fn a_completed_billed_run_drops_a_refusal_its_envelope_contradicts() {
        // The run oneharness published for a turn that finished and cost $12.11:
        // `rate_limit`, scanned off an intermediate record the harness retried
        // past. The envelope refutes it, so the classification does not survive —
        // and its source goes with it rather than dangling.
        for kind in FailureKind::ALL.into_iter().filter(|k| k.is_refusal()) {
            let mut result = finished(Status::Ok, Some(0), billed());
            result.failure_kind = Some(kind);
            result.failure_kind_source = Some("stdout".to_string());
            let reconciled = result.with_work_evidence();
            assert_eq!(
                reconciled.failure_kind, None,
                "{kind:?} must not survive a completed, billed run"
            );
            assert_eq!(reconciled.failure_kind_source, None);
            // A clean run needs no work reading — `failure_kind` is gone, but the
            // status was never a failure to explain.
            assert_eq!(reconciled.work, None);
        }
    }

    #[test]
    fn a_deferred_tool_dead_end_survives_its_own_clean_billed_exit() {
        // `tool_deferred` is the one kind that is not a refusal: the turn
        // completed and the finding is about what it produced, so a clean, billed
        // exit is the shape it describes rather than one that contradicts it.
        // Clearing it here would put the Claude Code bridge dead-end (issue
        // #1114) back to looking like an ordinary empty answer.
        let mut result = finished(Status::Ok, Some(0), billed());
        result.failure_kind = Some(FailureKind::ToolDeferred);
        result.failure_kind_source = Some("stdout".to_string());
        let reconciled = result.with_work_evidence();
        assert_eq!(reconciled.failure_kind, Some(FailureKind::ToolDeferred));
        assert_eq!(reconciled.failure_kind_source.as_deref(), Some("stdout"));
    }

    #[test]
    fn a_run_that_did_not_complete_and_be_billed_keeps_its_refusal() {
        // Each half of `completed_run_that_did_work` is load-bearing, so each is
        // dropped in turn: a run that failed, one that exited non-zero, and one
        // that exited cleanly having done nothing all keep the best account they
        // have of why. Only all three together refute a refusal.
        for (label, status, exit_code, usage) in [
            ("failed the task", Status::Nonzero, Some(1), billed()),
            ("timed out", Status::Timeout, None, billed()),
            ("was cancelled", Status::Cancelled, None, billed()),
            (
                "exited non-zero on a clean status",
                Status::Ok,
                Some(1),
                billed(),
            ),
            ("billed nothing", Status::Ok, Some(0), Usage::default()),
            (
                "billed zero",
                Status::Ok,
                Some(0),
                Usage {
                    input_tokens: Some(0),
                    output_tokens: Some(0),
                    cost_usd: Some(0.0),
                    ..Usage::default()
                },
            ),
        ] {
            let mut result = finished(status, exit_code, usage);
            result.failure_kind = Some(FailureKind::RateLimit);
            result.failure_kind_source = Some("stderr".to_string());
            let reconciled = result.with_work_evidence();
            assert_eq!(
                reconciled.failure_kind,
                Some(FailureKind::RateLimit),
                "a run that {label} keeps its classification"
            );
            assert_eq!(reconciled.failure_kind_source.as_deref(), Some("stderr"));
        }
    }

    #[test]
    fn a_completed_run_with_only_tool_evidence_also_refutes_a_refusal() {
        // Work evidence is `RunWork`'s one definition, not a second reading of
        // "billed": a harness that reported no accounting at all but recorded a
        // tool call did the task just as decisively.
        let mut result = finished(Status::Ok, Some(0), Usage::default());
        result.events = Some(vec![ActionEvent {
            kind: "tool_call".to_string(),
            name: Some("Bash".to_string()),
            input: None,
            output: None,
            index: 0,
            tool_call_id: None,
            started_at: None,
            finished_at: None,
            duration_ms: None,
            status: None,
            timing_source: None,
        }]);
        result.failure_kind = Some(FailureKind::RateLimit);
        assert_eq!(result.with_work_evidence().failure_kind, None);
    }

    fn sample_result() -> RunResult {
        RunResult {
            harness: "claude-code".to_string(),
            variant: None,
            harness_id: "claude-code".to_string(),
            bin: "claude".to_string(),
            available: true,
            status: Status::Cancelled,
            prompt: None,
            model: None,
            observed_model: None,
            exit_code: None,
            duration_ms: Some(1_200),
            telemetry: Some(ExecutionTelemetry::StdoutObserved { tool_ms: 12 }),
            command: vec!["claude".to_string()],
            output_format: OutputFormat::Json,
            text: None,
            text_source: None,
            usage: Usage::default(),
            usage_source: None,
            session_id: None,
            events: None,
            events_source: None,
            structured: None,
            schema_valid: None,
            schema_attempts: None,
            schema_error: None,
            failure_kind: None,
            work: None,
            failure_kind_source: None,
            stdout: String::new(),
            stderr: String::new(),
            error: Some("cancelled".to_string()),
        }
    }
}
