//! Standardized, cross-harness run history: the shape of one history record and
//! the pure helpers that name and time-stamp it. This is a pure module — it
//! builds the record and formats strings, but reads no clock or filesystem. The
//! actual streaming to disk, the `SystemTime` reads that mint the session id and
//! timestamps, and the reading/listing of history files all live in
//! `src/io/history.rs`.
//!
//! The history file is its own output contract, independent of the run report's:
//! it carries its own [`SCHEMA_VERSION`], and normalized signals only (never the
//! raw stdout/stderr — a consumer that needs the bytes re-runs, or reads the
//! harness's own transcript). Every field mirrors a [`crate::domain::report`]
//! signal, so a history record reads like a report result frozen in time.

use std::borrow::Cow;
use std::collections::BTreeMap;
use std::fmt;
use std::path::Path;
use std::str::FromStr;

use schemars::{JsonSchema, Schema, SchemaGenerator};
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;
use thiserror::Error;
use uuid::{Uuid, Variant};

use crate::domain::events::{ActionEvent, ToolCallStatus};
use crate::domain::fallback::RunWork;
use crate::domain::harness::HarnessIdentity;
use crate::domain::mode::PermissionMode;
use crate::domain::report::{attempted_failure, unclassified_failure, RunResult, Status};
use crate::domain::signals::{FailureKind, Usage};
use crate::domain::usage::UtcInstant;

/// Bumped when the history record shape changes in a way a consumer must notice.
/// Independent of [`crate::domain::report::SCHEMA_VERSION`] — the history file and
/// the run report are separate contracts and version on their own cadence.
///
/// A record's `schema_version` is the **oldest reader that can understand it**,
/// not the version of the writer that produced it: a provider-measured run still
/// declares [`PREVIOUS_CURRENT_SCHEMA_VERSION`] because nothing in it needs a
/// newer reader. That is what lets an additive field ship without rewriting the
/// shape of every record — and what makes each version constant below the exact
/// gate for the one field it introduced.
pub const SCHEMA_VERSION: &str = "1.11";
/// First history schema carrying an opening run line.
pub const FIRST_RUN_STARTED_SCHEMA_VERSION: &str = "1.11";
/// Opening lines advance independently: existing event lines stay readable by
/// v1.10 readers, which skip the new opening line.
pub const EVENT_SCHEMA_VERSION: &str = "1.10";
/// v1.10 introduced the `app_server_lost` failure kind: the shared Codex
/// app-server a pooled dispatch's turn ran on went away under the turn. A v1.9
/// reader's `failure_kind` enum has no such value.
pub const FIRST_APP_SERVER_LOST_SCHEMA_VERSION: &str = "1.10";
/// First history schema that accepts `server_overloaded`.
pub const FIRST_SERVER_OVERLOADED_SCHEMA_VERSION: &str = "1.9";
/// v1.8 introduced `observed_model` — the model the harness itself reported the
/// conversation would run under (see [`HistoryRecord::observed_model`]) — and
/// the `model_mismatch` failure kind, the refusal oneharness answers when that
/// report and the requested model differ. A v1.7 reader has neither the field
/// nor the enum value, so a record carrying either declares this version.
pub const FIRST_MODEL_OBSERVATION_SCHEMA_VERSION: &str = "1.8";
/// v1.7 introduced `work` — what a run that failed with **nothing to classify**
/// has to show for itself (see [`HistoryRecord::work`]). A v1.6 reader has no
/// such field, so a record carrying it declares this version rather than letting
/// an older reader see the same unexplained failure it always saw.
pub const FIRST_WORK_EVIDENCE_SCHEMA_VERSION: &str = "1.7";
/// v1.6 introduced the two precondition failure kinds — `untrusted_directory`
/// and `input_too_large`, the refusals a harness reaches before it makes the
/// request at all. A v1.5 reader's `failure_kind` enum has neither value, so it
/// refuses such a record rather than misreading it. One constant for both
/// because they arrived together; a kind added later needs its own.
pub const FIRST_PRECONDITION_SCHEMA_VERSION: &str = "1.6";
/// v1.5 introduced the `session_not_found` failure kind — the refusal a harness
/// returns when asked to continue a session its identity has never seen. A v1.4
/// reader's `failure_kind` enum has no such value, so it refuses the record
/// rather than misreading it.
pub const FIRST_SESSION_NOT_FOUND_SCHEMA_VERSION: &str = "1.5";
/// v1.4 introduced the `cancelled` run status — a record a v1.3 reader would
/// refuse rather than misread, since its `status` enum has no such value. Spelled
/// literally, like every constant here: aliasing [`SCHEMA_VERSION`] would move
/// this "first version that understood it" on the next bump.
pub const FIRST_CANCELLED_SCHEMA_VERSION: &str = "1.4";
/// v1.3 introduced the normalized failure `error` text and invocation-bounds-only
/// timing, so both name the same version — and both must, because an older
/// reader refuses either shape rather than misreading it.
pub const FIRST_ERROR_SCHEMA_VERSION: &str = "1.3";
pub const FIRST_PARTIAL_TIMING_SCHEMA_VERSION: &str = "1.3";
/// v1.2 introduced stdout-observed tool timing (`observed_tool_ms`) and the
/// `timing_source` provenance it puts on events.
pub const OBSERVED_TIMING_SCHEMA_VERSION: &str = "1.2";
pub const PREVIOUS_CURRENT_SCHEMA_VERSION: &str = "1.1";
pub(crate) const FIRST_EVENT_SCHEMA_VERSION: &str = "1.0";

/// Every event-sourced history version this build reads, oldest first. Order is
/// the contract: a field introduced in version N is legible to N and everything
/// after it, which is what [`version_at_least`] answers.
pub(crate) const READABLE_SCHEMA_VERSIONS: [&str; 12] = [
    FIRST_EVENT_SCHEMA_VERSION,
    PREVIOUS_CURRENT_SCHEMA_VERSION,
    OBSERVED_TIMING_SCHEMA_VERSION,
    FIRST_ERROR_SCHEMA_VERSION,
    FIRST_CANCELLED_SCHEMA_VERSION,
    FIRST_SESSION_NOT_FOUND_SCHEMA_VERSION,
    FIRST_PRECONDITION_SCHEMA_VERSION,
    FIRST_WORK_EVIDENCE_SCHEMA_VERSION,
    FIRST_MODEL_OBSERVATION_SCHEMA_VERSION,
    FIRST_SERVER_OVERLOADED_SCHEMA_VERSION,
    FIRST_APP_SERVER_LOST_SCHEMA_VERSION,
    FIRST_RUN_STARTED_SCHEMA_VERSION,
];

fn version_rank(version: &str) -> Option<usize> {
    READABLE_SCHEMA_VERSIONS
        .iter()
        .position(|known| *known == version)
}

/// Every readable version at or after `minimum`, oldest first — the versions a
/// field introduced in `minimum` may legitimately appear at.
///
/// Public because the generated SDK schemas describe such a field by listing
/// exactly these versions. Deriving the list from the private
/// `READABLE_SCHEMA_VERSIONS` list is what keeps a *new* version from silently
/// narrowing an older field's legality: a hand-written pair like `[introduced,
/// current]` stops covering the versions in between the moment `current` moves.
#[must_use]
pub fn versions_from(minimum: &str) -> Vec<&'static str> {
    READABLE_SCHEMA_VERSIONS
        .iter()
        .copied()
        .filter(|version| version_at_least(version, minimum))
        .collect()
}

/// Whether `version` is a readable version at or after `minimum`. An unreadable
/// version answers `false` rather than guessing an ordering for it.
fn version_at_least(version: &str, minimum: &str) -> bool {
    match (version_rank(version), version_rank(minimum)) {
        (Some(actual), Some(minimum)) => actual >= minimum,
        _ => false,
    }
}

/// The legacy record contract accepted by the migration reader.
pub const LEGACY_SCHEMA_VERSION: &str = "0.1";
const PREVIOUS_SCHEMA_VERSION: &str = "0.2";
const LEGACY_RECORD_SCHEMA_VERSION: &str = "0.3";

/// The whole-record versions whose events ended at `index`, before
/// `LEGACY_RECORD_SCHEMA_VERSION` (v0.3) added the lifecycle fields — see the
/// legacy `LegacyActionEvent` shape. Exported so the generated SDK schemas
/// describe that one legacy shape from this source rather than restating its
/// versions.
pub const PRE_LIFECYCLE_RECORD_VERSIONS: [&str; 2] =
    [LEGACY_SCHEMA_VERSION, PREVIOUS_SCHEMA_VERSION];

/// One event-sourced history JSONL line.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum HistoryLine {
    #[serde(rename = "run-started")]
    RunStarted(HistoryRunStarted),
    Event(HistoryEventLine),
    Run(HistoryRunRecord),
}

impl<'de> Deserialize<'de> for HistoryLine {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let mut value = Value::deserialize(deserializer)?;
        let line_type = value
            .as_object_mut()
            .and_then(|object| object.remove("type"))
            .and_then(|value| value.as_str().map(str::to_owned));
        match line_type.as_deref() {
            Some("run-started") => {
                let started: HistoryRunStarted =
                    serde_json::from_value(value).map_err(serde::de::Error::custom)?;
                Ok(Self::RunStarted(started))
            }
            Some("event") => {
                let event = serde_json::from_value::<HistoryEventLine>(value)
                    .map_err(serde::de::Error::custom)?;
                if event.valid() {
                    Ok(Self::Event(event))
                } else {
                    Err(serde::de::Error::custom(
                        "invalid history schema v1.0 event line",
                    ))
                }
            }
            Some("run") => {
                let run = serde_json::from_value::<HistoryRunRecord>(value)
                    .map_err(serde::de::Error::custom)?;
                if run.valid() {
                    Ok(Self::Run(run))
                } else {
                    Err(serde::de::Error::custom(
                        "invalid history schema v1.0 run line",
                    ))
                }
            }
            _ => Err(serde::de::Error::custom("invalid history schema v1.0 line")),
        }
    }
}

/// The complete prompt persisted before a candidate starts, including runs that
/// never reach their closing record.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct HistoryRunStarted {
    pub schema_version: RunStartedSchemaVersion,
    pub history_id: HistoryId,
    #[schemars(schema_with = "opening_harness_schema")]
    pub harness_id: HarnessIdentity,
    pub prompt: String,
    pub started: UtcInstant,
}

/// The schema version of an opening line; its only constructible value is 1.11.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RunStartedSchemaVersion;

/// An invalid opening schema version or history address component.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum HistoryAddressError {
    #[error("unsupported run-started schema version")]
    SchemaVersion,
    #[error("history_dir must be an absolute UTF-8 store path")]
    AbsoluteRoot,
    #[error("history path component must be nonempty ASCII [A-Za-z0-9._-], excluding . and ..")]
    Component,
}

impl TryFrom<String> for RunStartedSchemaVersion {
    type Error = HistoryAddressError;
    fn try_from(value: String) -> Result<Self, Self::Error> {
        if value == FIRST_RUN_STARTED_SCHEMA_VERSION {
            Ok(Self)
        } else {
            Err(HistoryAddressError::SchemaVersion)
        }
    }
}
impl RunStartedSchemaVersion {
    pub fn as_str(&self) -> &'static str {
        FIRST_RUN_STARTED_SCHEMA_VERSION
    }
}
impl fmt::Display for RunStartedSchemaVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}
impl Serialize for RunStartedSchemaVersion {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}
impl<'de> Deserialize<'de> for RunStartedSchemaVersion {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Self::try_from(String::deserialize(deserializer)?).map_err(serde::de::Error::custom)
    }
}
impl JsonSchema for RunStartedSchemaVersion {
    fn inline_schema() -> bool {
        true
    }
    fn schema_name() -> Cow<'static, str> {
        Cow::Borrowed("RunStartedSchemaVersion")
    }
    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        schemars::json_schema!({"type": "string", "const": FIRST_RUN_STARTED_SCHEMA_VERSION})
    }
}

/// One safe project directory name or session file stem.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "String")]
pub struct HistoryPathComponent(String);

impl TryFrom<String> for HistoryPathComponent {
    type Error = HistoryAddressError;
    fn try_from(value: String) -> Result<Self, Self::Error> {
        if !value.is_empty()
            && !matches!(value.as_str(), "." | "..")
            && value
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
        {
            Ok(Self(value))
        } else {
            Err(HistoryAddressError::Component)
        }
    }
}
impl HistoryPathComponent {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}
impl fmt::Display for HistoryPathComponent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}
impl AsRef<Path> for HistoryPathComponent {
    fn as_ref(&self) -> &Path {
        Path::new(self.as_str())
    }
}
impl std::ops::Deref for HistoryPathComponent {
    type Target = str;
    fn deref(&self) -> &str {
        self.as_str()
    }
}
impl JsonSchema for HistoryPathComponent {
    fn inline_schema() -> bool {
        true
    }
    fn schema_name() -> Cow<'static, str> {
        Cow::Borrowed("HistoryPathComponent")
    }
    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        schemars::json_schema!({"type": "string", "minLength": 1,
            "not": {"pattern": r"(^\.\.?$|[^A-Za-z0-9._-])"}})
    }
}

/// An absolute history store path, readable across Unix and Windows producers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "std::path::PathBuf")]
pub struct AbsoluteHistoryDir(std::path::PathBuf);

impl TryFrom<std::path::PathBuf> for AbsoluteHistoryDir {
    type Error = HistoryAddressError;
    fn try_from(value: std::path::PathBuf) -> Result<Self, Self::Error> {
        let text = value.to_str().ok_or(HistoryAddressError::AbsoluteRoot)?;
        let bytes = text.as_bytes();
        let drive = bytes.len() >= 3
            && bytes[0].is_ascii_alphabetic()
            && bytes[1] == b':'
            && matches!(bytes[2], b'/' | b'\\');
        let unc = text.strip_prefix(r"\\").is_some_and(|rest| {
            let mut parts = rest.split(['/', '\\']);
            parts.next().is_some_and(|part| !part.is_empty())
                && parts.next().is_some_and(|part| !part.is_empty())
        });
        if (text.starts_with('/') || drive || unc) && !text.chars().any(char::is_control) {
            Ok(Self(value))
        } else {
            Err(HistoryAddressError::AbsoluteRoot)
        }
    }
}
impl AbsoluteHistoryDir {
    pub fn as_path(&self) -> &Path {
        &self.0
    }
}
impl std::ops::Deref for AbsoluteHistoryDir {
    type Target = Path;
    fn deref(&self) -> &Path {
        self.as_path()
    }
}
impl fmt::Display for AbsoluteHistoryDir {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.display().fmt(f)
    }
}
impl JsonSchema for AbsoluteHistoryDir {
    fn inline_schema() -> bool {
        true
    }
    fn schema_name() -> Cow<'static, str> {
        Cow::Borrowed("AbsoluteHistoryDir")
    }
    fn json_schema(_: &mut SchemaGenerator) -> Schema {
        schemars::json_schema!({"type": "string", "pattern": r"^(/|[A-Za-z]:[\\/]|\\\\[^\\/]+[\\/][^\\/]+)",
            "not": {"pattern": r"[\u0000-\u001F\u007F-\u009F]"}})
    }
}

fn opening_harness_schema(_: &mut SchemaGenerator) -> Schema {
    let ids = crate::domain::harness::all()
        .iter()
        .map(|spec| spec.id)
        .collect::<Vec<_>>()
        .join("|");
    let pattern = format!(
        "^({ids})(:{})?$",
        crate::domain::config::VARIANT_NAME_PATTERN
    );
    schemars::json_schema!({"type": "string", "pattern": pattern,
        "not": {"pattern": "[^A-Za-z0-9:_-]"}})
}

/// A candidate's address in the history store, delivered before its events.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct RunBegun {
    #[schemars(schema_with = "opening_harness_schema")]
    pub harness_id: HarnessIdentity,
    pub history_id: HistoryId,
    pub history_dir: AbsoluteHistoryDir,
    pub history_project: HistoryPathComponent,
    pub history_session: HistoryPathComponent,
}

/// One normalized action observed during a run.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct HistoryEventLine {
    // llmlint: ignore[invalid_states_unrepresentable] Schema versions are strings throughout the report, session, and history contracts, and history migration must inspect several legacy string versions; `valid()` is the intentional wire-boundary check, so a bespoke current-version type would conflict with that established representation.
    pub schema_version: String,
    pub run_id: HistoryId,
    pub harness: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    // llmlint: ignore[invalid_states_unrepresentable] History v1.0 compatibility requires an optional string here; current writers derive it from a validated composed selector and materialization validates/normalizes the legacy tuple.
    pub variant: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    // llmlint: ignore[invalid_states_unrepresentable] This field is optional specifically to read v1.0 event lines; current writers always derive it with base/variant from one composed id, and event-stream integration coverage asserts all three.
    pub harness_id: Option<String>,
    pub event: ActionEvent,
    /// The name of the session this line belongs to — the `name` its run's
    /// closing `run` line will carry — so a run still in progress (events but
    /// no closing line yet) can be listed and followed by name. Omitted when
    /// absent: lines written before it existed carry none, and every reader
    /// that predates it ignores it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_name: Option<HistorySessionName>,
}

impl HistoryEventLine {
    pub(crate) fn valid(&self) -> bool {
        version_rank(&self.schema_version).is_some()
            && (self.event.timing_source.is_none()
                || version_at_least(&self.schema_version, OBSERVED_TIMING_SCHEMA_VERSION))
            && identity_fields_valid(
                &self.harness,
                self.variant.as_deref(),
                self.harness_id.as_deref(),
            )
    }
}

/// The terminal summary for one harness run.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct HistoryRunRecord {
    // llmlint: ignore[invalid_states_unrepresentable] Keep the established string version representation used by every serialized contract and required by legacy history migration; deserialization validates it before exposing a current record.
    pub schema_version: String,
    pub history_id: HistoryId,
    pub session: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "HistoryLabels::is_empty")]
    pub labels: HistoryLabels,
    pub project: String,
    pub timestamp: String,
    // llmlint: ignore[invalid_states_unrepresentable] These legacy-compatible wire fields must deserialize v1.0 records where the composed field is absent; `materialize` derives the composed identity and current writers copy the already-normalized run identity, with cross-contract tests pinning consistency.
    pub harness: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    // llmlint: ignore[invalid_states_unrepresentable] The v1.0 wire contract requires this optional string; current writers copy the validated run identity and deserialization/materialization validates the supported schema before exposure.
    pub variant: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    // llmlint: ignore[invalid_states_unrepresentable] This remains optional to read v1.0 records; v1.1 writers always derive it with base/variant from one composed selector and round-trip tests pin consistency.
    pub harness_id: Option<String>,
    pub model: Option<String>,
    /// The model the harness itself reported it would run under (see
    /// [`HistoryRecord::observed_model`]). Omitted on the wire when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observed_model: Option<String>,
    pub prompt: String,
    pub permission_mode: PermissionMode,
    pub status: Status,
    pub exit_code: Option<i32>,
    pub duration_ms: Option<u128>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub started_at: Option<String>,
    pub finished_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model_ms: Option<u128>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_ms: Option<u128>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub time_to_first_token_ms: Option<u128>,
    /// Union of tool intervals derived from stdout pipe-read observations.
    /// This is deliberately separate from provider-measured `tool_ms`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    // llmlint: ignore[invalid_states_unrepresentable] This additive published wire field must remain flat beside legacy provider timing fields; `valid()` rejects mixed states and constructors derive exactly one timing mode.
    pub observed_tool_ms: Option<u128>,
    pub text: Option<String>,
    pub text_source: Option<String>,
    pub usage: Usage,
    pub session_id: Option<String>,
    pub failure_kind: Option<FailureKind>,
    /// Work evidence for a failure nothing classified (see
    /// [`HistoryRecord::work`]). Omitted on the wire when absent.
    // llmlint: ignore[invalid_states_unrepresentable] This wire field must stay flat beside `status`/`failure_kind` to read and write the published JSONL contract; `valid()` refuses a line whose combination is not one this crate writes, and the shared contract matrix holds the same rule across the Rust, Node and Python validators.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub work: Option<RunWork>,
    /// Normalized failure text for a run that did not succeed (see
    /// [`HistoryRecord::error`]). Omitted on the wire when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<FailureText>,
}

impl HistoryRunRecord {
    pub(crate) fn valid(&self) -> bool {
        if version_rank(&self.schema_version).is_none() {
            return false;
        }
        if !identity_fields_valid(
            &self.harness,
            self.variant.as_deref(),
            self.harness_id.as_deref(),
        ) {
            return false;
        }
        if !status_version_valid(&self.schema_version, self.status)
            || !failure_kind_version_valid(&self.schema_version, self.failure_kind)
            || !error_text_valid(
                &self.schema_version,
                self.error.as_ref(),
                self.status,
                self.failure_kind,
            )
            || !work_evidence_valid(&self.schema_version, self.work, self.status)
            || !observed_model_valid(&self.schema_version, self.observed_model.as_deref())
        {
            return false;
        }
        if self.observed_tool_ms.is_some() {
            return version_at_least(&self.schema_version, OBSERVED_TIMING_SCHEMA_VERSION)
                && self.duration_ms.is_some()
                && self.started_at.is_none()
                && self.finished_at.is_none()
                && self.model_ms.is_none()
                && self.tool_ms.is_none()
                && self.time_to_first_token_ms.is_none();
        }
        match (
            self.started_at.as_deref(),
            self.finished_at.as_deref(),
            self.model_ms,
            self.tool_ms,
            self.time_to_first_token_ms,
        ) {
            (None, None, None, None, None) => true,
            (Some(started_at), finished_at, Some(model_ms), Some(tool_ms), _) => {
                !started_at.is_empty()
                    && self
                        .duration_ms
                        .is_some_and(|duration| model_ms.saturating_add(tool_ms) <= duration)
                    && (!requires_provider_finish(self.status) || finished_at.is_some())
            }
            // Invocation bounds with no split derived from them, and no provider
            // finish either: what a run cut short leaves, legible only on a run
            // that was cut short or classified as failed, and only to a reader
            // that knows the shape.
            (Some(started_at), None, None, None, time_to_first_token_ms) => {
                (run_failed(self.status) || self.failure_kind.is_some())
                    && version_at_least(&self.schema_version, FIRST_PARTIAL_TIMING_SCHEMA_VERSION)
                    && partial_trace_valid(started_at, time_to_first_token_ms, self.duration_ms)
            }
            _ => false,
        }
    }

    /// Split the terminal portion from the familiar materialized record.
    pub fn from_record(record: &HistoryRecord) -> Self {
        Self {
            schema_version: record.schema_version.clone(),
            history_id: record.history_id,
            session: record.session.clone(),
            name: record.name.clone(),
            labels: record.labels.clone(),
            project: record.project.clone(),
            timestamp: record.timestamp.clone(),
            harness: record.harness.clone(),
            variant: record.variant.clone(),
            harness_id: Some(record.harness_id.clone()),
            model: record.model.clone(),
            observed_model: record.observed_model.clone(),
            prompt: record.prompt.clone(),
            permission_mode: record.permission_mode,
            status: record.status,
            exit_code: record.exit_code,
            duration_ms: record.duration_ms,
            started_at: record.started_at.clone(),
            finished_at: record.finished_at.clone(),
            model_ms: record.model_ms,
            tool_ms: record.tool_ms,
            time_to_first_token_ms: record.time_to_first_token_ms,
            observed_tool_ms: record.observed_tool_ms,
            text: record.text.clone(),
            text_source: record.text_source.clone(),
            usage: record.usage.clone(),
            session_id: record.session_id.clone(),
            failure_kind: record.failure_kind,
            work: record.work,
            error: record.error.clone(),
        }
    }

    /// Rebuild the stable per-run presentation object from event-sourced lines.
    pub fn materialize(self, events: Vec<ActionEvent>) -> HistoryRecord {
        let harness_id = self
            .harness_id
            .clone()
            .unwrap_or_else(|| self.harness.clone());
        HistoryRecord {
            schema_version: self.schema_version,
            history_id: self.history_id,
            session: self.session,
            name: self.name,
            labels: self.labels,
            project: self.project,
            timestamp: self.timestamp,
            harness: self.harness,
            variant: self.variant,
            harness_id,
            model: self.model,
            observed_model: self.observed_model,
            prompt: self.prompt,
            permission_mode: self.permission_mode,
            status: self.status,
            exit_code: self.exit_code,
            duration_ms: self.duration_ms,
            started_at: self.started_at,
            finished_at: self.finished_at,
            model_ms: self.model_ms,
            tool_ms: self.tool_ms,
            time_to_first_token_ms: self.time_to_first_token_ms,
            observed_tool_ms: self.observed_tool_ms,
            text: self.text,
            text_source: self.text_source,
            usage: self.usage,
            session_id: self.session_id,
            events: (!events.is_empty()).then_some(events),
            failure_kind: self.failure_kind,
            work: self.work,
            error: self.error,
        }
        .without_untimed_result_status()
    }
}

/// Label lengths are bounded in *characters* — Unicode code points. That is the
/// unit JSON Schema's `maxLength` measures in, and therefore the one unit both
/// the runtime checks below and every generated SDK validator can agree on; a
/// byte bound is not expressible in the schema the SDKs are generated from.
const LABEL_KEY_MAX: usize = 64;
const LABEL_VALUE_MAX: usize = 256;

/// Canonical hyphenated UUID text is always exactly this many characters. The
/// bound is load-bearing, not decoration: a regex `$` also matches *before* a
/// trailing newline in some engines the SDKs validate with (Python's `re`, which
/// drives the generated Python schemas), so pinning the length is what keeps
/// `"<uuid>\n"` from slipping past [`UUID_PATTERN`] there but not here.
const UUID_LEN: usize = 36;
/// Canonical hyphenated UUID text carrying the RFC 4122 variant (`[89abAB]`) and
/// a defined version (`[1-8]`) — exactly what [`HistoryId::from_str`] accepts.
const UUID_PATTERN: &str =
    "^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[1-8][0-9a-fA-F]{3}-[89abAB][0-9a-fA-F]{3}-[0-9a-fA-F]{12}$";
/// A label key begins with a letter or digit...
const LABEL_KEY_START_PATTERN: &str = "^[A-Za-z0-9]";
/// ...and carries nothing outside the permitted set. Stated as a *forbidden*
/// unanchored search rather than an anchored allow-list so the schema needs no
/// `$`, whose meaning differs across the regex engines the SDKs validate with
/// (see [`UUID_LEN`]).
const LABEL_KEY_FORBIDDEN_PATTERN: &str = "[^A-Za-z0-9._-]";
/// Every character Rust classifies as control (Unicode `Cc`): C0, DEL, *and* the
/// C1 block. Kept exactly in step with [`char::is_control`], which is what the
/// runtime checks, and forbidden by search for the same reason as
/// [`LABEL_KEY_FORBIDDEN_PATTERN`].
const LABEL_VALUE_FORBIDDEN_PATTERN: &str = "[\\u0000-\\u001f\\u007f-\\u009f]";

/// The error returned when text is not a canonical [`HistoryId`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
#[error(
    "must be a canonical hyphenated UUID with a version between 1 and 8 and the RFC 4122 variant"
)]
pub struct HistoryIdError;

/// A validated history-record identifier. New records are minted as UUIDv7 in
/// the I/O layer; legacy records are assigned deterministic UUIDv5 values while
/// being read, so the same v0.1 line always receives the same stable id.
///
/// Parsed text must be the canonical hyphenated spelling carrying the RFC 4122
/// variant and a defined version — both of which every minted id (v5 and v7)
/// satisfies, and both of which the schema's `UUID_PATTERN` promises consumers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub struct HistoryId(Uuid);

impl HistoryId {
    /// Wrap an already-generated UUID. The I/O layer uses this after its clock /
    /// randomness read; pure migration uses [`Self::legacy`].
    #[must_use]
    pub fn from_uuid(value: Uuid) -> Self {
        Self(value)
    }

    /// Deterministically identify one legacy record from a stable source key.
    #[must_use]
    pub fn legacy(stable_key: &[u8]) -> Self {
        Self(Uuid::new_v5(&Uuid::NAMESPACE_OID, stable_key))
    }

    /// The underlying UUID.
    #[must_use]
    pub fn as_uuid(self) -> Uuid {
        self.0
    }
}

impl fmt::Display for HistoryId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl FromStr for HistoryId {
    type Err = HistoryIdError;

    /// Accept exactly what the schema's `UUID_PATTERN` promises.
    /// `Uuid::parse_str` is laxer on both counts: it also takes the simple,
    /// braced, and URN spellings, and it reads any variant or version bits —
    /// including the nil UUID. Comparing against the parsed value's own
    /// canonical rendering is what rejects the alternate spellings while still
    /// accepting upper-case hex.
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let uuid = Uuid::parse_str(value).map_err(|_| HistoryIdError)?;
        if uuid.hyphenated().to_string() != value.to_ascii_lowercase() {
            return Err(HistoryIdError);
        }
        if !matches!(uuid.get_variant(), Variant::RFC4122)
            || !(1..=8).contains(&uuid.get_version_num())
        {
            return Err(HistoryIdError);
        }
        Ok(Self(uuid))
    }
}

impl<'de> Deserialize<'de> for HistoryId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        String::deserialize(deserializer)?
            .parse()
            .map_err(serde::de::Error::custom)
    }
}

impl JsonSchema for HistoryId {
    fn inline_schema() -> bool {
        true
    }

    fn schema_name() -> Cow<'static, str> {
        Cow::Borrowed("HistoryId")
    }

    fn json_schema(_generator: &mut SchemaGenerator) -> Schema {
        schemars::json_schema!({
            "type": "string",
            "minLength": UUID_LEN,
            "maxLength": UUID_LEN,
            "pattern": UUID_PATTERN,
        })
    }
}

/// A non-empty character run outside the sanitized session-name alphabet, or a
/// dash that leads, trails, or doubles — the shapes [`sanitize_name`] never
/// produces. Stated as a forbidden unanchored search (see
/// [`LABEL_KEY_FORBIDDEN_PATTERN`]) so every SDK regex engine agrees.
const SESSION_NAME_FORBIDDEN_PATTERN: &str = "[^a-z0-9-]|^-|-$|--";

/// The error returned when text is not a sanitized [`HistorySessionName`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
#[error(
    "must be a non-empty session name of lowercase ASCII letters and digits joined by single dashes"
)]
pub struct HistorySessionNameError;

/// A session name in exactly the shape [`sanitize_name`] and [`session_name`]
/// produce: lowercase ASCII alphanumeric words joined by single dashes. It is
/// read back from history files and used to select a session, so text that no
/// writer could have produced — empty, upper-case, spaced, or path-like — is
/// refused at the boundary rather than matched.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
#[serde(transparent)]
pub struct HistorySessionName(String);

impl HistorySessionName {
    /// Sanitize arbitrary text (a `--history-name`, or a derived name) into a
    /// valid session name; see [`sanitize_name`].
    #[must_use]
    pub fn sanitize(raw: &str) -> Self {
        Self(sanitize_name(raw))
    }

    /// The name as text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for HistorySessionName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl FromStr for HistorySessionName {
    type Err = HistorySessionNameError;

    /// Accept exactly the fixed points of [`sanitize_name`] other than its
    /// empty-input fallback — what the JSON Schema's forbidden pattern plus a
    /// minimum length of one also accepts.
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        if value.is_empty() || sanitize_name(value) != value {
            return Err(HistorySessionNameError);
        }
        Ok(Self(value.to_string()))
    }
}

impl<'de> Deserialize<'de> for HistorySessionName {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        String::deserialize(deserializer)?
            .parse()
            .map_err(serde::de::Error::custom)
    }
}

impl JsonSchema for HistorySessionName {
    fn inline_schema() -> bool {
        true
    }

    fn schema_name() -> Cow<'static, str> {
        Cow::Borrowed("HistorySessionName")
    }

    fn json_schema(_generator: &mut SchemaGenerator) -> Schema {
        schemars::json_schema!({
            "type": "string",
            "minLength": 1,
            "not": { "pattern": SESSION_NAME_FORBIDDEN_PATTERN },
        })
    }
}

/// A session id: a [`HistorySessionName`], then the compact UTC instant and the
/// pid it was minted with (`<name>-<YYYYMMDDThhmmssZ>-<pid>`, see
/// [`format_compact_utc`]). The instant is a real proleptic-Gregorian one —
/// days per month, leap years, hours to 23 and seconds to 59 — exactly what
/// `session_started_from_id` accepts. Paired with
/// [`SESSION_SELECTOR_FORBIDDEN_PATTERN`], which already rules out the newline
/// Python's `$` would otherwise admit.
const SESSION_SELECTOR_PATTERN: &str = concat!(
    "^(?:[a-z0-9-]+|[a-z0-9-]+-",
    "(?:[0-9]{4}(?:(?:0[13578]|1[02])(?:0[1-9]|[12][0-9]|3[01])",
    "|(?:0[469]|11)(?:0[1-9]|[12][0-9]|30)",
    "|02(?:0[1-9]|1[0-9]|2[0-8]))",
    "|(?:[0-9]{2}(?:0[48]|[2468][048]|[13579][26])|(?:[02468][048]|[13579][26])00)0229)",
    "T(?:[01][0-9]|2[0-3])[0-5][0-9][0-5][0-9]Z-[0-9]+)$",
);

/// [`SESSION_NAME_FORBIDDEN_PATTERN`] widened by the upper-case letters an id's
/// instant carries; [`SESSION_SELECTOR_PATTERN`] confines them to it.
const SESSION_SELECTOR_FORBIDDEN_PATTERN: &str = "[^A-Za-z0-9-]|^-|-$|--";

/// The error returned when text selects no session any writer could mint.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
#[error(
    "must be a session id (`<name>-<YYYYMMDDThhmmssZ>-<pid>`, as `history list` prints it) or a session name of lowercase ASCII letters and digits joined by single dashes"
)]
pub struct HistorySessionSelectorError;

/// A session id in the shape a history writer mints it:
/// `<name>-<YYYYMMDDThhmmssZ>-<pid>`, where the name is a
/// [`HistorySessionName`] and the instant a real calendar one. Constructed only
/// by parsing, so no other text can be held as one.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct HistorySessionId(String);

impl HistorySessionId {
    /// The id as text — the session file's stem.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for HistorySessionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl FromStr for HistorySessionId {
    type Err = HistorySessionSelectorError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let name = value
            .rsplitn(3, '-')
            .nth(2)
            .ok_or(HistorySessionSelectorError)?;
        if name.parse::<HistorySessionName>().is_err() || session_started_from_id(value).is_none() {
            return Err(HistorySessionSelectorError);
        }
        Ok(Self(value.to_string()))
    }
}

/// Which one session to follow: its id — the session file's stem, exactly as
/// `history list` prints it — or its name. Both shapes are the ones a
/// [`HistorySessionName`]-based writer mints, so a selector nothing could ever
/// match (empty, spaced, path-like, an impossible instant) is refused where it
/// enters rather than followed silently forever.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum HistorySessionSelector {
    /// A session id: `<name>-<YYYYMMDDThhmmssZ>-<pid>`.
    Id(HistorySessionId),
    /// A session name, matched against the name its lines and records carry.
    Name(HistorySessionName),
}

impl HistorySessionSelector {
    /// The selector as the text it was parsed from.
    #[must_use]
    pub fn as_str(&self) -> &str {
        match self {
            Self::Id(id) => id.as_str(),
            Self::Name(name) => name.as_str(),
        }
    }
}

impl fmt::Display for HistorySessionSelector {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for HistorySessionSelector {
    type Err = HistorySessionSelectorError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        if let Ok(id) = value.parse() {
            return Ok(Self::Id(id));
        }
        value
            .parse()
            .map(Self::Name)
            .map_err(|_| HistorySessionSelectorError)
    }
}

impl Serialize for HistorySessionSelector {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for HistorySessionSelector {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        String::deserialize(deserializer)?
            .parse()
            .map_err(serde::de::Error::custom)
    }
}

impl JsonSchema for HistorySessionSelector {
    fn inline_schema() -> bool {
        true
    }

    fn schema_name() -> Cow<'static, str> {
        Cow::Borrowed("HistorySessionSelector")
    }

    fn json_schema(_generator: &mut SchemaGenerator) -> Schema {
        schemars::json_schema!({
            "type": "string",
            "minLength": 1,
            "pattern": SESSION_SELECTOR_PATTERN,
            "not": { "pattern": SESSION_SELECTOR_FORBIDDEN_PATTERN },
        })
    }
}

/// A validated, deterministically ordered label set attached to every record in
/// one history session. Keys are portable identifier-like strings; values are
/// non-empty strings without control characters, bounded in characters (see
/// `LABEL_VALUE_MAX`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct HistoryLabels(BTreeMap<String, String>);

impl HistoryLabels {
    /// Validate and construct a label set at a config/env/CLI boundary.
    pub fn new(labels: BTreeMap<String, String>) -> Result<Self, String> {
        for (key, value) in &labels {
            validate_label(key, value)?;
        }
        Ok(Self(labels))
    }

    /// Borrow the ordered map.
    #[must_use]
    pub fn as_map(&self) -> &BTreeMap<String, String> {
        &self.0
    }

    /// Whether no labels are present.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Overlay higher-precedence labels on this set, key by key.
    pub fn extend(&mut self, higher: &Self) {
        self.0.extend(higher.0.clone());
    }

    /// Whether every filter pair appears with the same value.
    #[must_use]
    pub fn matches(&self, filters: &Self) -> bool {
        filters
            .0
            .iter()
            .all(|(key, value)| self.0.get(key) == Some(value))
    }
}

impl<'de> Deserialize<'de> for HistoryLabels {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let labels = BTreeMap::<String, String>::deserialize(deserializer)?;
        Self::new(labels).map_err(serde::de::Error::custom)
    }
}

impl JsonSchema for HistoryLabels {
    fn schema_name() -> Cow<'static, str> {
        Cow::Borrowed("HistoryLabels")
    }

    fn json_schema(_generator: &mut SchemaGenerator) -> Schema {
        schemars::json_schema!({
            "type": "object",
            "propertyNames": {
                "type": "string",
                "maxLength": LABEL_KEY_MAX,
                "pattern": LABEL_KEY_START_PATTERN,
                "not": { "pattern": LABEL_KEY_FORBIDDEN_PATTERN },
            },
            "additionalProperties": {
                "type": "string",
                "minLength": 1,
                "maxLength": LABEL_VALUE_MAX,
                "not": { "pattern": LABEL_VALUE_FORBIDDEN_PATTERN },
            },
        })
    }
}

/// Parse one `key=value` label spelling used by the CLI and environment.
pub fn parse_label(input: &str) -> Result<(String, String), String> {
    let Some((key, value)) = input.split_once('=') else {
        return Err(format!(
            "invalid history label `{input}`: expected KEY=VALUE"
        ));
    };
    validate_label(key, value)?;
    Ok((key.to_string(), value.to_string()))
}

/// Parse repeated `key=value` spellings into a validated set. Later duplicate
/// keys win, matching repeated CLI flags and layer precedence.
pub fn parse_labels<'a>(
    values: impl IntoIterator<Item = &'a str>,
) -> Result<HistoryLabels, String> {
    let mut labels = BTreeMap::new();
    for value in values {
        let (key, value) = parse_label(value)?;
        labels.insert(key, value);
    }
    HistoryLabels::new(labels)
}

fn validate_label(key: &str, value: &str) -> Result<(), String> {
    let valid_key = !key.is_empty()
        && key.chars().count() <= LABEL_KEY_MAX
        && key.chars().enumerate().all(|(index, c)| {
            c.is_ascii_alphanumeric() || (index > 0 && matches!(c, '.' | '_' | '-'))
        });
    if !valid_key {
        return Err(format!(
            "invalid history label key `{key}`: expected 1-{LABEL_KEY_MAX} ASCII letters, digits, `.`, `_`, or `-`, beginning with a letter or digit"
        ));
    }
    if value.is_empty() {
        return Err(format!(
            "invalid history label `{key}`: value must not be empty"
        ));
    }
    if value.chars().count() > LABEL_VALUE_MAX {
        return Err(format!(
            "invalid history label `{key}`: value exceeds {LABEL_VALUE_MAX} characters"
        ));
    }
    if value.chars().any(char::is_control) {
        return Err(format!(
            "invalid history label `{key}`: value must not contain control characters"
        ));
    }
    Ok(())
}

/// One harness run, normalized and frozen for the history log. Serialized as one
/// JSONL line per harness run, appended as the run finalizes. Carries only the
/// normalized cross-harness signals — no raw stdout/stderr.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct HistoryRecord {
    pub schema_version: String,
    /// Globally unique, time-ordered record id. This is also the cursor accepted
    /// by `history watch --after` and the exact id accepted by history lookup.
    pub history_id: HistoryId,
    /// The oneharness session id this run belongs to (the history file's stem).
    pub session: String,
    /// The human-meaningful session name (see [`session_name`]); repeated on
    /// every record so a reader can resolve a session by name from any line.
    pub name: String,
    /// Caller-supplied metadata used to select related task-graph records.
    /// Omitted on the wire when empty for additive compatibility.
    #[serde(default, skip_serializing_if = "HistoryLabels::is_empty")]
    pub labels: HistoryLabels,
    /// The project directory the run operated in (the real path, not the
    /// on-disk slug), so the list view can show where a session ran.
    pub project: String,
    /// RFC3339 UTC instant the record was written (append time).
    pub timestamp: String,
    /// Canonical harness id (e.g. `claude-code`).
    pub harness: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    // llmlint: ignore[invalid_states_unrepresentable] Materialized records preserve the stable SDK string contract; the only constructors normalize a validated current/legacy wire identity before creating this value.
    pub variant: Option<String>,
    // llmlint: ignore[invalid_states_unrepresentable] The composed selector is a public serialized SDK field; materialization derives it from the validated wire tuple and never accepts caller-provided independent pieces.
    pub harness_id: String,
    /// The model this run **requested** — the value oneharness put on the
    /// harness's model flag or protocol frame — if any. Copied from
    /// [`RunResult::model`], so it keeps meaning the requested model even where
    /// `observed_model` below is present.
    pub model: Option<String>,
    /// The model the harness **itself** reported the conversation would run
    /// under, read off its own protocol before any token was spent — for codex
    /// over `app-server`, `result.model` of the `thread/start` /
    /// `thread/resume` response. Copied from [`RunResult::observed_model`]:
    /// never inferred and never copied from `model`, and omitted on the wire on
    /// every path that reports none.
    ///
    /// On a completed app-server turn the two agree, because a turn whose
    /// reported model differed from the requested one was refused before it
    /// ran (`model_mismatch`) — so a completed record's `model` is what codex
    /// ran, and the record can no longer say one model while another was
    /// billed. They differ only on such a refused record, and where no model
    /// was requested at all. Gated to [`FIRST_MODEL_OBSERVATION_SCHEMA_VERSION`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observed_model: Option<String>,
    /// The prompt this harness run received (its own, on a batch run; else the
    /// run's single prompt).
    pub prompt: String,
    /// The normalized approval mode requested for the run.
    pub permission_mode: PermissionMode,
    pub status: Status,
    pub exit_code: Option<i32>,
    pub duration_ms: Option<u128>,
    /// UTC invocation bounds and monotonic time attribution. The provider/tool
    /// split is conservative when a transcript has tool calls but lacks native
    /// boundaries: the observed invocation interval is attributed to the union
    /// of those calls, never double-counted.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub started_at: Option<String>,
    pub finished_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model_ms: Option<u128>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_ms: Option<u128>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub time_to_first_token_ms: Option<u128>,
    /// Union of tool intervals observed at the stdout pipe. Unlike `tool_ms`,
    /// this is not provider-measured and has no model-latency counterpart.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    // llmlint: ignore[invalid_states_unrepresentable] The published history record is an additive flat JSON contract; constructors use the internal timing enum and deserialization rejects mixed provider/observed states.
    pub observed_tool_ms: Option<u128>,
    /// Best-effort final assistant text; `null` when extraction was impossible.
    pub text: Option<String>,
    /// How `text` was extracted; `null` when absent.
    pub text_source: Option<String>,
    /// Best-effort token/cost accounting (every field `null` when unreported).
    pub usage: Usage,
    /// The harness's own continuation id, when it exposed one; `null` otherwise.
    pub session_id: Option<String>,
    /// Best-effort normalized tool-call events; `null` when the harness exposes
    /// no machine-readable trace.
    pub events: Option<Vec<ActionEvent>>,
    /// Best-effort classified failure reason (see [`FailureKind`]); `null` when
    /// unclassified.
    pub failure_kind: Option<FailureKind>,
    /// What a run that failed with **nothing to classify** has to show for
    /// itself: [`RunWork::Done`] where the harness recorded a tool call or billed
    /// usage, [`RunWork::None`] where nothing says it got that far. Copied from
    /// [`RunResult::work`] — one value with one source, like every signal here —
    /// and omitted on the wire on every other run, since a success raises the
    /// question and a classified failure has already answered it.
    ///
    /// This is the field that tells apart, for a reader of the record, the two
    /// things an unexplained failure can be: a candidate that never got started,
    /// and one that ran the task and lost. Without it both are a `nonzero` line with a
    /// null `failure_kind` and empty accounting, and a chain that stopped at the
    /// first reads exactly like a chain that stopped at the second. Gated to
    /// [`FIRST_WORK_EVIDENCE_SCHEMA_VERSION`].
    // llmlint: ignore[invalid_states_unrepresentable] The materialized record is the flat published contract every SDK is generated from, so this cannot be folded into `status`; `from_result` derives it with the fields it qualifies, `complete()` refuses an incoherent combination on the way back in, and the contract matrix pins both directions.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub work: Option<RunWork>,
    /// Best-effort normalized failure text for a run that did not succeed: the
    /// harness's own diagnostic as oneharness captured it on stderr, or
    /// oneharness's own message when it generated one (a spawn failure, a
    /// timeout, a binary that is not installed). This is the *only* place a
    /// record quotes the process's own bytes, and it is deliberately narrow —
    /// trimmed, bounded to [`ERROR_MAX`] characters, and written only for a run
    /// that failed. `failure_kind` says what class of failure it was; this says
    /// what the harness actually reported, which is what an operator reads when
    /// the class is unclassified. Never derived from stdout, so it can never
    /// stand in for provider output the run did not produce. Omitted on the wire
    /// when absent, and gated to [`FIRST_ERROR_SCHEMA_VERSION`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<FailureText>,
}

/// A run `history show` reads before its closing record has landed: the events
/// its event lines have published so far, under the run they belong to. It is a
/// display entry, never a store line: once the run's closing record lands the
/// same run is shown as a [`HistoryRecord`] instead. It exists only because an
/// event line was read, so it always carries at least one event.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct IncompleteHistoryRun {
    /// Always `incomplete`: what tells this entry apart from a record.
    #[serde(rename = "type")]
    kind: IncompleteRunType,
    /// The id the run's closing record will carry as its `history_id`.
    run_id: HistoryId,
    /// The harness the event lines name.
    // llmlint: ignore[invalid_states_unrepresentable] Copied verbatim from `HistoryEventLine::harness`, a legacy-compatible wire string (v1.0 lines predate the composed identity); this display entry repeats what the line says rather than re-deriving an identity the line may not carry.
    harness: String,
    /// The events published so far, in `index` order; never empty.
    #[schemars(length(min = 1))]
    events: Vec<ActionEvent>,
}

impl IncompleteHistoryRun {
    /// The in-flight entry for `run_id`, with `events` put in `index` order;
    /// `None` when there are no events, since a run nothing was read for is not
    /// in flight as far as the store can tell.
    #[must_use]
    pub fn new(run_id: HistoryId, harness: String, mut events: Vec<ActionEvent>) -> Option<Self> {
        if events.is_empty() {
            return None;
        }
        events.sort_by_key(|event| event.index);
        Some(Self {
            kind: IncompleteRunType::Incomplete,
            run_id,
            harness,
            events,
        })
    }

    /// The id the run's closing record will carry as its `history_id`.
    #[must_use]
    pub fn run_id(&self) -> HistoryId {
        self.run_id
    }

    /// The harness the event lines name.
    #[must_use]
    pub fn harness(&self) -> &str {
        &self.harness
    }

    /// The events published so far, in `index` order; never empty.
    #[must_use]
    pub fn events(&self) -> &[ActionEvent] {
        &self.events
    }
}

/// The discriminator of an [`IncompleteHistoryRun`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, JsonSchema)]
pub enum IncompleteRunType {
    #[default]
    #[serde(rename = "incomplete")]
    Incomplete,
}

/// One entry of `history show`'s answer: a finished run's record, or a run still
/// in flight.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
#[serde(untagged)]
pub enum HistoryShowEntry {
    /// A run whose closing record has landed.
    Record(HistoryRecord),
    /// A run that has published events but no closing record yet.
    Incomplete(IncompleteHistoryRun),
}

impl HistoryRecord {
    /// Decode a whole-record history line written before the v1.0 event-sourced
    /// contract. The stable source identity gives v0.1 records (which predate
    /// `history_id`) a repeatable UUID, making migration retries idempotent.
    pub fn from_legacy_value(
        value: Value,
        stable_identity: &str,
    ) -> Result<Self, serde_json::Error> {
        let wire: HistoryRecordWire = serde_json::from_value(value)?;
        let history_id = match wire.schema_version.as_str() {
            PREVIOUS_SCHEMA_VERSION | LEGACY_RECORD_SCHEMA_VERSION => wire
                .history_id
                .ok_or_else(|| invalid_history("legacy history record is missing `history_id`"))?,
            LEGACY_SCHEMA_VERSION => HistoryId::legacy(stable_identity.as_bytes()),
            version => {
                return Err(invalid_history(&format!(
                    "unsupported legacy history schema version `{version}`"
                )))
            }
        };
        let version = wire.schema_version.clone();
        let mut record = Self::from_wire(wire, history_id, SCHEMA_VERSION.to_string());
        if version == LEGACY_SCHEMA_VERSION {
            record.labels = HistoryLabels::default();
        }
        if version == LEGACY_RECORD_SCHEMA_VERSION && !record.complete() {
            return Err(invalid_history(
                "history schema v0.3 record has incomplete telemetry",
            ));
        }
        Ok(record)
    }

    /// Freeze one [`RunResult`] into a history record. `session`/`name` identify
    /// the oneharness session; `timestamp` is the caller-supplied append instant
    /// (an I/O read, kept out of this pure function); `model` is the run's
    /// effective top-level model. `run_prompt` is the fallback prompt for an
    /// ordinary run — a batch result carries its own `prompt`, which wins.
    #[allow(
        clippy::too_many_arguments,
        reason = "each value is an independent published history field; a parameter object would duplicate the wire shape"
    )]
    pub fn from_result(
        history_id: HistoryId,
        session: &str,
        name: &str,
        labels: &HistoryLabels,
        project: &str,
        timestamp: String,
        mode: PermissionMode,
        model: Option<&str>,
        run_prompt: &str,
        r: &RunResult,
    ) -> Self {
        let telemetry = r.telemetry.as_ref();
        let measured = telemetry.and_then(|telemetry| match telemetry {
            crate::domain::report::ExecutionTelemetry::ProviderMeasured {
                started_at,
                finished_at,
                model_ms,
                tool_ms,
                time_to_first_token_ms,
            } => Some((
                started_at,
                finished_at,
                model_ms,
                tool_ms,
                time_to_first_token_ms,
            )),
            crate::domain::report::ExecutionTelemetry::StdoutObserved { .. }
            | crate::domain::report::ExecutionTelemetry::PartialInvocation { .. } => None,
        });
        let partial = telemetry.and_then(|telemetry| match telemetry {
            crate::domain::report::ExecutionTelemetry::PartialInvocation { started_at } => {
                Some(started_at)
            }
            _ => None,
        });
        let error = failure_text(r.status, r.error.as_deref(), &r.stderr);
        // Carried from the result rather than re-derived: the reading the
        // fallback verdict consulted is the reading the record has to show.
        let work = unclassified_failure(r.status, r.failure_kind)
            .then_some(r.work)
            .flatten();
        HistoryRecord {
            // The oldest reader that can understand this record: only the shape a
            // record actually carries forces its version forward, and where it
            // carries several gated shapes the newest of their gates wins — a
            // `model_mismatch` refusal (v1.8) beside a `work` reading (v1.7)
            // must not be declared at the older of the two.
            schema_version: [
                r.observed_model
                    .is_some()
                    .then_some(FIRST_MODEL_OBSERVATION_SCHEMA_VERSION),
                gated_failure_kind_version(r.failure_kind),
                work.is_some().then_some(FIRST_WORK_EVIDENCE_SCHEMA_VERSION),
                (r.status == Status::Cancelled).then_some(FIRST_CANCELLED_SCHEMA_VERSION),
                error.is_some().then_some(FIRST_ERROR_SCHEMA_VERSION),
                partial
                    .is_some()
                    .then_some(FIRST_PARTIAL_TIMING_SCHEMA_VERSION),
                matches!(
                    telemetry,
                    Some(crate::domain::report::ExecutionTelemetry::StdoutObserved { .. })
                )
                .then_some(OBSERVED_TIMING_SCHEMA_VERSION),
            ]
            .into_iter()
            .flatten()
            .max_by_key(|version| version_rank(version))
            .unwrap_or(PREVIOUS_CURRENT_SCHEMA_VERSION)
            .to_string(),
            history_id,
            session: session.to_string(),
            name: name.to_string(),
            labels: labels.clone(),
            project: project.to_string(),
            timestamp: timestamp.clone(),
            harness: r.harness.clone(),
            variant: r.variant.clone(),
            harness_id: r.harness_id.clone(),
            model: model.map(str::to_string),
            observed_model: r.observed_model.clone(),
            prompt: r.prompt.clone().unwrap_or_else(|| run_prompt.to_string()),
            permission_mode: mode,
            status: r.status,
            exit_code: r.exit_code,
            duration_ms: r.duration_ms,
            started_at: measured
                .map(|timing| timing.0.as_str().to_string())
                .or_else(|| partial.map(|started_at| started_at.as_str().to_string())),
            finished_at: measured
                .and_then(|timing| timing.1.as_ref())
                .map(|finished_at| finished_at.as_str().to_string()),
            model_ms: measured.and_then(|timing| *timing.2),
            tool_ms: measured.and_then(|timing| *timing.3),
            time_to_first_token_ms: measured.and_then(|timing| *timing.4),
            observed_tool_ms: telemetry.and_then(|telemetry| match telemetry {
                crate::domain::report::ExecutionTelemetry::StdoutObserved { tool_ms } => {
                    Some(*tool_ms)
                }
                crate::domain::report::ExecutionTelemetry::ProviderMeasured { .. }
                | crate::domain::report::ExecutionTelemetry::PartialInvocation { .. } => None,
            }),
            text: r.text.clone(),
            text_source: r.text_source.clone(),
            usage: r.usage.clone(),
            session_id: r.session_id.clone(),
            events: r.events.clone(),
            failure_kind: r.failure_kind,
            work,
            error,
        }
        .without_untimed_result_status()
    }

    /// `self` with every `tool_result`'s outcome cleared when the record is
    /// untimed (no `started_at`, no `observed_tool_ms`): such a record makes
    /// no tool-state claim on any event (`untimed_trace_valid`, and the schema
    /// every published reader validates with), so the outcome stays in the
    /// report, the live stream and the event lines, and not in the record. A
    /// claude-code result whose `tool_use` was never seen is the case.
    fn without_untimed_result_status(mut self) -> Self {
        if self.started_at.is_none() && self.observed_tool_ms.is_none() {
            for event in self.events.iter_mut().flatten() {
                if event.kind == "tool_result" {
                    event.status = None;
                }
            }
        }
        self
    }

    pub(crate) fn complete(&self) -> bool {
        if !self.versioned_timing_valid()
            || !status_version_valid(&self.schema_version, self.status)
            || !failure_kind_version_valid(&self.schema_version, self.failure_kind)
            || !error_text_valid(
                &self.schema_version,
                self.error.as_ref(),
                self.status,
                self.failure_kind,
            )
            || !work_evidence_valid(&self.schema_version, self.work, self.status)
            || !observed_model_valid(&self.schema_version, self.observed_model.as_deref())
        {
            return false;
        }
        match self.timing_state() {
            // Measured telemetry is held to the same bar for every run: a
            // trace-capable harness must never write numbers that disagree with
            // the run's own wall clock or tool trace.
            Ok(HistoryTiming::Measured {
                started_at,
                finished_at,
                model_ms,
                tool_ms,
            }) => self.measured_trace_valid(started_at, finished_at, model_ms, tool_ms),
            // Absence is the honest representation twice over: for a harness
            // whose spec declares no provider/tool boundary trace, and for a run
            // that failed, which has no timing *because* it failed.
            Ok(HistoryTiming::Unavailable) => {
                run_failed(self.status)
                    || self.failure_kind.is_some()
                    || (self.harness_lacks_trace() && self.untimed_trace_valid())
            }
            // A measurement cut short belongs to a run that was cut short. On a
            // run that succeeded the same shape is corrupt data: the provider
            // answered, so the split it never wrote should have been there.
            Ok(HistoryTiming::Partial {
                started_at,
                time_to_first_token_ms,
            }) => {
                (run_failed(self.status) || self.failure_kind.is_some())
                    && version_at_least(&self.schema_version, FIRST_PARTIAL_TIMING_SCHEMA_VERSION)
                    && partial_trace_valid(started_at, time_to_first_token_ms, self.duration_ms)
            }
            // Anything else is incoherent rather than partial (see `timing_state`).
            Err(()) => false,
        }
    }

    /// Whether the harness this record names declares no provider/tool boundary
    /// trace, so it could not have derived timing however the run went.
    fn harness_lacks_trace(&self) -> bool {
        crate::domain::harness::all()
            .iter()
            .find(|spec| spec.id == self.harness)
            .is_some_and(|spec| spec.telemetry.is_none())
    }

    fn measured_trace_valid(
        &self,
        started_at: &str,
        finished_at: Option<&str>,
        model_ms: u128,
        tool_ms: u128,
    ) -> bool {
        let Some(duration) = self.duration_ms else {
            return false;
        };
        if started_at.is_empty() || model_ms.saturating_add(tool_ms) > duration {
            return false;
        }
        if requires_provider_finish(self.status) && finished_at.is_none() {
            return false;
        }
        self.events.as_ref().is_none_or(|events| {
            events
                .iter()
                .filter(|event| event.kind == "tool_call")
                .all(|event| {
                    let base = event
                        .tool_call_id
                        .as_deref()
                        .is_some_and(|id| !id.is_empty())
                        && event.started_at.is_some()
                        && event.status.is_some();
                    base && match event.status {
                        Some(ToolCallStatus::Completed | ToolCallStatus::Failed) => {
                            event.finished_at.is_some() && event.duration_ms.is_some()
                        }
                        Some(ToolCallStatus::Timeout | ToolCallStatus::Interrupted) => true,
                        None => false,
                    }
                })
        })
    }

    /// With no run-level timing, no event may claim any — otherwise a
    /// trace-capable harness silently writes half-measured data.
    fn untimed_trace_valid(&self) -> bool {
        if self.observed_tool_ms.is_some() {
            return true;
        }
        self.events.as_ref().is_none_or(|events| {
            events.iter().all(|event| {
                event.started_at.is_none()
                    && event.finished_at.is_none()
                    && event.duration_ms.is_none()
                    && event.status.is_none()
            })
        })
    }

    fn versioned_timing_valid(&self) -> bool {
        let reads_observed_timing =
            version_at_least(&self.schema_version, OBSERVED_TIMING_SCHEMA_VERSION);
        if !reads_observed_timing
            && self
                .events
                .as_ref()
                .is_some_and(|events| events.iter().any(|event| event.timing_source.is_some()))
        {
            return false;
        }
        match self.observed_tool_ms {
            None => true,
            Some(_) => {
                reads_observed_timing
                    && self.duration_ms.is_some()
                    && matches!(self.timing_state(), Ok(HistoryTiming::Unavailable))
            }
        }
    }

    fn timing_state(&self) -> Result<HistoryTiming<'_>, ()> {
        match (
            self.started_at.as_deref(),
            self.finished_at.as_deref(),
            self.model_ms,
            self.tool_ms,
            self.time_to_first_token_ms,
        ) {
            (None, None, None, None, None) => Ok(HistoryTiming::Unavailable),
            (Some(started_at), finished_at, Some(model_ms), Some(tool_ms), _) => {
                Ok(HistoryTiming::Measured {
                    started_at,
                    finished_at,
                    model_ms,
                    tool_ms,
                })
            }
            // A measurement that stopped at the invocation bounds — including
            // before any provider finish, which is dropped with the split it
            // belongs to. Every other combination is incoherent rather than
            // partial — a finish with no start or no split, a model total with no
            // tool total, a first-token offset with nothing to measure it from —
            // and stays refused.
            (Some(started_at), None, None, None, time_to_first_token_ms) => {
                Ok(HistoryTiming::Partial {
                    started_at,
                    time_to_first_token_ms,
                })
            }
            _ => Err(()),
        }
    }

    /// Deserialize the materialized view of a current event-sourced run.
    pub fn from_value(value: Value) -> Result<Self, serde_json::Error> {
        let wire: HistoryRecordWire = serde_json::from_value(value)?;
        if version_rank(&wire.schema_version).is_none() {
            return Err(serde_json::Error::io(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!(
                    "unsupported history schema version `{}`",
                    wire.schema_version
                ),
            )));
        }
        if !identity_fields_valid(
            &wire.harness,
            wire.variant.as_deref(),
            wire.harness_id.as_deref(),
        ) {
            return Err(invalid_history("inconsistent history harness identity"));
        }
        let history_id = wire.history_id.ok_or_else(|| {
            serde_json::Error::io(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "history record is missing `history_id`",
            ))
        })?;
        let schema_version = wire.schema_version.clone();
        let record = Self::from_wire(wire, history_id, schema_version);
        if !record.complete() {
            return Err(invalid_history(
                "history schema v1.0 record has incomplete telemetry",
            ));
        }
        Ok(record)
    }

    fn from_wire(wire: HistoryRecordWire, history_id: HistoryId, schema_version: String) -> Self {
        Self {
            schema_version,
            history_id,
            session: wire.session,
            name: wire.name,
            labels: wire.labels,
            project: wire.project,
            timestamp: wire.timestamp.clone(),
            harness: wire.harness.clone(),
            variant: wire.variant,
            harness_id: wire.harness_id.unwrap_or_else(|| wire.harness.clone()),
            model: wire.model,
            observed_model: wire.observed_model,
            prompt: wire.prompt,
            permission_mode: wire.permission_mode,
            status: wire.status,
            exit_code: wire.exit_code,
            duration_ms: wire.duration_ms,
            started_at: wire.started_at,
            finished_at: wire.finished_at,
            model_ms: wire.model_ms,
            tool_ms: wire.tool_ms,
            time_to_first_token_ms: wire.time_to_first_token_ms,
            observed_tool_ms: wire.observed_tool_ms,
            text: wire.text,
            text_source: wire.text_source,
            usage: wire.usage,
            session_id: wire.session_id,
            events: wire.events.map(|events| {
                events
                    .into_iter()
                    .map(LegacyActionEvent::into_current)
                    .collect()
            }),
            failure_kind: wire.failure_kind,
            work: wire.work,
            error: wire.error,
        }
    }
}

fn identity_fields_valid(harness: &str, variant: Option<&str>, harness_id: Option<&str>) -> bool {
    let expected = match variant {
        Some(variant)
            if variant
                .parse::<crate::domain::config::VariantName>()
                .is_ok() =>
        {
            format!("{harness}:{variant}")
        }
        Some(_) => return false,
        None => harness.to_string(),
    };
    harness_id.is_none_or(|harness_id| harness_id == expected)
}

fn invalid_history(message: &str) -> serde_json::Error {
    serde_json::Error::io(std::io::Error::new(
        std::io::ErrorKind::InvalidData,
        message,
    ))
}

/// Validated availability states for v1.0 timing telemetry. Keeping the
/// cross-field wire representation behind this conversion prevents producers
/// and readers from treating a partial set of optional fields as meaningful.
enum HistoryTiming<'a> {
    Unavailable,
    /// The invocation bounds the runner observed directly, with no provider/tool
    /// split derived from them — what a run cut short leaves behind. Only the
    /// bounds: a split read out of a transcript that never finished is not a
    /// measurement, so it is dropped rather than reported as one. Introduced in
    /// [`FIRST_PARTIAL_TIMING_SCHEMA_VERSION`], which every record carrying it
    /// declares.
    Partial {
        started_at: &'a str,
        time_to_first_token_ms: Option<u128>,
    },
    Measured {
        started_at: &'a str,
        finished_at: Option<&'a str>,
        model_ms: u128,
        tool_ms: u128,
    },
}

/// Whether an invocation-bounds-only measurement agrees with the run's own wall
/// clock. Shared by the record and the run line so the file a writer produces and
/// the record a reader materializes are held to one rule.
fn partial_trace_valid(
    started_at: &str,
    time_to_first_token_ms: Option<u128>,
    duration_ms: Option<u128>,
) -> bool {
    !started_at.is_empty()
        && duration_ms.is_some_and(|duration| {
            time_to_first_token_ms.is_none_or(|to_first_token| to_first_token <= duration)
        })
}

/// Whether a record with this status must carry a provider `finished_at` to be a
/// complete measurement: the run reached its own end, so a measured trace that
/// omits the finish is corrupt rather than cut short.
///
/// Public for the same reason [`run_failed`] is: the generated SDK schemas split
/// their measured-timing branches by this predicate, so the rule lives once and
/// is applied to the enum's own variants rather than being restated as a
/// hand-kept status list that a new variant would silently fall out of.
#[must_use]
pub fn requires_provider_finish(status: Status) -> bool {
    matches!(status, Status::Ok | Status::Nonzero)
}

/// Whether the run this record describes did not succeed. Such a run may have no
/// timing *because* it failed — it never reached the boundary a provider trace is
/// measured between — so absent telemetry is its honest representation, exactly
/// as it is for a harness that declares no trace at all. A run that succeeded
/// keeps the full strictness, so a trace-capable harness still cannot silently
/// write corrupt data for work that actually happened.
///
/// The test is the status alone. Extracted `text` is deliberately NOT consulted:
/// a failed run's transcript can still yield partial assistant text (a reasoning
/// line, a half-finished answer), and reading that as "the provider succeeded"
/// would refuse the record for the very runs — cut short mid-turn — this exists
/// to keep. `status` is the run's own verdict on whether it worked.
///
/// Public because the generated SDK schemas split their status branches by this
/// same predicate: one rule, applied to the enum's own serialized variants,
/// rather than two hand-kept lists.
pub fn run_failed(status: Status) -> bool {
    matches!(
        status,
        Status::Nonzero
            | Status::Timeout
            | Status::SpawnError
            | Status::Skipped
            | Status::Cancelled
    )
}

/// The longest normalized failure text a record carries, in *characters* — the
/// unit [`crate::domain::history`] bounds every string in, because it is the one
/// unit `maxLength` expresses and therefore the one every generated SDK
/// validator can agree on. A CLI's error message is a line or two; the bound is
/// what keeps a runaway stderr dump out of a history line, and the marker is
/// what tells a reader the text was cut.
pub const ERROR_MAX: usize = 2048;
const ERROR_TRUNCATION_MARKER: char = '\u{2026}';

/// The error returned when text cannot be a [`FailureText`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
#[error("must be non-empty and at most {ERROR_MAX} characters")]
pub struct FailureTextError;

/// A record's failure message: non-empty, and bounded to [`ERROR_MAX`]
/// characters. That pair is the whole invariant — exactly what the generated
/// schema states, so reading a record can never widen what writing one promises.
/// Wrapping it is what keeps an empty or runaway value out of a record entirely,
/// the same way [`HistoryId`] and [`HistoryLabels`] keep their own invariants
/// unrepresentable rather than re-checked at each use.
///
/// Trimming and the truncation marker belong to [`Self::normalized`], the
/// constructor writers use — not to the type. A record read back from disk
/// preserves the text it was written with rather than being quietly rewritten.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct FailureText(String);

impl FailureText {
    /// Trim `text` and bound it to [`ERROR_MAX`] characters, marking the cut so
    /// a reader can tell truncated output from a message that simply ended.
    /// `None` when nothing is left to report.
    #[must_use]
    pub fn normalized(text: &str) -> Option<Self> {
        let source = text.trim();
        if source.is_empty() {
            return None;
        }
        let mut bounded: String = source.chars().take(ERROR_MAX).collect();
        if source.chars().nth(ERROR_MAX).is_some() {
            bounded.truncate(
                bounded
                    .char_indices()
                    .nth(ERROR_MAX - 1)
                    .map_or(bounded.len(), |(offset, _)| offset),
            );
            bounded.push(ERROR_TRUNCATION_MARKER);
        }
        Some(Self(bounded))
    }

    /// The message text — non-empty and within [`ERROR_MAX`] by construction, so
    /// a caller renders or stores it without re-checking either.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for FailureText {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl FromStr for FailureText {
    type Err = FailureTextError;

    /// Accept exactly what the schema promises a reader, and nothing more.
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        if value.is_empty() || value.chars().count() > ERROR_MAX {
            return Err(FailureTextError);
        }
        Ok(Self(value.to_string()))
    }
}

impl<'de> Deserialize<'de> for FailureText {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        String::deserialize(deserializer)?
            .parse()
            .map_err(serde::de::Error::custom)
    }
}

impl JsonSchema for FailureText {
    fn inline_schema() -> bool {
        true
    }

    fn schema_name() -> Cow<'static, str> {
        Cow::Borrowed("FailureText")
    }

    fn json_schema(_generator: &mut SchemaGenerator) -> Schema {
        schemars::json_schema!({
            "type": "string",
            "minLength": 1,
            "maxLength": ERROR_MAX,
        })
    }
}

/// The failure text for one run, normalized by [`FailureText::normalized`]:
/// oneharness's own diagnostic when it generated one (a spawn failure, a
/// timeout, a binary that is not installed), else the harness's captured stderr
/// — but only for a run that failed, so a successful run's stderr chatter never
/// lands in history.
fn failure_text(status: Status, error: Option<&str>, stderr: &str) -> Option<FailureText> {
    match error.map(str::trim) {
        Some(error) if !error.is_empty() => FailureText::normalized(error),
        _ if run_failed(status) => FailureText::normalized(stderr),
        _ => None,
    }
}

/// Whether the run this record describes reported a failure *in words* — by its
/// own exit status, or as the one clean exit that still has something to say.
///
/// That exception is [`FailureKind::ToolDeferred`]: a deferred-tool dead-end
/// exits 0 having done no work, and oneharness's own message about it is exactly
/// the failure text a record should carry. It is the only clean exit that gets
/// one — a provider failure classified off a clean run's own output leaves the
/// harness's words in its transcript, not in a diagnostic — so no other kind
/// justifies failure text on a `status: ok` record.
///
/// Deliberately narrower than timing validation: any classified failure may
/// lack a complete provider trace even when its transport exited zero, but only
/// `tool_deferred` has a oneharness-authored failure message to persist from
/// such a clean exit.
fn reported_failure(status: Status, failure_kind: Option<FailureKind>) -> bool {
    run_failed(status) || failure_kind == Some(FailureKind::ToolDeferred)
}

/// Whether `status` is a value a reader at `schema_version` has. A record's
/// version is the promise "you can read this"; a status introduced later would
/// break that promise, so it is refused rather than read back at a version that
/// never had it. Stated here because the generated SDK schemas gate the same
/// value the same way — one rule, two validators.
fn status_version_valid(schema_version: &str, status: Status) -> bool {
    status != Status::Cancelled || version_at_least(schema_version, FIRST_CANCELLED_SCHEMA_VERSION)
}

/// The same promise for `failure_kind`: a kind introduced after `schema_version`
/// is refused rather than read back at a version whose enum never had it. Six
/// kinds are gated — [`FailureKind::SessionNotFound`] at v1.5, the two
/// precondition refusals at v1.6, [`FailureKind::ModelMismatch`] at v1.8,
/// [`FailureKind::ServerOverloaded`] at v1.9, and
/// [`FailureKind::AppServerLost`] at v1.10;
/// every other kind predates the oldest readable version. Stated here because the generated SDK schemas gate the same values
/// the same way — one rule, two validators.
fn failure_kind_version_valid(schema_version: &str, failure_kind: Option<FailureKind>) -> bool {
    match gated_failure_kind_version(failure_kind) {
        Some(introduced) => version_at_least(schema_version, introduced),
        None => true,
    }
}

/// The version a gated `failure_kind` arrived in, or `None` for a kind every
/// readable version already had. Six gates: `session_not_found` at v1.5, the
/// two precondition refusals at v1.6, `model_mismatch` at v1.8,
/// `server_overloaded` at v1.9, and `app_server_lost` at v1.10. One table so the runtime reader and the
/// generated SDK schemas ([`crate::sdk_schema`]) enumerate the same gates —
/// a second list is how the two validators drift apart. Crate-visible, since
/// both of those readers are in this crate: a consumer reads the gate from the
/// generated schema, not from this table.
#[must_use]
pub(crate) fn gated_failure_kind_version(
    failure_kind: Option<FailureKind>,
) -> Option<&'static str> {
    match failure_kind? {
        FailureKind::SessionNotFound => Some(FIRST_SESSION_NOT_FOUND_SCHEMA_VERSION),
        FailureKind::UntrustedDirectory | FailureKind::InputTooLarge => {
            Some(FIRST_PRECONDITION_SCHEMA_VERSION)
        }
        FailureKind::ModelMismatch => Some(FIRST_MODEL_OBSERVATION_SCHEMA_VERSION),
        FailureKind::ServerOverloaded => Some(FIRST_SERVER_OVERLOADED_SCHEMA_VERSION),
        FailureKind::AppServerLost => Some(FIRST_APP_SERVER_LOST_SCHEMA_VERSION),
        FailureKind::Auth
        | FailureKind::RateLimit
        | FailureKind::ModelNotFound
        | FailureKind::Quota
        | FailureKind::ToolDeferred => None,
    }
}

/// Whether a record's failure text agrees with the rest of the record: the field
/// arrived in [`FIRST_ERROR_SCHEMA_VERSION`], so an older record carrying it was
/// not written by any oneharness, and it is a *failure* signal, so a run that
/// reported no failure has nothing to put in it. (Emptiness and length are not
/// checked here — [`FailureText`] makes those unrepresentable, and the generated
/// schema states them.)
fn error_text_valid(
    schema_version: &str,
    error: Option<&FailureText>,
    status: Status,
    failure_kind: Option<FailureKind>,
) -> bool {
    error.is_none()
        || (version_at_least(schema_version, FIRST_ERROR_SCHEMA_VERSION)
            && reported_failure(status, failure_kind))
}

/// Whether a record's work reading agrees with the rest of the record: the field
/// arrived in [`FIRST_WORK_EVIDENCE_SCHEMA_VERSION`], so an older record carrying
/// it was not written by any oneharness, and it answers a question only a run the
/// **harness itself** failed can raise ([`attempted_failure`]) — a run that
/// succeeded, and one whose status already says it never ran, each have nothing
/// to put in it.
///
/// The writer is narrower: [`HistoryRecord::from_result`] records a reading only
/// where nothing classified the failure. That stays a writer rule rather than a
/// third condition here, because a record carrying both a kind and a reading is
/// consistent rather than corrupt — and because a validator this bundle's
/// generated SDKs cannot express is a rule only one of the three would enforce
/// (see [`crate::sdk_schema`]'s `work_placement_gate`).
fn work_evidence_valid(schema_version: &str, work: Option<RunWork>, status: Status) -> bool {
    work.is_none()
        || (version_at_least(schema_version, FIRST_WORK_EVIDENCE_SCHEMA_VERSION)
            && attempted_failure(status))
}

/// Whether a record's observed model agrees with its version: the field arrived
/// in [`FIRST_MODEL_OBSERVATION_SCHEMA_VERSION`], so an older record carrying it
/// was not written by any oneharness. Stated here because the generated SDK
/// schemas gate the same field the same way — one rule, two validators.
fn observed_model_valid(schema_version: &str, observed_model: Option<&str>) -> bool {
    observed_model.is_none()
        || version_at_least(schema_version, FIRST_MODEL_OBSERVATION_SCHEMA_VERSION)
}

impl<'de> Deserialize<'de> for HistoryRecord {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = Value::deserialize(deserializer)?;
        Self::from_value(value).map_err(serde::de::Error::custom)
    }
}

#[derive(Deserialize)]
struct HistoryRecordWire {
    schema_version: String,
    history_id: Option<HistoryId>,
    session: String,
    name: String,
    #[serde(default)]
    labels: HistoryLabels,
    project: String,
    timestamp: String,
    harness: String,
    #[serde(default)]
    // llmlint: ignore[invalid_states_unrepresentable] This private compatibility wire type must deserialize legacy optional strings before schema-aware normalization; it is never exposed directly.
    variant: Option<String>,
    #[serde(default)]
    // llmlint: ignore[invalid_states_unrepresentable] This private optional field exists only for v1.0 compatibility and is normalized into the required materialized composed id.
    harness_id: Option<String>,
    model: Option<String>,
    #[serde(default)]
    observed_model: Option<String>,
    prompt: String,
    permission_mode: PermissionMode,
    status: Status,
    exit_code: Option<i32>,
    duration_ms: Option<u128>,
    #[serde(default)]
    started_at: Option<String>,
    #[serde(default)]
    finished_at: Option<String>,
    #[serde(default)]
    model_ms: Option<u128>,
    #[serde(default)]
    tool_ms: Option<u128>,
    #[serde(default)]
    time_to_first_token_ms: Option<u128>,
    #[serde(default)]
    observed_tool_ms: Option<u128>,
    text: Option<String>,
    text_source: Option<String>,
    usage: Usage,
    session_id: Option<String>,
    events: Option<Vec<LegacyActionEvent>>,
    failure_kind: Option<FailureKind>,
    #[serde(default)]
    work: Option<RunWork>,
    #[serde(default)]
    error: Option<FailureText>,
}

/// Superset reader for action events across the legacy contracts. Versions 0.1
/// and 0.2 ended at `index`; 0.3 added the remaining lifecycle fields.
#[derive(Deserialize)]
struct LegacyActionEvent {
    kind: String,
    name: Option<String>,
    input: Option<Value>,
    output: Option<String>,
    index: usize,
    #[serde(default)]
    tool_call_id: Option<String>,
    #[serde(default)]
    started_at: Option<String>,
    #[serde(default)]
    finished_at: Option<String>,
    #[serde(default)]
    duration_ms: Option<u128>,
    #[serde(default)]
    status: Option<ToolCallStatus>,
    #[serde(default)]
    timing_source: Option<crate::domain::events::TimingSource>,
}

impl LegacyActionEvent {
    fn into_current(self) -> ActionEvent {
        ActionEvent {
            kind: self.kind,
            name: self.name,
            input: self.input,
            output: self.output,
            index: self.index,
            tool_call_id: self.tool_call_id,
            started_at: self.started_at,
            finished_at: self.finished_at,
            duration_ms: self.duration_ms,
            status: self.status,
            timing_source: self.timing_source,
        }
    }
}

/// One line emitted by `history watch --format jsonl`. The tagged envelope lets
/// SDKs distinguish the stream from other NDJSON surfaces while the record's
/// `history_id` remains the resumable cursor.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum HistoryStreamEnvelope {
    /// A newly observed history record.
    Record { record: HistoryRecord },
    /// A normalized action event observed before its run closes.
    Event { line: HistoryEventLine },
}

/// The version of the [`HistoryPointer`] line. Its own contract, on its own
/// cadence: the pointer names a session, and says nothing about the record
/// shape inside it, so a record bump never moves this. A reader accepts any
/// `1.<minor>` line, since a minor only ever adds a field.
pub const POINTER_SCHEMA_VERSION: &str = "1.0";

/// The file extension of every session log, which a pointer's `history_file`
/// ends in. The I/O layer names the file with it; the pointer checks it.
pub const SESSION_FILE_EXT: &str = "jsonl";

/// Why a line is not a [`HistoryPointer`]: the text names the invariant that
/// failed, flattened like every other external string this module quotes back.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("not a history pointer: {0}")]
pub struct HistoryPointerError(String);

/// One line of a run's **pointer file**: where one harness run's history went.
///
/// A consumer that starts many `oneharness` processes (or in-process runs) names
/// one file, and every run with history on appends one of these per harness run
/// it begins — so "which sessions did this run launch, and where are they" is a
/// read of that one small file rather than a scan of the whole store, wherever
/// the store lives. `history_dir` / `history_project` / `history_session` are
/// spelled as the `oneharness-session` artifact `oneagentgraph` publishes, so a
/// reader already resolving that through `io::history::find_session_path`
/// resolves these unchanged.
///
/// Built only through [`HistoryPointer::new`] over a [`PointerSession`], and
/// read back only through a deserialization that re-checks every invariant
/// `new` establishes — the four session spellings compose into one file, the
/// two identity spellings are the parsed [`HarnessIdentity`]'s own, the version
/// is one this reader knows — so a line that parses IS a pointer, and a foreign
/// object that happens to carry these keys is counted as skipped rather than
/// read as one.
/// The fields are read through accessors for the same reason: a Rust caller
/// gets a line from one of those two doors, never assembles or edits one.
// `remote = "Self"` is how the derived deserializer becomes the inherent
// `HistoryPointer::deserialize` the checking `Deserialize` impl below wraps,
// with one field list rather than a mirror of it; the derived serializer is
// wrapped the same way, unchanged. A plain comment, because a doc comment here
// is also the SDKs' generated description.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(remote = "Self")]
pub struct HistoryPointer {
    /// [`POINTER_SCHEMA_VERSION`]; a reader accepts any `1.<minor>`.
    schema_version: String,
    /// The history id of the record this harness run will close with — the
    /// exact id `history show <history-id>` resolves.
    history_id: HistoryId,
    /// The store the session is under, absolute.
    history_dir: String,
    /// The project slug — the session file's parent directory name.
    history_project: String,
    /// The session id — the session file's stem.
    history_session: String,
    /// The session file, absolute; the same path the run report echoes, and
    /// always `<history_dir>/<history_project>/<history_session>.jsonl`.
    history_file: String,
    /// The session's human-meaningful name (see [`session_name`]).
    name: String,
    /// The project directory the run operates in, canonical.
    project: String,
    /// The harness id's base, e.g. `claude-code`.
    harness: String,
    /// The variant, omitted for a bare harness.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    variant: Option<String>,
    /// The whole configured id, e.g. `claude-code:primary` — always `harness`
    /// with `:variant` when there is one.
    // The identity value type, not its text: parsing is its only constructor,
    // so a base that names no harness or a variant outside
    // `VARIANT_NAME_PATTERN` (a `:` inside it, which would re-split on the next
    // read) cannot be held here at all. `harness` and `variant` above are the
    // contract's two spellings of the same value, held equal to it by
    // `checked`. Its schema is the string the SDKs already read it as.
    #[schemars(with = "String")]
    harness_id: HarnessIdentity,
    /// RFC 3339 UTC, when this harness run began.
    started: UtcInstant,
    /// The session's validated labels, omitted when empty.
    #[serde(default, skip_serializing_if = "HistoryLabels::is_empty")]
    labels: HistoryLabels,
}

/// Everything a session knows about itself that every pointer line repeats,
/// built once per session by the writer and stamped per harness run. The
/// project slug and session id are read off the session file rather than
/// supplied beside it, so the four spellings cannot disagree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PointerSession {
    history_dir: String,
    history_project: String,
    history_session: String,
    history_file: String,
    name: String,
    project: String,
    labels: HistoryLabels,
}

impl PointerSession {
    /// A session whose file is `history_file`, under the store `history_dir`.
    /// Both must be absolute, and the file must sit exactly one project
    /// directory below the store with the session-log extension — which is
    /// where the writer always puts it, so this refuses only a caller that
    /// hands in a path it did not get from the writer.
    pub fn new(
        history_dir: &Path,
        history_file: &Path,
        name: &str,
        project: &str,
        labels: HistoryLabels,
    ) -> Result<Self, HistoryPointerError> {
        let refuse = |what: &str| HistoryPointerError(what.to_string());
        if !history_dir.is_absolute() || !history_file.is_absolute() {
            return Err(refuse("the store and the session file must be absolute"));
        }
        if !Path::new(project).is_absolute() {
            return Err(refuse("the project directory must be absolute"));
        }
        if name.is_empty() {
            return Err(refuse("the session name is empty"));
        }
        let session = history_file
            .file_stem()
            .and_then(|stem| stem.to_str())
            .filter(|stem| !stem.is_empty())
            .ok_or_else(|| refuse("the session file has no stem"))?;
        if history_file.extension().and_then(|ext| ext.to_str()) != Some(SESSION_FILE_EXT) {
            return Err(refuse("the session file is not a session log"));
        }
        let project_dir = history_file
            .parent()
            .ok_or_else(|| refuse("the session file has no project directory"))?;
        let slug = project_dir
            .file_name()
            .and_then(|slug| slug.to_str())
            .filter(|slug| !slug.is_empty())
            .ok_or_else(|| refuse("the session file's project directory has no name"))?;
        if project_dir.parent() != Some(history_dir) {
            return Err(refuse(
                "the session file is not one project directory below the store",
            ));
        }
        Ok(PointerSession {
            history_dir: history_dir.display().to_string(),
            history_project: slug.to_string(),
            history_session: session.to_string(),
            history_file: history_file.display().to_string(),
            name: name.to_string(),
            project: project.to_string(),
            labels,
        })
    }
}

impl HistoryPointer {
    /// [`POINTER_SCHEMA_VERSION`] as written; a `1.<minor>` this reader knows.
    #[must_use]
    pub fn schema_version(&self) -> &str {
        &self.schema_version
    }

    /// The history id of the record this harness run closes with.
    #[must_use]
    pub fn history_id(&self) -> HistoryId {
        self.history_id
    }

    /// The store the session is under, absolute.
    #[must_use]
    pub fn history_dir(&self) -> &str {
        &self.history_dir
    }

    /// The project slug — the session file's parent directory name.
    #[must_use]
    pub fn history_project(&self) -> &str {
        &self.history_project
    }

    /// The session id — the session file's stem.
    #[must_use]
    pub fn history_session(&self) -> &str {
        &self.history_session
    }

    /// The session file, absolute; the same path the run report echoes.
    #[must_use]
    pub fn history_file(&self) -> &str {
        &self.history_file
    }

    /// The session's human-meaningful name.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The project directory the run operates in, canonical.
    #[must_use]
    pub fn project(&self) -> &str {
        &self.project
    }

    /// The harness id's base, e.g. `claude-code`.
    #[must_use]
    pub fn harness(&self) -> &str {
        &self.harness
    }

    /// The variant, `None` for a bare harness.
    #[must_use]
    pub fn variant(&self) -> Option<&str> {
        self.variant.as_deref()
    }

    /// The whole configured id, e.g. `claude-code:primary`.
    #[must_use]
    pub fn harness_id(&self) -> &str {
        self.harness_id.as_str()
    }

    /// When this harness run began.
    #[must_use]
    pub fn started(&self) -> &UtcInstant {
        &self.started
    }

    /// The session's validated labels (empty when none).
    #[must_use]
    pub fn labels(&self) -> &HistoryLabels {
        &self.labels
    }

    /// One pointer line for the harness run `history_id`, begun at `started`,
    /// under a session. The identity's base and variant are the ones
    /// [`HarnessIdentity`] parsed — the same grammar every other identity in
    /// the crate is held to — so a line this constructs is one the reader
    /// admits.
    pub fn new(
        session: &PointerSession,
        history_id: HistoryId,
        harness_id: &HarnessIdentity,
        started: UtcInstant,
    ) -> Result<Self, HistoryPointerError> {
        HistoryPointer {
            schema_version: POINTER_SCHEMA_VERSION.to_string(),
            history_id,
            history_dir: session.history_dir.clone(),
            history_project: session.history_project.clone(),
            history_session: session.history_session.clone(),
            history_file: session.history_file.clone(),
            name: session.name.clone(),
            project: session.project.clone(),
            harness: harness_id.base().to_string(),
            variant: harness_id.variant().map(str::to_string),
            harness_id: harness_id.clone(),
            started,
            labels: session.labels.clone(),
        }
        .checked()
    }

    /// Every invariant a line must hold to be a pointer — the version is one
    /// this reader knows, the identity spellings compose, the four session
    /// spellings name one session file — applied to what [`new`](Self::new)
    /// assembled and to a line read back off disk alike.
    fn checked(self) -> Result<Self, HistoryPointerError> {
        let refuse = |what: String| HistoryPointerError(what);
        let (major, minor) = self
            .schema_version
            .split_once('.')
            .ok_or_else(|| refuse(format!("schema_version `{}`", self.schema_version)))?;
        if major != "1" || minor.is_empty() || !minor.bytes().all(|b| b.is_ascii_digit()) {
            return Err(refuse(format!(
                "schema_version `{}` is not a 1.x pointer",
                self.schema_version
            )));
        }
        // `harness_id` is already a parsed identity; the two spellings beside
        // it must be exactly its base and its variant.
        if self.harness != self.harness_id.base()
            || self.variant.as_deref() != self.harness_id.variant()
        {
            return Err(refuse(format!(
                "harness `{}` / variant `{}` do not spell harness_id `{}`",
                self.harness,
                self.variant.as_deref().unwrap_or(""),
                self.harness_id
            )));
        }
        let session = PointerSession::new(
            Path::new(&self.history_dir),
            Path::new(&self.history_file),
            &self.name,
            &self.project,
            self.labels.clone(),
        )?;
        if session.history_project != self.history_project
            || session.history_session != self.history_session
        {
            return Err(refuse(format!(
                "history_project `{}` / history_session `{}` do not name `{}`",
                self.history_project, self.history_session, self.history_file
            )));
        }
        Ok(self)
    }
}

impl Serialize for HistoryPointer {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        HistoryPointer::serialize(self, serializer)
    }
}

impl<'de> Deserialize<'de> for HistoryPointer {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        HistoryPointer::deserialize(deserializer)?
            .checked()
            .map_err(serde::de::Error::custom)
    }
}

/// A filesystem-safe slug for a project directory, so history is partitioned by
/// project. Every character outside `[A-Za-z0-9._-]` becomes `-`, runs of `-`
/// collapse, and leading/trailing `-` are trimmed. An empty result falls back to
/// `project` so a path made entirely of separators still yields a directory.
pub fn project_slug(path: &str) -> String {
    let slug = collapse_dashes(path, |c| {
        c.is_ascii_alphanumeric() || matches!(c, '.' | '_')
    });
    if slug.is_empty() {
        "project".to_string()
    } else {
        slug
    }
}

/// The default human-meaningful session name: the first few words of the session's
/// first prompt, lowercased and joined with `-`, capped in length. Punctuation is
/// dropped; an empty or word-less prompt falls back to `session`. Deterministic and
/// pure, so the same prompt always names the same way.
pub fn session_name(first_prompt: &str) -> String {
    const MAX_WORDS: usize = 6;
    const MAX_LEN: usize = 48;

    let mut words: Vec<String> = Vec::new();
    for raw in first_prompt.split_whitespace() {
        let word: String = raw
            .chars()
            .filter(|c| c.is_ascii_alphanumeric())
            .map(|c| c.to_ascii_lowercase())
            .collect();
        if !word.is_empty() {
            words.push(word);
        }
        if words.len() == MAX_WORDS {
            break;
        }
    }
    if words.is_empty() {
        return "session".to_string();
    }
    let mut name = words.join("-");
    if name.len() > MAX_LEN {
        name.truncate(MAX_LEN);
        // Never end on a dangling dash after the truncation.
        name = name.trim_end_matches('-').to_string();
    }
    name
}

/// Sanitize an explicit `--history-name` into the same shape [`session_name`]
/// produces, so a user label and a derived name are interchangeable everywhere
/// (filenames, name lookup). An empty result falls back to `session`.
pub fn sanitize_name(name: &str) -> String {
    let slug = collapse_dashes(name, |c| c.is_ascii_alphanumeric());
    if slug.is_empty() {
        "session".to_string()
    } else {
        slug.to_ascii_lowercase()
    }
}

/// Replace every run of characters failing `keep` with a single `-`, trimming
/// leading/trailing dashes. Shared by [`project_slug`] and [`sanitize_name`].
fn collapse_dashes(input: &str, keep: impl Fn(char) -> bool) -> String {
    let mut out = String::with_capacity(input.len());
    let mut pending_dash = false;
    for c in input.chars() {
        if keep(c) {
            if pending_dash && !out.is_empty() {
                out.push('-');
            }
            pending_dash = false;
            out.push(c);
        } else {
            pending_dash = true;
        }
    }
    out
}

/// The civil (proleptic Gregorian) date and time-of-day for a UNIX timestamp in
/// seconds (UTC). Uses Howard Hinnant's `civil_from_days` algorithm — exact for
/// all dates, no lookup tables — so history timestamps need no date library.
/// Returns `(year, month, day, hour, minute, second)`.
pub fn civil_from_epoch(secs: i64) -> (i64, u32, u32, u32, u32, u32) {
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let (h, mi, s) = (
        (rem / 3600) as u32,
        ((rem % 3600) / 60) as u32,
        (rem % 60) as u32,
    );

    // civil_from_days: days is the count since 1970-01-01.
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365; // [0, 399]
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32; // [1, 31]
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32; // [1, 12]
    let year = if m <= 2 { y + 1 } else { y };
    (year, m, d, h, mi, s)
}

/// Format a UNIX timestamp (seconds, UTC) as `YYYY-MM-DDThh:mm:ssZ` — the
/// human-and-machine-readable instant stored on each history record.
pub fn format_rfc3339(secs: i64) -> String {
    let (y, mo, d, h, mi, s) = civil_from_epoch(secs);
    format!("{y:04}-{mo:02}-{d:02}T{h:02}:{mi:02}:{s:02}Z")
}

/// Format a UNIX millisecond instant as RFC3339 UTC while retaining the
/// precision needed to distinguish short model/tool intervals.
pub fn format_rfc3339_millis(millis: u128) -> String {
    let secs = (millis / 1_000).min(i64::MAX as u128) as i64;
    let fraction = millis % 1_000;
    let base = format_rfc3339(secs);
    format!("{}.{fraction:03}Z", base.trim_end_matches('Z'))
}

/// Format a UNIX timestamp (seconds, UTC) as `YYYYMMDDThhmmssZ` — colon-free so
/// it is safe in a filename on every platform (Windows forbids `:`). Used to make
/// the session id sortable by start time.
pub fn format_compact_utc(secs: i64) -> String {
    let (y, mo, d, h, mi, s) = civil_from_epoch(secs);
    format!("{y:04}{mo:02}{d:02}T{h:02}{mi:02}{s:02}Z")
}

/// The RFC 3339 instant a session id records its start at: the id is minted
/// as `<name>-<YYYYMMDDThhmmssZ>-<pid>` ([`format_compact_utc`]), so a session
/// with no closing record yet still has a start to sort by. `None` for an id
/// not in that shape — never a guessed instant.
pub(crate) fn session_started_from_id(id: &str) -> Option<String> {
    let mut parts = id.rsplitn(3, '-');
    let pid = parts.next()?;
    let stamp = parts.next()?;
    parts.next()?;
    let digits = |range: std::ops::Range<usize>| {
        stamp
            .get(range)
            .filter(|part| part.bytes().all(|byte| byte.is_ascii_digit()))
    };
    if pid.is_empty()
        || !pid.bytes().all(|byte| byte.is_ascii_digit())
        || stamp.len() != 16
        || stamp.as_bytes()[8] != b'T'
        || stamp.as_bytes()[15] != b'Z'
    {
        return None;
    }
    let text = format!(
        "{}-{}-{}T{}:{}:{}Z",
        digits(0..4)?,
        digits(4..6)?,
        digits(6..8)?,
        digits(9..11)?,
        digits(11..13)?,
        digits(13..15)?
    );
    // Only a real calendar instant: parsed, then formatted back unchanged, so
    // a month 13 or a February 30th (which the parse would roll over) is none.
    crate::domain::usage::normalize_timestamp(&text)
        .filter(|instant| instant.as_str() == text)
        .map(|_| text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::report::OutputFormat;

    #[test]
    fn a_session_id_carries_its_start_instant() {
        let id = format!("fix-the-bug-{}-4242", format_compact_utc(1_790_700_180));
        assert_eq!(
            session_started_from_id(&id).as_deref(),
            Some(format_rfc3339(1_790_700_180).as_str())
        );
        for foreign in [
            "",
            "name",
            "x-20260929T165809Z",
            "x-2026092T165809Z-1",
            "x-20260929T16580aZ-1",
            "x-20260929T165809Z-p",
            "x-20261329T165809Z-1",
            "x-20260230T165809Z-1",
            "x-20260929T245809Z-1",
            "x-20260929T166009Z-1",
        ] {
            assert_eq!(session_started_from_id(foreign), None, "{foreign}");
        }
    }

    #[test]
    fn slug_sanitizes_and_collapses() {
        assert_eq!(
            project_slug("/home/user/My Project"),
            "home-user-My-Project"
        );
        assert_eq!(project_slug("/a//b/./c"), "a-b-.-c");
        assert_eq!(project_slug("C:\\Users\\me\\proj"), "C-Users-me-proj");
        // A path of only separators still yields a usable directory.
        assert_eq!(project_slug("///"), "project");
        assert_eq!(project_slug(""), "project");
    }

    #[test]
    fn session_name_takes_first_words_lowercased() {
        assert_eq!(
            session_name("Fix the login redirect bug please now urgently"),
            "fix-the-login-redirect-bug-please"
        );
        assert_eq!(
            session_name("Refactor!! the (parser)."),
            "refactor-the-parser"
        );
    }

    #[test]
    fn session_name_falls_back_when_empty_or_wordless() {
        assert_eq!(session_name(""), "session");
        assert_eq!(session_name("   \n\t  "), "session");
        assert_eq!(session_name("!!! ??? ..."), "session");
    }

    #[test]
    fn session_name_is_length_capped_without_trailing_dash() {
        let long = "aaaa bbbb cccc dddd eeee ffff gggg hhhh iiii jjjj kkkk";
        let name = session_name(long);
        assert!(name.len() <= 48, "{name} too long");
        assert!(!name.ends_with('-'), "{name} ends with a dash");
    }

    #[test]
    fn sanitize_name_matches_derived_shape() {
        assert_eq!(sanitize_name("My Release v2!"), "my-release-v2");
        assert_eq!(sanitize_name("   "), "session");
        assert_eq!(sanitize_name("---"), "session");
    }

    #[test]
    fn session_name_type_accepts_only_what_a_writer_produces() {
        for name in [
            "session",
            "fix-login-v2",
            &session_name("Refactor!! the (parser)."),
        ] {
            assert_eq!(
                name.parse::<HistorySessionName>().map(|n| n.to_string()),
                Ok(name.to_string())
            );
        }
        for name in [
            "", "Name", "my name", "../name", "my--name", "-name", "name-", "name\n",
        ] {
            assert_eq!(
                name.parse::<HistorySessionName>(),
                Err(HistorySessionNameError),
                "{name:?}"
            );
        }
        assert_eq!(
            HistorySessionName::sanitize("My Release v2!").as_str(),
            "my-release-v2"
        );
        assert_eq!(HistorySessionName::sanitize("").as_str(), "session");
    }

    #[test]
    fn a_session_selector_is_a_minted_id_or_a_sanitized_name() {
        assert_eq!(
            "release-20260929T165809Z-4242".parse(),
            "release-20260929T165809Z-4242"
                .parse::<HistorySessionId>()
                .map(HistorySessionSelector::Id)
        );
        for leap in ["release-20280229T000000Z-1", "release-20000229T235959Z-1"] {
            assert!(
                matches!(leap.parse(), Ok(HistorySessionSelector::Id(_))),
                "{leap}"
            );
        }
        assert_eq!(
            "release-check".parse(),
            Ok(HistorySessionSelector::Name(HistorySessionName::sanitize(
                "release-check"
            )))
        );
        for bad in [
            "",
            "Release",
            "release check",
            "../release",
            "release-",
            "release\n",
            "Release-20260929T165809Z-1",
            "release-2026092T165809Z-1",
            "release-20260929T165809Z-",
            "-20260929T165809Z-1",
            "release-20260230T165809Z-1",
            "release-20260229T165809Z-1",
            "release-19000229T165809Z-1",
            "release-20261301T165809Z-1",
            "release-20260929T245809Z-1",
            "release-20260929T165860Z-1",
        ] {
            assert_eq!(
                bad.parse::<HistorySessionSelector>(),
                Err(HistorySessionSelectorError),
                "{bad:?}"
            );
        }
    }

    #[test]
    fn an_event_line_with_an_unsanitized_session_name_is_refused() {
        let line = |name: &str| {
            serde_json::json!({
                "type": "event", "schema_version": "1.1",
                "run_id": "0198f0d0-7b31-7000-8000-00000000003c",
                "harness": "codex", "harness_id": "codex",
                "event": {"kind": "message", "name": null, "input": null, "output": "On it.",
                          "index": 0, "tool_call_id": null},
                "session_name": name,
            })
        };
        let accepted = serde_json::from_value::<HistoryLine>(line("fix-login")).unwrap();
        let HistoryLine::Event(event) = accepted else {
            panic!("an event line parses as an event")
        };
        assert_eq!(
            event.session_name.as_ref().map(HistorySessionName::as_str),
            Some("fix-login")
        );
        for bad in ["", "Fix Login", "../fix"] {
            assert!(
                serde_json::from_value::<HistoryLine>(line(bad)).is_err(),
                "{bad:?}"
            );
        }
    }

    #[test]
    fn civil_from_epoch_matches_known_instants() {
        assert_eq!(civil_from_epoch(0), (1970, 1, 1, 0, 0, 0));
        // 2026-07-07T13:14:15Z
        assert_eq!(civil_from_epoch(1_783_430_055), (2026, 7, 7, 13, 14, 15));
        // A leap day: 2024-02-29T23:59:59Z.
        assert_eq!(civil_from_epoch(1_709_251_199), (2024, 2, 29, 23, 59, 59));
        // Pre-epoch (negative) stays exact: 1969-12-31T23:59:59Z.
        assert_eq!(civil_from_epoch(-1), (1969, 12, 31, 23, 59, 59));
    }

    #[test]
    fn formatters_render_both_shapes() {
        assert_eq!(format_rfc3339(1_783_430_055), "2026-07-07T13:14:15Z");
        assert_eq!(format_compact_utc(1_783_430_055), "20260707T131415Z");
        // Colon-free compact form is safe as a filename component.
        assert!(!format_compact_utc(1_783_430_055).contains(':'));
    }

    fn result() -> RunResult {
        RunResult {
            harness: "claude-code".to_string(),
            variant: None,
            harness_id: "claude-code".to_string(),
            bin: "claude".to_string(),
            available: true,
            status: Status::Ok,
            prompt: None,
            model: None,
            observed_model: None,
            exit_code: Some(0),
            duration_ms: Some(42),
            telemetry: Some(
                crate::domain::report::ExecutionTelemetry::ProviderMeasured {
                    started_at: "2026-07-07T13:14:14.958Z".parse().unwrap(),
                    finished_at: Some("2026-07-07T13:14:15.000Z".parse().unwrap()),
                    model_ms: Some(42),
                    tool_ms: Some(0),
                    time_to_first_token_ms: Some(10),
                },
            ),
            command: vec!["claude".to_string()],
            output_format: OutputFormat::Json,
            text: Some("hello".to_string()),
            text_source: Some("json:result".to_string()),
            usage: Usage::default(),
            usage_source: None,
            session_id: Some("abc-123".to_string()),
            events: None,
            events_source: None,
            structured: None,
            schema_valid: None,
            schema_attempts: None,
            schema_error: None,
            failure_kind: None,
            work: None,
            failure_kind_source: None,
            stdout: "hello".to_string(),
            stderr: String::new(),
            error: None,
        }
    }

    #[test]
    fn from_result_maps_normalized_signals() {
        let r = result();
        let rec = HistoryRecord::from_result(
            HistoryId::legacy(b"record-1"),
            "fix-bug-20260707T131415Z-9",
            "fix-bug",
            &parse_labels(["graph=deploy"]).unwrap(),
            "/home/user/proj",
            "2026-07-07T13:14:15Z".to_string(),
            PermissionMode::Bypass,
            Some("sonnet"),
            "fix the bug",
            &r,
        );
        assert_eq!(rec.schema_version, PREVIOUS_CURRENT_SCHEMA_VERSION);
        assert_eq!(rec.session, "fix-bug-20260707T131415Z-9");
        assert_eq!(rec.name, "fix-bug");
        assert_eq!(rec.labels.as_map().get("graph").unwrap(), "deploy");
        assert_eq!(rec.project, "/home/user/proj");
        assert_eq!(rec.harness, "claude-code");
        assert_eq!(rec.model.as_deref(), Some("sonnet"));
        assert_eq!(rec.session_id.as_deref(), Some("abc-123"));
        assert_eq!(rec.status, Status::Ok);
        // No per-result prompt → the run prompt is used.
        assert_eq!(rec.prompt, "fix the bug");
    }

    #[test]
    fn codex_provider_record_matches_the_pre_observed_timing_bytes() {
        let mut r = result();
        r.harness = "codex".to_string();
        r.harness_id = "codex".to_string();
        r.bin = "codex".to_string();
        let rec = HistoryRecord::from_result(
            HistoryId::legacy(b"codex-provider-golden"),
            "measure-20260707T131415Z-9",
            "measure",
            &parse_labels(["graph=deploy"]).unwrap(),
            "/home/user/proj",
            "2026-07-07T13:14:15Z".to_string(),
            PermissionMode::Bypass,
            Some("gpt-5"),
            "measure the run",
            &r,
        );

        let actual = serde_json::to_string(&rec).unwrap();
        let expected = include_str!("../../../../tests/fixtures/history-codex-v11.json").trim_end();
        assert_eq!(actual.as_bytes(), expected.as_bytes());
    }

    #[test]
    fn from_result_prefers_the_per_result_batch_prompt() {
        let mut r = result();
        r.prompt = Some("batch prompt 2".to_string());
        let rec = HistoryRecord::from_result(
            HistoryId::legacy(b"record-2"),
            "s",
            "n",
            &HistoryLabels::default(),
            "/p",
            "t".to_string(),
            PermissionMode::Default,
            None,
            "run prompt",
            &r,
        );
        assert_eq!(rec.prompt, "batch prompt 2");
        assert_eq!(rec.model, None);
    }

    #[test]
    fn labels_validate_and_later_values_win() {
        let labels = parse_labels(["graph=release", "task=build", "task=test"]).unwrap();
        assert_eq!(labels.as_map().get("graph").unwrap(), "release");
        assert_eq!(labels.as_map().get("task").unwrap(), "test");

        for invalid in ["missing-equals", "=value", "bad/key=value", "key="] {
            assert!(parse_label(invalid).is_err(), "{invalid} should fail");
        }
        assert!(parse_label("key=line\nbreak").is_err());
        assert!(parse_label(&format!("key={}", "x".repeat(LABEL_VALUE_MAX + 1))).is_err());
    }

    #[test]
    fn label_lengths_are_bounded_in_characters_not_bytes() {
        // The bound is stated in characters because that is the unit the JSON
        // Schema the SDKs are generated from can express. An astral character is
        // four UTF-8 bytes, so a byte bound would reject this at a quarter of the
        // documented limit.
        let astral = "🚀".repeat(LABEL_VALUE_MAX);
        assert_eq!(astral.len(), LABEL_VALUE_MAX * 4);
        assert!(parse_label(&format!("key={astral}")).is_ok());

        let over = "🚀".repeat(LABEL_VALUE_MAX + 1);
        let error = parse_label(&format!("key={over}")).expect_err("one character too many");
        assert!(error.contains("exceeds 256 characters"), "{error}");

        assert!(parse_label(&format!("{}=value", "k".repeat(LABEL_KEY_MAX))).is_ok());
        assert!(parse_label(&format!("{}=value", "k".repeat(LABEL_KEY_MAX + 1))).is_err());
    }

    #[test]
    fn label_values_reject_every_character_rust_calls_control() {
        // Including the C1 block, which the schema's allow-list once let through
        // while this runtime check rejected it.
        for control in ['\u{0}', '\u{1f}', '\u{7f}', '\u{80}', '\u{85}', '\u{9f}'] {
            assert!(control.is_control(), "{control:?} must be a control char");
            assert!(
                parse_label(&format!("key=a{control}b")).is_err(),
                "{control:?} must be rejected"
            );
        }
        // A lone trailing newline is the spelling a `$`-anchored schema pattern
        // would miss under a regex engine whose `$` also matches before one.
        assert!(parse_label("key=value\n").is_err());
        // Neighbours of the control ranges stay valid.
        assert!(parse_label("key=caf\u{e9} \u{7e}\u{a0}").is_ok());
    }

    #[test]
    fn history_id_accepts_only_canonical_hyphenated_text() {
        let canonical = "01931b0f-4a3c-7cde-bf12-3456789abcde";
        assert_eq!(
            canonical.parse::<HistoryId>().unwrap().to_string(),
            canonical
        );
        // Upper-case hex names the same id; every other spelling `Uuid::parse_str`
        // would take is not the canonical text the schema promises.
        assert_eq!(
            canonical
                .to_ascii_uppercase()
                .parse::<HistoryId>()
                .unwrap()
                .to_string(),
            canonical
        );
        for rejected in [
            "01931b0f4a3c7cdebf123456789abcde",       // simple, unhyphenated
            "{01931b0f-4a3c-7cde-bf12-3456789abcde}", // braced
            "urn:uuid:01931b0f-4a3c-7cde-bf12-3456789abcde", // URN
            "01931b0f-4a3c-7cde-bf12-3456789abcde\n", // trailing newline
            "01931b0f-4a3c-7cde-0f12-3456789abcde",   // NCS variant
            "01931b0f-4a3c-7cde-cf12-3456789abcde",   // Microsoft variant
            "01931b0f-4a3c-0cde-bf12-3456789abcde",   // undefined version
            "01931b0f-4a3c-9cde-bf12-3456789abcde",   // undefined version
            "00000000-0000-0000-0000-000000000000",   // nil
            "not-a-uuid",
        ] {
            assert!(
                rejected.parse::<HistoryId>().is_err(),
                "{rejected} must be rejected"
            );
        }
    }

    #[test]
    fn schemas_state_exactly_what_the_runtime_enforces() {
        // These are hand-written, so nothing but a test keeps them in step with
        // `validate_label` and `HistoryId::from_str` above. Every SDK's validator
        // is generated from them.
        let labels = schemars::schema_for!(HistoryLabels);
        let labels = labels.as_value();
        let value = &labels["additionalProperties"];
        assert_eq!(value["minLength"], 1);
        assert_eq!(value["maxLength"], LABEL_VALUE_MAX);
        assert_eq!(value["not"]["pattern"], LABEL_VALUE_FORBIDDEN_PATTERN);
        // An anchored allow-list would let a trailing newline through a regex
        // engine whose `$` matches before one, so there must be no `pattern`.
        assert!(value.get("pattern").is_none(), "{value}");

        let key = &labels["propertyNames"];
        assert_eq!(key["maxLength"], LABEL_KEY_MAX);
        assert_eq!(key["pattern"], LABEL_KEY_START_PATTERN);
        assert_eq!(key["not"]["pattern"], LABEL_KEY_FORBIDDEN_PATTERN);

        let id = schemars::schema_for!(HistoryId);
        let id = id.as_value();
        assert_eq!(id["pattern"], UUID_PATTERN);
        assert_eq!(id["minLength"], UUID_LEN);
        assert_eq!(id["maxLength"], UUID_LEN);

        let failure = schemars::schema_for!(FailureText);
        let failure = failure.as_value();
        assert_eq!(failure["minLength"], 1);
        assert_eq!(failure["maxLength"], ERROR_MAX);
        // The bound is a character count in both places: an astral-heavy message
        // at the limit must read back, or an SDK would refuse what the CLI wrote.
        let astral = "\u{1F600}".repeat(ERROR_MAX);
        assert_eq!(
            FailureText::normalized(&astral).map(|text| text.as_str().chars().count()),
            Some(ERROR_MAX)
        );
        assert!(astral.parse::<FailureText>().is_ok());
    }

    #[test]
    fn minted_history_ids_round_trip_through_the_public_contract() {
        // A migrated legacy record is the one id minted in this pure layer, so the
        // text it renders must be text the same contract accepts back.
        let id = HistoryId::legacy(b"stable-key");
        assert_eq!(id.as_uuid().get_version_num(), 5);
        assert_eq!(id.to_string().parse::<HistoryId>().unwrap(), id);
    }

    #[test]
    fn current_record_requires_a_valid_id_but_tolerates_additive_fields() {
        let current = HistoryRecord::from_result(
            HistoryId::legacy(b"current"),
            "session",
            "name",
            &HistoryLabels::default(),
            "/project",
            "2026-01-01T00:00:00Z".to_string(),
            PermissionMode::Default,
            None,
            "prompt",
            &result(),
        );
        let mut value = serde_json::to_value(&current).unwrap();
        value["future_output_field"] = serde_json::json!({ "accepted": true });
        assert_eq!(
            serde_json::from_value::<HistoryRecord>(value).unwrap(),
            current
        );
        let mut inconsistent = serde_json::to_value(&current).unwrap();
        inconsistent["harness_id"] = Value::String("codex:work".to_string());
        assert!(serde_json::from_value::<HistoryRecord>(inconsistent).is_err());
        let mut malformed_variant = serde_json::to_value(&current).unwrap();
        malformed_variant["variant"] = Value::String("bad.name".to_string());
        malformed_variant["harness_id"] = Value::String("claude-code:bad.name".to_string());
        assert!(serde_json::from_value::<HistoryRecord>(malformed_variant).is_err());

        let mut invalid_v03 = serde_json::to_value(&current).unwrap();
        invalid_v03.as_object_mut().unwrap().remove("model_ms");
        assert!(serde_json::from_value::<HistoryRecord>(invalid_v03).is_err());

        let mut unavailable_v03 = serde_json::to_value(&current).unwrap();
        for field in [
            "started_at",
            "finished_at",
            "model_ms",
            "tool_ms",
            "time_to_first_token_ms",
        ] {
            unavailable_v03.as_object_mut().unwrap().remove(field);
        }
        assert!(serde_json::from_value::<HistoryRecord>(unavailable_v03).is_ok());

        let mut invalid_unavailable = serde_json::to_value(&current).unwrap();
        for field in [
            "started_at",
            "model_ms",
            "tool_ms",
            "time_to_first_token_ms",
        ] {
            invalid_unavailable.as_object_mut().unwrap().remove(field);
        }
        invalid_unavailable["finished_at"] = Value::Null;
        invalid_unavailable["events"] = serde_json::json!([{
            "kind": "tool_call", "name": "shell", "input": {}, "output": null, "index": 0,
            "tool_call_id": "call-1", "started_at": "2026-01-01T00:00:00Z",
            "finished_at": null, "duration_ms": null, "status": null
        }]);
        assert!(serde_json::from_value::<HistoryRecord>(invalid_unavailable).is_err());

        let mut previous = serde_json::to_value(&current).unwrap();
        previous["schema_version"] = Value::String("0.2".to_string());
        for field in ["started_at", "finished_at", "model_ms", "tool_ms"] {
            previous.as_object_mut().unwrap().remove(field);
        }
        assert!(serde_json::from_value::<HistoryRecord>(previous).is_err());

        let mut missing = serde_json::to_value(&current).unwrap();
        missing.as_object_mut().unwrap().remove("history_id");
        assert!(serde_json::from_value::<HistoryRecord>(missing).is_err());
        let mut malformed = serde_json::to_value(current).unwrap();
        malformed["history_id"] = Value::String("not-a-uuid".to_string());
        assert!(serde_json::from_value::<HistoryRecord>(malformed).is_err());

        let mut unsupported = serde_json::to_value(HistoryRecord::from_result(
            HistoryId::legacy(b"future"),
            "session",
            "name",
            &HistoryLabels::default(),
            "/project",
            "2026-01-01T00:00:00Z".to_string(),
            PermissionMode::Default,
            None,
            "prompt",
            &result(),
        ))
        .unwrap();
        unsupported["schema_version"] = Value::String("9.9".to_string());
        assert!(serde_json::from_value::<HistoryRecord>(unsupported).is_err());
    }

    /// A codex run — a harness that *does* declare a provider trace, so timing is
    /// normally mandatory — killed before it produced any answer.
    fn failed_traced_result() -> RunResult {
        RunResult {
            harness: "codex".to_string(),
            harness_id: "codex".to_string(),
            bin: "codex".to_string(),
            status: Status::Nonzero,
            exit_code: Some(1),
            telemetry: None,
            text: None,
            text_source: None,
            session_id: None,
            stdout: String::new(),
            stderr: "Error: insufficient_quota".to_string(),
            failure_kind: Some(FailureKind::Quota),
            ..result()
        }
    }

    fn record_of(r: &RunResult) -> HistoryRecord {
        HistoryRecord::from_result(
            HistoryId::legacy(b"failure"),
            "session",
            "name",
            &HistoryLabels::default(),
            "/project",
            "2026-01-01T00:00:00Z".to_string(),
            PermissionMode::Default,
            // The result's own model, exactly as `io::history`'s writer passes it.
            r.model.as_deref(),
            "prompt",
            r,
        )
    }

    /// The reading a record owes an unexplained failure, and the two ways it can
    /// be wrong: carried where something already explained the failure, or
    /// offered to a reader whose version never had the field.
    #[test]
    fn an_unclassified_failure_records_what_it_had_to_show_for_itself() {
        let with_work = |r: &RunResult| record_of(r).work;
        // Failed, and nothing said why: the record says what it did instead.
        let nothing_done = RunResult {
            status: Status::Nonzero,
            exit_code: Some(2),
            stderr: "No claude executable found for nodejs 26.5.0".to_string(),
            failure_kind: None,
            ..result()
        }
        .with_work_evidence();
        assert_eq!(with_work(&nothing_done), Some(RunWork::None));
        assert_eq!(
            record_of(&nothing_done).schema_version,
            FIRST_WORK_EVIDENCE_SCHEMA_VERSION,
            "a reading no older reader has moves the record forward"
        );
        // The same failure from a candidate that had already done the work reads
        // the other way — which is the whole distinction the field exists for.
        let worked = RunResult {
            usage: Usage {
                input_tokens: Some(1200),
                ..Usage::default()
            },
            ..nothing_done.clone()
        }
        .with_work_evidence();
        assert_eq!(with_work(&worked), Some(RunWork::Done));
        // A classified failure has already answered the question, so it carries
        // no reading and keeps the version its own kind asks for.
        let classified = record_of(&failed_traced_result());
        assert_eq!(classified.work, None);
        assert_eq!(classified.schema_version, FIRST_ERROR_SCHEMA_VERSION);
        // And a run that worked raises no such question at all.
        assert_eq!(with_work(&result()), None);
    }

    #[test]
    fn a_work_reading_is_refused_where_no_reader_or_no_run_could_carry_it() {
        let unexplained = record_of(
            &RunResult {
                status: Status::Nonzero,
                exit_code: Some(2),
                stderr: "shim failure".to_string(),
                failure_kind: None,
                ..result()
            }
            .with_work_evidence(),
        );
        assert_eq!(unexplained.work, Some(RunWork::None));
        let wire = serde_json::to_value(&unexplained).unwrap();
        assert!(serde_json::from_value::<HistoryRecord>(wire.clone()).is_ok());
        // Offered to a reader whose enum never had the field.
        for version in [
            FIRST_ERROR_SCHEMA_VERSION,
            FIRST_CANCELLED_SCHEMA_VERSION,
            FIRST_PRECONDITION_SCHEMA_VERSION,
        ] {
            let mut older = wire.clone();
            older["schema_version"] = Value::String(version.to_string());
            assert!(
                serde_json::from_value::<HistoryRecord>(older).is_err(),
                "{version} predates the work reading"
            );
        }
        // A record carrying both a kind and a reading is *readable* — the two say
        // two true things — and simply never written; the writer rule is pinned
        // above, and `sdk_schema` says why the reader stops short of it.
        let mut classified = wire.clone();
        classified["failure_kind"] = Value::String("auth".to_string());
        assert!(serde_json::from_value::<HistoryRecord>(classified).is_ok());
        // Carried on a run that did not fail at all, though, is not a question
        // this field answers.
        let mut clean = wire.clone();
        clean["status"] = Value::String("ok".to_string());
        clean["exit_code"] = Value::from(0);
        assert!(serde_json::from_value::<HistoryRecord>(clean).is_err());
        // Nor on the two failures the harness never reached: their status
        // already says the candidate did not run, so a reading there would
        // restate it as a finding.
        for never_ran in ["skipped", "spawn-error"] {
            let mut unreached = wire.clone();
            unreached["status"] = Value::String(never_ran.to_string());
            unreached["exit_code"] = Value::Null;
            assert!(
                serde_json::from_value::<HistoryRecord>(unreached).is_err(),
                "{never_ran} says on its own that nothing ran"
            );
        }
        // ...and such a record is written without one, so it keeps a version an
        // older reader can still read.
        let skipped = record_of(
            &RunResult {
                status: Status::Skipped,
                available: false,
                exit_code: None,
                duration_ms: None,
                stderr: String::new(),
                error: Some("`claude` not found on PATH".to_string()),
                failure_kind: None,
                ..result()
            }
            .with_work_evidence(),
        );
        assert_eq!(skipped.work, None);
        assert_eq!(skipped.schema_version, FIRST_ERROR_SCHEMA_VERSION);
    }

    /// The model the harness itself reported, carried beside the requested one
    /// and never derived from it: a completed turn records both as equal, a
    /// refused one records the difference, and a path that reports nothing
    /// records nothing — at the version that first had the field.
    #[test]
    fn an_observed_model_is_recorded_beside_the_requested_one_at_its_own_version() {
        let completed = record_of(&RunResult {
            model: Some("gpt-5.6-sol".to_string()),
            observed_model: Some("gpt-5.6-sol".to_string()),
            ..result()
        });
        assert_eq!(completed.model.as_deref(), Some("gpt-5.6-sol"));
        assert_eq!(completed.observed_model.as_deref(), Some("gpt-5.6-sol"));
        assert_eq!(
            completed.schema_version,
            FIRST_MODEL_OBSERVATION_SCHEMA_VERSION
        );
        assert!(completed.complete());
        assert!(HistoryRunRecord::from_record(&completed).valid());

        // A refused turn carries both names and its own gated kind, on top of a
        // failure text — the newest gate among them decides the version.
        let refused = record_of(
            &RunResult {
                model: Some("gpt-5.6-sol".to_string()),
                observed_model: Some("gpt-6-astra".to_string()),
                status: Status::Nonzero,
                exit_code: None,
                failure_kind: Some(FailureKind::ModelMismatch),
                failure_kind_source: Some("jsonrpc:codex-app-server".to_string()),
                error: Some("codex would run this turn under \"gpt-6-astra\"".to_string()),
                ..result()
            }
            .with_work_evidence(),
        );
        assert_eq!(refused.failure_kind, Some(FailureKind::ModelMismatch));
        assert_eq!(refused.observed_model.as_deref(), Some("gpt-6-astra"));
        assert_eq!(refused.model.as_deref(), Some("gpt-5.6-sol"));
        assert_eq!(refused.work, None, "a classified refusal has said why");
        assert_eq!(
            refused.schema_version,
            FIRST_MODEL_OBSERVATION_SCHEMA_VERSION
        );
        assert!(refused.complete());

        // The kind alone — no observed model on the result — still needs the
        // reader that has the value.
        let kind_only = record_of(
            &RunResult {
                status: Status::Nonzero,
                exit_code: None,
                failure_kind: Some(FailureKind::ModelMismatch),
                error: Some("refused".to_string()),
                ..result()
            }
            .with_work_evidence(),
        );
        assert_eq!(
            kind_only.schema_version,
            FIRST_MODEL_OBSERVATION_SCHEMA_VERSION
        );

        // A path that reports no model records none, and is not derived from
        // the requested one — the field is the harness's own claim or nothing.
        let unreported = record_of(&RunResult {
            model: Some("gpt-5.6-sol".to_string()),
            ..result()
        });
        assert_eq!(unreported.observed_model, None);
        assert_eq!(unreported.schema_version, PREVIOUS_CURRENT_SCHEMA_VERSION);
        let wire = serde_json::to_value(&unreported).unwrap();
        assert!(
            wire.get("observed_model").is_none(),
            "omitted on the wire when absent: {wire}"
        );
        assert!(
            serde_json::to_value(HistoryRunRecord::from_record(&unreported))
                .unwrap()
                .get("observed_model")
                .is_none()
        );

        // Offered to a reader whose version never had the field, it is refused
        // rather than read back as a record that version never wrote.
        let wire = serde_json::to_value(&completed).unwrap();
        for version in [
            FIRST_WORK_EVIDENCE_SCHEMA_VERSION,
            FIRST_PRECONDITION_SCHEMA_VERSION,
        ] {
            let mut older = wire.clone();
            older["schema_version"] = Value::String(version.to_string());
            assert!(
                serde_json::from_value::<HistoryRecord>(older.clone()).is_err(),
                "{version} predates the observed model"
            );
            let mut run: HistoryRunRecord = serde_json::from_value(older).unwrap();
            run.schema_version = version.to_string();
            assert!(!run.valid(), "{version} predates the observed model");
        }
        assert!(serde_json::from_value::<HistoryRecord>(wire).is_ok());
    }

    #[test]
    fn a_cancelled_run_records_at_the_version_that_first_understood_it() {
        // `cancelled` is a status value no v1.3 reader has, so a record carrying
        // it must declare v1.4 — otherwise an older consumer would read the
        // version as a promise it can parse the record, and then choke on it.
        let cancelled = record_of(&RunResult {
            status: Status::Cancelled,
            exit_code: None,
            error: Some("`codex` was cancelled".to_string()),
            ..failed_traced_result()
        });
        assert_eq!(cancelled.status, Status::Cancelled);
        assert_eq!(cancelled.schema_version, FIRST_CANCELLED_SCHEMA_VERSION);
        assert!(run_failed(cancelled.status));
        assert!(cancelled.complete());
        // Round-trips: the writer's shape is the reader's shape.
        let wire = serde_json::to_value(&cancelled).unwrap();
        assert_eq!(wire["status"], "cancelled");
        assert_eq!(
            serde_json::from_value::<HistoryRecord>(wire).unwrap(),
            cancelled
        );
    }

    #[test]
    fn an_unknown_session_run_records_at_the_version_that_first_understood_it() {
        // `session_not_found` is a `failure_kind` value no v1.4 reader has, so a
        // record carrying it must declare v1.5 — the same promise the `cancelled`
        // status makes, on the other gated enum.
        let refused = record_of(&RunResult {
            failure_kind: Some(FailureKind::SessionNotFound),
            work: None,
            failure_kind_source: Some("stderr".to_string()),
            error: Some("No conversation found with session ID: s-1".to_string()),
            ..failed_traced_result()
        });
        assert_eq!(refused.failure_kind, Some(FailureKind::SessionNotFound));
        assert_eq!(
            refused.schema_version,
            FIRST_SESSION_NOT_FOUND_SCHEMA_VERSION
        );
        assert!(refused.complete());
        let wire = serde_json::to_value(&refused).unwrap();
        assert_eq!(wire["failure_kind"], "session_not_found");
        assert_eq!(
            serde_json::from_value::<HistoryRecord>(wire.clone()).unwrap(),
            refused
        );
        // A record that claims an older reader can parse it is refused, not read.
        let mut backdated = wire;
        backdated["schema_version"] = Value::String(FIRST_CANCELLED_SCHEMA_VERSION.to_string());
        assert!(serde_json::from_value::<HistoryRecord>(backdated).is_err());
    }

    #[test]
    fn a_failed_run_records_with_whatever_telemetry_its_failure_left() {
        let failed = record_of(&failed_traced_result());
        // Absent timing: the run never reached the boundary a trace is measured
        // between, so absence is the honest reading — not a reason to drop it.
        assert!(failed.complete());
        assert_eq!(failed.failure_kind, Some(FailureKind::Quota));
        // The harness's own stderr is what an operator reads when the classified
        // kind is not enough, so it rides along — and forces v1.3.
        assert_eq!(
            failed.error.as_ref().map(FailureText::as_str),
            Some("Error: insufficient_quota")
        );
        assert_eq!(failed.schema_version, FIRST_ERROR_SCHEMA_VERSION);

        // Events the failure interrupted keep their observed boundaries; the
        // "no run timing means no event timing" rule is a success-run rule.
        let mut interrupted = serde_json::to_value(&failed).unwrap();
        interrupted["events"] = serde_json::json!([{
            "kind": "tool_call", "name": "shell", "input": {}, "output": null, "index": 0,
            "tool_call_id": "call-1", "started_at": "2026-01-01T00:00:00Z",
            "finished_at": null, "duration_ms": null, "status": "interrupted"
        }]);
        assert!(serde_json::from_value::<HistoryRecord>(interrupted).is_ok());

        // The run line written to the JSONL file reads back by the same rule.
        let line =
            serde_json::to_value(HistoryLine::Run(HistoryRunRecord::from_record(&failed))).unwrap();
        assert_eq!(
            line["error"],
            Value::String("Error: insufficient_quota".to_string())
        );
        assert!(serde_json::from_value::<HistoryLine>(line).is_ok());
    }

    /// The carve-out reads the run's own verdict, never its extracted text. A
    /// turn cut short can still leave partial assistant text behind, and treating
    /// that as "the provider succeeded" would refuse exactly the records this
    /// exists to keep.
    #[test]
    fn a_failure_that_left_partial_text_is_still_recordable() {
        let partial_answer = record_of(&RunResult {
            text: Some("I was still thinking when".to_string()),
            text_source: Some("json:codex-agent-message".to_string()),
            ..failed_traced_result()
        });
        assert!(partial_answer.complete());
        assert_eq!(
            partial_answer.text.as_deref(),
            Some("I was still thinking when")
        );
    }

    #[test]
    fn a_run_that_succeeded_still_needs_complete_telemetry() {
        // A clean exit is the run's own claim that it worked, so a trace-capable
        // harness that reported no timing for it is drift, not an honest failure.
        let worked = record_of(&RunResult {
            status: Status::Ok,
            exit_code: Some(0),
            stderr: String::new(),
            failure_kind: None,
            text: Some("the answer is 42".to_string()),
            ..failed_traced_result()
        });
        assert!(!worked.complete());
        assert!(worked.error.is_none());
        // ... and one whose answer was never extracted is no different.
        let silent = record_of(&RunResult {
            status: Status::Ok,
            exit_code: Some(0),
            stderr: String::new(),
            failure_kind: None,
            ..failed_traced_result()
        });
        assert!(!silent.complete());
    }

    /// A run cut short leaves the invocation bounds the runner watched directly,
    /// with no provider/tool split derived from a transcript that never finished.
    /// That partial measurement is legible on a failed run and corrupt on one
    /// that succeeded — where the provider answered, so the split it never wrote
    /// should have been there.
    #[test]
    fn invocation_bounds_without_a_split_belong_to_a_run_that_was_cut_short() {
        let mut partial = serde_json::to_value(record_of(&failed_traced_result())).unwrap();
        partial["started_at"] = Value::String("2026-01-01T00:00:00Z".to_string());
        let read = serde_json::from_value::<HistoryRecord>(partial.clone()).unwrap();
        assert_eq!(read.started_at.as_deref(), Some("2026-01-01T00:00:00Z"));
        assert!(read.model_ms.is_none() && read.tool_ms.is_none());
        // A first-token offset within the run's duration rides along...
        let mut with_offset = partial.clone();
        with_offset["time_to_first_token_ms"] = Value::from(7);
        assert!(serde_json::from_value::<HistoryRecord>(with_offset).is_ok());
        // ...but not one the run was never long enough to contain.
        let mut impossible = partial.clone();
        impossible["time_to_first_token_ms"] = Value::from(u64::from(u32::MAX));
        assert!(serde_json::from_value::<HistoryRecord>(impossible).is_err());

        // The written run line reads back by the same rule.
        let mut line =
            serde_json::to_value(HistoryLine::Run(HistoryRunRecord::from_record(&read))).unwrap();
        assert!(serde_json::from_value::<HistoryLine>(line.clone()).is_ok());

        // A classified provider failure may exit cleanly and still have only
        // invocation-bound timing; the failure kind, not the process exit code,
        // is what says the turn was cut short.
        let mut clean_classified = partial.clone();
        clean_classified["status"] = Value::String("ok".to_string());
        clean_classified["exit_code"] = Value::from(0);
        clean_classified["error"] = Value::Null;
        assert!(serde_json::from_value::<HistoryRecord>(clean_classified).is_ok());

        // The same shape on a run that succeeded is refused, record and line alike.
        for succeeded in ["ok", "planned"] {
            let mut worked = partial.clone();
            worked["status"] = Value::String(succeeded.to_string());
            worked["failure_kind"] = Value::Null;
            worked.as_object_mut().unwrap().remove("error");
            assert!(
                serde_json::from_value::<HistoryRecord>(worked).is_err(),
                "{succeeded}"
            );
        }
        line["status"] = Value::String("ok".to_string());
        line["failure_kind"] = Value::Null;
        line.as_object_mut().unwrap().remove("error");
        assert!(serde_json::from_value::<HistoryLine>(line).is_err());
    }

    /// A measurement is either whole, stopped at the bounds, or absent. Anything
    /// else is incoherent rather than partial — a total with no counterpart, an
    /// offset with nothing to measure it from — and no status makes it legible.
    #[test]
    fn an_incoherent_half_measurement_is_refused_whatever_the_run_did() {
        let base = serde_json::to_value(record_of(&failed_traced_result())).unwrap();
        let incoherent = [
            (
                "model total with no tool total",
                vec![
                    (
                        "started_at",
                        Value::String("2026-01-01T00:00:00Z".to_string()),
                    ),
                    ("model_ms", Value::from(1)),
                ],
            ),
            (
                "tool total with no model total",
                vec![
                    (
                        "started_at",
                        Value::String("2026-01-01T00:00:00Z".to_string()),
                    ),
                    ("tool_ms", Value::from(1)),
                ],
            ),
            (
                "first-token offset with no start",
                vec![("time_to_first_token_ms", Value::from(1))],
            ),
            (
                "finish with no start",
                vec![(
                    "finished_at",
                    Value::String("2026-01-01T00:00:01Z".to_string()),
                )],
            ),
        ];
        for (tag, fields) in incoherent {
            for status in ["nonzero", "timeout", "ok"] {
                let mut record = base.clone();
                record["status"] = Value::String(status.to_string());
                for (field, value) in &fields {
                    record[*field] = value.clone();
                }
                assert!(
                    serde_json::from_value::<HistoryRecord>(record).is_err(),
                    "{tag} on {status}"
                );
            }
        }
    }

    #[test]
    fn failure_text_is_normalized_bounded_and_never_taken_from_a_run_that_worked() {
        let text = |status, error, stderr| {
            failure_text(status, error, stderr).map(|text| text.as_str().to_string())
        };
        // oneharness's own diagnostic wins over the child's stderr: on a spawn
        // failure or a timeout it is the only account of what happened.
        assert_eq!(
            text(Status::SpawnError, Some("  could not spawn  "), "noise").as_deref(),
            Some("could not spawn")
        );
        // A failed run with no diagnostic of its own falls back to stderr.
        assert_eq!(
            text(Status::Nonzero, None, "\n401 Unauthorized\n").as_deref(),
            Some("401 Unauthorized")
        );
        // A run that worked never contributes its stderr chatter.
        assert_eq!(text(Status::Ok, None, "warning: deprecated"), None);
        // Nothing to say stays absent rather than becoming an empty string.
        assert_eq!(text(Status::Nonzero, None, "   \n"), None);
        // A runaway stderr is cut to the bound, with the cut marked.
        let flood = "x".repeat(ERROR_MAX * 2);
        let bounded = text(Status::Timeout, None, &flood).unwrap();
        assert_eq!(bounded.chars().count(), ERROR_MAX);
        assert!(bounded.ends_with(ERROR_TRUNCATION_MARKER));
        // Exactly at the bound, nothing is marked.
        let exact = "y".repeat(ERROR_MAX);
        assert_eq!(
            text(Status::Timeout, None, &exact).as_deref(),
            Some(&*exact)
        );
        // An invalid value is not constructible, so a record cannot carry one.
        assert_eq!("".parse::<FailureText>(), Err(FailureTextError));
        assert_eq!(
            "z".repeat(ERROR_MAX + 1).parse::<FailureText>(),
            Err(FailureTextError)
        );
    }

    #[test]
    fn failure_text_is_gated_to_the_version_that_introduced_it() {
        let failed = record_of(&failed_traced_result());
        let mut older = serde_json::to_value(&failed).unwrap();
        for version in [
            FIRST_EVENT_SCHEMA_VERSION,
            PREVIOUS_CURRENT_SCHEMA_VERSION,
            OBSERVED_TIMING_SCHEMA_VERSION,
        ] {
            older["schema_version"] = Value::String(version.to_string());
            assert!(
                serde_json::from_value::<HistoryRecord>(older.clone()).is_err(),
                "{version} predates the error field"
            );
        }
        // An empty or over-long value is not what the contract promises either.
        let mut empty = serde_json::to_value(&failed).unwrap();
        empty["error"] = Value::String(String::new());
        assert!(serde_json::from_value::<HistoryRecord>(empty).is_err());
        let mut long = serde_json::to_value(&failed).unwrap();
        long["error"] = Value::String("z".repeat(ERROR_MAX + 1));
        assert!(serde_json::from_value::<HistoryRecord>(long).is_err());
    }

    /// `error` is a failure signal, so the record cannot claim one for a run that
    /// reported no failure — but a clean exit that *did* report one (the
    /// deferred-tool dead-end: exit 0, no work done) is exactly the case the
    /// field exists for, and must not be caught by the same rule.
    #[test]
    fn failure_text_belongs_only_to_a_run_that_reported_a_failure() {
        let failed = record_of(&failed_traced_result());
        let mut clean_exit = serde_json::to_value(&failed).unwrap();
        clean_exit["status"] = Value::String("ok".to_string());
        clean_exit["exit_code"] = Value::from(0);
        clean_exit["started_at"] = Value::String("2026-01-01T00:00:00Z".to_string());
        clean_exit["finished_at"] = Value::String("2026-01-01T00:00:01Z".to_string());
        clean_exit["model_ms"] = Value::from(1);
        clean_exit["tool_ms"] = Value::from(0);
        clean_exit["duration_ms"] = Value::from(42);

        let mut unreported = clean_exit.clone();
        unreported["failure_kind"] = Value::Null;
        assert!(serde_json::from_value::<HistoryRecord>(unreported).is_err());

        let deferred = record_of(&RunResult {
            status: Status::Ok,
            exit_code: Some(0),
            stderr: String::new(),
            failure_kind: Some(FailureKind::ToolDeferred),
            error: Some("claude-code deferred a builtin tool call".to_string()),
            ..result()
        });
        assert_eq!(
            deferred.error.as_ref().map(FailureText::as_str),
            Some("claude-code deferred a builtin tool call")
        );
        assert!(deferred.complete());

        // ...and it is the *only* clean exit that gets one. A provider failure
        // classified off a clean run's own output left the harness's words in its
        // transcript, not in a diagnostic, so no other kind justifies the field.
        for other in [
            FailureKind::Auth,
            FailureKind::Quota,
            FailureKind::RateLimit,
            FailureKind::ModelNotFound,
        ] {
            let mut mismatched = clean_exit.clone();
            mismatched["failure_kind"] = serde_json::to_value(other).unwrap();
            assert!(
                serde_json::from_value::<HistoryRecord>(mismatched).is_err(),
                "{other:?} on a clean exit"
            );
        }
    }

    #[test]
    fn readable_versions_are_ordered_so_a_field_gate_can_ask_for_a_minimum() {
        assert!(version_at_least(SCHEMA_VERSION, FIRST_EVENT_SCHEMA_VERSION));
        assert!(version_at_least(
            OBSERVED_TIMING_SCHEMA_VERSION,
            OBSERVED_TIMING_SCHEMA_VERSION
        ));
        assert!(!version_at_least(
            PREVIOUS_CURRENT_SCHEMA_VERSION,
            OBSERVED_TIMING_SCHEMA_VERSION
        ));
        // An unreadable version never satisfies a minimum by accident.
        assert!(!version_at_least("9.9", FIRST_EVENT_SCHEMA_VERSION));
        assert!(!version_at_least(SCHEMA_VERSION, "9.9"));
    }

    #[test]
    fn pre_v1_2_record_rejects_event_timing_provenance() {
        let current = HistoryRecord::from_result(
            HistoryId::legacy(b"pre-v1.2-event-timing"),
            "session",
            "name",
            &HistoryLabels::default(),
            "/project",
            "2026-01-01T00:00:00Z".to_string(),
            PermissionMode::Default,
            None,
            "prompt",
            &result(),
        );
        let mut value = serde_json::to_value(current).unwrap();
        value["events"] = serde_json::json!([{
            "kind": "tool_call",
            "name": "Bash",
            "input": {"command": "true"},
            "output": "",
            "index": 0,
            "tool_call_id": "tool-1",
            "started_at": "2026-01-01T00:00:00.000Z",
            "finished_at": "2026-01-01T00:00:00.001Z",
            "duration_ms": 1,
            "status": "completed",
            "timing_source": "stdout_observed"
        }]);

        assert!(serde_json::from_value::<HistoryRecord>(value.clone()).is_err());
        value["schema_version"] = Value::String(SCHEMA_VERSION.to_string());
        assert!(serde_json::from_value::<HistoryRecord>(value).is_ok());
    }

    // The pointer tests are unix-only because they spell absolute paths as
    // `/state/…`, which Windows does not read as absolute; their helpers are
    // gated beside them so the Windows build carries no dead code.
    #[cfg(unix)]
    fn pointer_session() -> PointerSession {
        PointerSession::new(
            Path::new("/state/history"),
            Path::new("/state/history/home-me-proj/fix-it-20260101T000000Z-42.jsonl"),
            "fix-it",
            "/home/me/proj",
            HistoryLabels::default(),
        )
        .unwrap()
    }

    #[cfg(unix)]
    fn pointer_id() -> HistoryId {
        "0192b2a0-0000-7000-8000-000000000001".parse().unwrap()
    }

    #[cfg(unix)]
    fn pointer_started() -> UtcInstant {
        "2026-01-01T00:00:00Z".parse().unwrap()
    }

    #[cfg(unix)]
    fn identity(text: &str) -> HarnessIdentity {
        text.parse().unwrap()
    }

    #[cfg(unix)]
    #[test]
    fn pointer_session_reads_the_slug_and_stem_off_the_file_and_refuses_a_stray_path() {
        let session = pointer_session();
        assert_eq!(session.history_project, "home-me-proj");
        assert_eq!(session.history_session, "fix-it-20260101T000000Z-42");
        let refused = |dir: &str, file: &str, name: &str, project: &str| {
            PointerSession::new(
                Path::new(dir),
                Path::new(file),
                name,
                project,
                HistoryLabels::default(),
            )
            .unwrap_err()
            .to_string()
        };
        assert!(refused("state", "/state/p/s.jsonl", "n", "/p").contains("absolute"));
        assert!(refused("/state", "/state/p/s.jsonl", "n", "p").contains("project directory"));
        assert!(refused("/state", "/state/p/s.jsonl", "", "/p").contains("name"));
        assert!(refused("/state", "/state/p/s.log", "n", "/p").contains("session log"));
        assert!(refused("/state", "/elsewhere/p/s.jsonl", "n", "/p").contains("below the store"));
        assert!(refused("/state", "/state/p/q/s.jsonl", "n", "/p").contains("below the store"));
    }

    #[cfg(unix)]
    #[test]
    fn pointer_splits_the_harness_id_and_omits_what_is_empty() {
        let bare = HistoryPointer::new(
            &pointer_session(),
            pointer_id(),
            &identity("claude-code"),
            pointer_started(),
        )
        .unwrap();
        assert_eq!(bare.schema_version(), POINTER_SCHEMA_VERSION);
        assert_eq!(bare.harness(), "claude-code");
        assert_eq!(bare.variant(), None);
        assert_eq!(bare.harness_id(), "claude-code");
        let wire = serde_json::to_value(&bare).unwrap();
        assert_eq!(wire["history_id"], "0192b2a0-0000-7000-8000-000000000001");
        assert_eq!(wire["history_project"], "home-me-proj");
        assert_eq!(wire["history_session"], "fix-it-20260101T000000Z-42");
        assert_eq!(wire["started"], "2026-01-01T00:00:00Z");
        assert!(
            wire.get("variant").is_none(),
            "a bare harness has no variant key"
        );
        assert!(
            wire.get("labels").is_none(),
            "empty labels stay off the wire"
        );

        let mut session = pointer_session();
        session.labels = HistoryLabels::new(BTreeMap::from([(
            "graph".to_string(),
            "release".to_string(),
        )]))
        .unwrap();
        let variant = HistoryPointer::new(
            &session,
            pointer_id(),
            &identity("claude-code:primary"),
            pointer_started(),
        )
        .unwrap();
        assert_eq!(variant.harness(), "claude-code");
        assert_eq!(variant.variant(), Some("primary"));
        assert_eq!(variant.harness_id(), "claude-code:primary");
        let wire = serde_json::to_value(&variant).unwrap();
        assert_eq!(wire["variant"], "primary");
        assert_eq!(wire["labels"]["graph"], "release");
        let back: HistoryPointer = serde_json::from_value(wire).unwrap();
        assert_eq!(back, variant);
        // The identity the constructor takes is already parsed, so what a
        // pointer can name is exactly what `HarnessIdentity` admits — no
        // separator inside a variant, no unknown base.
        for bad in [
            "",
            "codex:",
            ":primary",
            "claude-code:primary:extra",
            "not-a-harness",
        ] {
            assert!(
                bad.parse::<HarnessIdentity>().is_err(),
                "`{bad}` is not a harness id"
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn a_line_whose_spellings_disagree_is_not_a_pointer() {
        let good = serde_json::to_value(
            HistoryPointer::new(
                &pointer_session(),
                pointer_id(),
                &identity("claude-code:primary"),
                pointer_started(),
            )
            .unwrap(),
        )
        .unwrap();
        assert!(serde_json::from_value::<HistoryPointer>(good.clone()).is_ok());
        // A later minor still reads; a foreign major does not.
        let mut minor = good.clone();
        minor["schema_version"] = Value::String("1.7".to_string());
        assert!(serde_json::from_value::<HistoryPointer>(minor).is_ok());
        for (key, value) in [
            ("schema_version", serde_json::json!("2.0")),
            ("schema_version", serde_json::json!("1")),
            ("harness_id", serde_json::json!("claude-code")),
            ("harness_id", serde_json::json!("claude-code:primary:extra")),
            ("harness_id", serde_json::json!("not-a-harness:primary")),
            ("harness", serde_json::json!("codex")),
            ("harness", serde_json::json!("")),
            ("variant", serde_json::json!("")),
            ("variant", serde_json::json!("primary:extra")),
            ("history_project", serde_json::json!("other-proj")),
            ("history_session", serde_json::json!("other-session")),
            ("history_dir", serde_json::json!("/elsewhere")),
            (
                "history_file",
                serde_json::json!("/state/history/home-me-proj/fix.txt"),
            ),
            ("project", serde_json::json!("relative/proj")),
            ("name", serde_json::json!("")),
            ("started", serde_json::json!("2026-01-01T00:00:00-04:00")),
            ("history_id", serde_json::json!("not-a-uuid")),
        ] {
            let mut bad = good.clone();
            bad[key] = value.clone();
            assert!(
                serde_json::from_value::<HistoryPointer>(bad).is_err(),
                "{key} = {value} should not read as a pointer"
            );
        }
        // Dropping the variant alone leaves `harness_id` naming one, and the
        // two must agree.
        let mut dropped = good;
        dropped.as_object_mut().unwrap().remove("variant");
        assert!(serde_json::from_value::<HistoryPointer>(dropped).is_err());
    }

    #[test]
    fn an_in_flight_run_reads_back_its_events_in_index_order() {
        let event = |index: usize, kind: &str| -> ActionEvent {
            serde_json::from_value(serde_json::json!({
                "kind": kind, "name": null, "input": null, "output": null,
                "index": index,
            }))
            .unwrap()
        };
        let run_id = "0198f0d0-7b31-7000-8000-000000000002"
            .parse::<HistoryId>()
            .unwrap();
        let run = IncompleteHistoryRun::new(
            run_id,
            "codex".to_string(),
            vec![event(1, "message"), event(0, "reasoning")],
        )
        .unwrap();
        assert_eq!(run.run_id(), run_id);
        assert_eq!(run.harness(), "codex");
        let order: Vec<(usize, &str)> = run
            .events()
            .iter()
            .map(|event| (event.index, event.kind.as_str()))
            .collect();
        assert_eq!(order, [(0, "reasoning"), (1, "message")]);
        assert!(IncompleteHistoryRun::new(run_id, "codex".to_string(), Vec::new()).is_none());
    }
    #[test]
    fn opening_documentation_tracks_the_wire_fields_version_and_file_layout() {
        let matrix: Value = serde_json::from_str(include_str!(
            "../../../../tests/fixtures/sdk-contract-matrix.json"
        ))
        .unwrap();
        let cases = matrix["cases"].as_array().unwrap();
        let opening = &cases
            .iter()
            .find(|case| case["name"] == "history opening line v1.11")
            .unwrap()["value"];
        let stream = &cases
            .iter()
            .find(|case| case["name"] == "stream opening location")
            .unwrap()["value"];
        let opening: HistoryLine = serde_json::from_value(opening.clone()).unwrap();
        let opening = serde_json::to_value(opening).unwrap();
        let begun: RunBegun = serde_json::from_value(stream.clone()).unwrap();
        let begun = serde_json::to_value(begun).unwrap();
        let docs = include_str!("../../../../docs/history-index.md");
        let example = docs
            .split("`{\"type\":\"run-started\"")
            .nth(1)
            .unwrap()
            .split('`')
            .next()
            .unwrap();
        let example: Value =
            serde_json::from_str(&format!("{{\"type\":\"run-started\"{example}")).unwrap();
        assert_eq!(
            example.as_object().unwrap().keys().collect::<Vec<_>>(),
            opening.as_object().unwrap().keys().collect::<Vec<_>>()
        );
        assert_eq!(example["schema_version"], opening["schema_version"]);
        let fields = docs
            .split("<!-- BEGIN RUN-BEGUN FIELDS -->")
            .nth(1)
            .unwrap()
            .split("<!-- END RUN-BEGUN FIELDS -->")
            .next()
            .unwrap();
        let mut names: Vec<_> = fields
            .lines()
            .filter_map(|line| {
                line.strip_prefix("| `")
                    .and_then(|rest| rest.split('`').next())
            })
            .collect();
        names.sort_unstable();
        assert_eq!(
            names,
            begun
                .as_object()
                .unwrap()
                .keys()
                .map(String::as_str)
                .collect::<Vec<_>>()
        );
        let layout =
            format!("<history_dir>/<history_project>/<history_session>.{SESSION_FILE_EXT}");
        assert!(docs.contains(&format!("The file is `{layout}`.")));
    }
}
