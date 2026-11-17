//! Rust-owned schema metadata for the Node SDK's public input contract.
//!
//! The SDK generator reflects this type into both TypeScript and Zod. Keeping
//! the option names here means adding an SDK option cannot leave its static and
//! runtime contracts out of sync. This is metadata only: command orchestration
//! stays in the Node SDK and the CLI command layer.

use std::borrow::Cow;
use std::collections::BTreeMap;
use std::fmt;

use schemars::{generate::SchemaSettings, JsonSchema, Schema, SchemaGenerator};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
#[cfg(test)]
use serde_json::Value;
use thiserror::Error;

use crate::domain::batch::BatchStrategy;
use crate::domain::fallback::RunMode;
use crate::domain::history::{HistoryId, HistoryLabels};
use crate::domain::mode::PermissionMode;
use crate::domain::report::OutputFormat;

/// Generate a schema for a value emitted by oneharness.
///
/// The generator explicitly describes serialization rather than deserialization:
/// nullable fields that are always emitted remain required, while fields marked
/// `skip_serializing_if` stay optional. That distinction lets additive empty
/// fields be omitted without weakening the rest of the output contract.
pub fn schema_for_serialize<T: ?Sized + JsonSchema>() -> Schema {
    SchemaSettings::default()
        .for_serialize()
        .into_generator()
        .into_root_schema_for::<T>()
}

/// The error returned when a value that must carry text is empty.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
#[error("must be a non-empty string")]
pub struct EmptyStringError;

/// A string that cannot be empty.
///
/// Deserialization rejects `""`, so an empty value is not representable past the
/// boundary and no caller-facing code has to re-check for one. The schema is the
/// inline `string` + `minLength: 1`, so the generated TypeScript stays `string`
/// and the generated Zod adds the runtime check.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub struct NonEmptyString(String);

impl NonEmptyString {
    /// Borrow the validated text.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Consume the wrapper and return the validated text.
    pub fn into_string(self) -> String {
        self.0
    }
}

impl TryFrom<String> for NonEmptyString {
    type Error = EmptyStringError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        if value.is_empty() {
            return Err(EmptyStringError);
        }
        Ok(Self(value))
    }
}

impl TryFrom<&str> for NonEmptyString {
    type Error = EmptyStringError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        Self::try_from(value.to_string())
    }
}

impl fmt::Display for NonEmptyString {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for NonEmptyString {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let raw = String::deserialize(deserializer)?;
        Self::try_from(raw).map_err(serde::de::Error::custom)
    }
}

impl JsonSchema for NonEmptyString {
    fn inline_schema() -> bool {
        true
    }

    fn schema_name() -> Cow<'static, str> {
        Cow::Borrowed("NonEmptyString")
    }

    fn json_schema(_generator: &mut SchemaGenerator) -> Schema {
        schemars::json_schema!({
            "type": "string",
            "minLength": 1,
        })
    }
}

/// The literal `true`.
///
/// This is what makes "select the most recent session" a value rather than a
/// flag to re-read: `last: false` does not select, so it is not a value of this
/// type and cannot satisfy the variant that requires it. The schema is the
/// inline `const: true`, so the generated TypeScript is the literal `true` and
/// the generated Zod is `z.literal(true)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct LiteralTrue;

impl Serialize for LiteralTrue {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_bool(true)
    }
}

impl<'de> Deserialize<'de> for LiteralTrue {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        if bool::deserialize(deserializer)? {
            Ok(Self)
        } else {
            Err(serde::de::Error::custom("must be `true`"))
        }
    }
}

impl JsonSchema for LiteralTrue {
    fn inline_schema() -> bool {
        true
    }

    fn schema_name() -> Cow<'static, str> {
        Cow::Borrowed("LiteralTrue")
    }

    fn json_schema(_generator: &mut SchemaGenerator) -> Schema {
        schemars::json_schema!({
            "type": "boolean",
            "const": true,
        })
    }
}

/// The literal `false`.
///
/// The [`LiteralTrue`] counterpart, for the other side of a discriminated pair:
/// `history clear` prints one of two documents and `dry_run` is what tells them
/// apart, so each frame pins its own value rather than carrying a `bool` either
/// constructor could set wrong.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct LiteralFalse;

impl Serialize for LiteralFalse {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_bool(false)
    }
}

impl<'de> Deserialize<'de> for LiteralFalse {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        if bool::deserialize(deserializer)? {
            Err(serde::de::Error::custom("must be `false`"))
        } else {
            Ok(Self)
        }
    }
}

impl JsonSchema for LiteralFalse {
    fn inline_schema() -> bool {
        true
    }

    fn schema_name() -> Cow<'static, str> {
        Cow::Borrowed("LiteralFalse")
    }

    fn json_schema(_generator: &mut SchemaGenerator) -> Schema {
        schemars::json_schema!({
            "type": "boolean",
            "const": false,
        })
    }
}

/// Options accepted by `OneHarness.run()` in the published Node SDK.
///
/// Unknown fields are rejected because the SDK cannot forward an option it does
/// not understand. This differs deliberately from output contracts, whose Zod
/// schemas preserve unknown fields for forward compatibility.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[schemars(rename = "RunOptions")]
pub struct RunOptions {
    /// The user message sent to the selected harnesses.
    pub prompt: NonEmptyString,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Vec<String>")]
    pub harnesses: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Vec<String>")]
    pub models: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "String")]
    pub system: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "String")]
    pub reasoning: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "String")]
    pub resume: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "String")]
    pub session: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "bool")]
    pub fork: Option<bool>,
    // The four enum-typed options here carry NO doc comment on purpose, and a
    // `//` comment rather than a `///` one for the same reason: a `$ref` with a
    // sibling `description` is merged inline by json-schema-to-typescript
    // instead of resolving to the named type, so the generated SDK loses
    // `PermissionMode`/`OutputFormat`/`RunMode`/`BatchStrategy` as exported
    // names and `zod.ts` then fails to import them. Each enum's own definition
    // carries the description.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "PermissionMode")]
    pub mode: Option<PermissionMode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "String")]
    pub cwd: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "u64")]
    pub timeout_seconds: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "bool")]
    pub events: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "bool")]
    pub history: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "String")]
    pub history_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "String")]
    pub history_dir: Option<String>,
    /// The run's pointer file: with history on, every harness run this run
    /// begins appends one `HistoryPointer` line here saying where its session
    /// went (see `historyPointers()`), the store itself staying where
    /// `historyDir` puts it. An empty path is refused rather than read as
    /// unset — leave the option out to name no file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "NonEmptyString")]
    pub history_pointer_file: Option<NonEmptyString>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "HistoryLabels")]
    pub history_labels: Option<HistoryLabels>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "BTreeMap<String, String>")]
    pub env: Option<BTreeMap<String, String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "BTreeMap<String, String>")]
    pub bins: Option<BTreeMap<String, String>>,
    /// Further prompts, making this a **batch**: one harness fanned over each
    /// prompt, sharing the cacheable `system`/model prefix. Combined order is
    /// `prompt`, then these, then `promptFiles` — the CLI's own order.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Vec<String>")]
    pub batch_prompts: Option<Vec<NonEmptyString>>,
    /// Files each holding one whole prompt, or `-` for stdin.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Vec<String>")]
    pub prompt_files: Option<Vec<String>>,
    /// Replace these selected harnesses' provider processes with oneharness's
    /// deterministic `MOCK_*`-scripted responder.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Vec<String>")]
    pub mock_harnesses: Option<Vec<String>>,
    /// Run against every supported harness.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "bool")]
    pub all: Option<bool>,
    /// Harness id(s) to drop from an all-harness run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Vec<String>")]
    pub exclude: Option<Vec<String>>,
    /// Read the system prompt from a file — the counterpart to `system` for a
    /// value too large to pass on the argv.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "String")]
    pub system_file: Option<String>,
    /// Directory the `session` store lives in.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "String")]
    pub session_dir: Option<String>,
    /// Open the out-of-band turn-control socket, so a separate `interrupt()`
    /// can abort the in-flight turn without killing this run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "bool")]
    pub control: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "OutputFormat")]
    pub output_format: Option<OutputFormat>,
    /// Mock/spy the selected harnesses' tool calls for this run only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "String")]
    pub mock_rules: Option<String>,
    /// Append one JSONL record per observed tool call to this file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "String")]
    pub spy_file: Option<String>,
    /// Constrain each harness's final answer to this JSON Schema file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "String")]
    pub schema: Option<String>,
    /// Max retries when a response fails schema validation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "u32")]
    pub schema_max_retries: Option<u32>,
    /// Max retries for a zero-work Codex server-overloaded refusal.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "u32")]
    pub server_overloaded_max_retries: Option<u32>,
    /// Write each harness's raw stdout/stderr under this directory.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "String")]
    pub output_dir: Option<String>,
    /// Silence the warning that the chosen mode may block on an approval prompt.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "bool")]
    pub permit_prompts: Option<bool>,
    /// Load configuration from exactly these files, in order, skipping
    /// user/project discovery; each later file overrides the earlier ones.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Vec<String>")]
    // llmlint: ignore[invalid_states_unrepresentable] shape fixed by manager ruling 1; the config/no_config pair predates this change, the SDKs refuse both together and the library gives no_config precedence.
    pub config: Option<Vec<String>>,
    /// Ignore every configuration file and `ONEHARNESS_*` override. `true`
    /// beside a non-empty `config` is refused before anything runs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "bool")]
    pub no_config: Option<bool>,
    /// Maximum harnesses (or, in a batch, prompts) to run concurrently.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "u32")]
    pub max_parallel: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "BatchStrategy")]
    pub batch_strategy: Option<BatchStrategy>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "RunMode")]
    pub run_mode: Option<RunMode>,
    /// Build and report each command without executing it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "bool")]
    pub print_command: Option<bool>,
    /// Treat a not-installed harness as a failure.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "bool")]
    pub require_available: Option<bool>,
    /// Do NOT record history for this run, overriding config or the
    /// `ONEHARNESS_HISTORY` environment override.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "bool")]
    pub no_history: Option<bool>,
    /// Extra arguments appended verbatim to each harness command, after `--`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Vec<String>")]
    pub passthrough: Option<Vec<String>>,
}

/// The session selector accepted by `OneHarness.history()` in the published Node
/// SDK.
///
/// A lookup must select a session, so the contract is a union of the only two
/// ways to do that: ask for the most recent one ([`HistoryLookupByLast`]), or
/// name one ([`HistoryLookupBySession`]). A lookup that selects nothing — `{}`,
/// `{"session": ""}`, or `{"last": false}` — matches neither variant and fails
/// at the boundary, so `history()` never has to re-check for one.
///
/// The variants overlap on purpose, and the order is load-bearing: an untagged
/// enum takes the first match, so `Last` coming first is what gives `last: true`
/// priority over a name. `{"session": "x", "last": true}` satisfies *both*
/// variants and resolves to `Last`, selecting the most recent session rather
/// than `x`; `{"session": "x", "last": false}` fails `Last` and resolves to
/// `Session`. That is exactly the reading of the `if (last) … else if (session)`
/// rule this replaces, now decided by the union rather than re-derived after
/// parsing. Zod resolves its generated union in the same order, so the Node SDK
/// agrees with Rust by construction.
///
/// Because `Last` ignores the name it carries, that name is a plain `String`:
/// `{"session": "", "last": true}` stays valid and still selects the most recent
/// session, as it always has. Only a name that actually selects has to be
/// non-empty.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
#[schemars(rename = "HistoryLookup")]
pub enum HistoryLookup {
    /// Select the most recent session.
    Last(HistoryLookupByLast),
    /// Select the session named by `session`.
    Session(HistoryLookupBySession),
}

/// A [`HistoryLookup`] that selects the most recent session.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[schemars(rename = "HistoryLookupByLast")]
pub struct HistoryLookupByLast {
    /// Select the most recent session. Only `true` selects, so this variant
    /// accepts no other value.
    pub last: LiteralTrue,
    /// A name may accompany `last: true` — it is what the caller would have
    /// looked up otherwise — but `last` takes priority, so it never selects.
    /// It is therefore unconstrained: an empty name is meaningless here rather
    /// than invalid, so `{"session": "", "last": true}` stays accepted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "String")]
    pub session: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "String")]
    pub project: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "bool")]
    pub all_projects: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "String")]
    pub history_dir: Option<String>,
    /// Which dates of the index to read: the last `days` UTC days, from a
    /// UTC date on, or all time (every dated segment plus the legacy index an
    /// older oneharness kept). Omitted, the last 7 UTC days.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window: Option<crate::domain::history_index::HistoryWindow>,
    /// Show every record in the session rather than the run summaries.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "bool")]
    pub all: Option<bool>,
    /// Load configuration from exactly these files, in order, skipping
    /// user/project discovery; each later file overrides the earlier ones.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Vec<String>")]
    // llmlint: ignore[invalid_states_unrepresentable] shape fixed by manager ruling 1; the config/no_config pair predates this change, the SDKs refuse both together and the library gives no_config precedence.
    pub config: Option<Vec<String>>,
    /// Ignore every configuration file and `ONEHARNESS_*` override. `true`
    /// beside a non-empty `config` is refused before anything runs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "bool")]
    pub no_config: Option<bool>,
}

/// A [`HistoryLookup`] that names its session.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[schemars(rename = "HistoryLookupBySession")]
pub struct HistoryLookupBySession {
    /// The oneharness-derived session name recorded by `run --history`. This is
    /// the name that selects, so it must be non-empty.
    pub session: NonEmptyString,
    /// Whether the most recent session was asked for instead. An ordinary
    /// `bool`, so a caller holding a `boolean` can pass it straight through.
    /// `true` here also satisfies [`HistoryLookup::Last`], which the union tries
    /// first — so a lookup that reaches this variant always meant the name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "bool")]
    pub last: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "String")]
    pub project: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "bool")]
    pub all_projects: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "String")]
    pub history_dir: Option<String>,
    /// Which dates of the index to read: the last `days` UTC days, from a
    /// UTC date on, or all time (every dated segment plus the legacy index an
    /// older oneharness kept). Omitted, the last 7 UTC days.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window: Option<crate::domain::history_index::HistoryWindow>,
    /// Show every record in the session rather than the run summaries.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "bool")]
    pub all: Option<bool>,
    /// Load configuration from exactly these files, in order, skipping
    /// user/project discovery; each later file overrides the earlier ones.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Vec<String>")]
    // llmlint: ignore[invalid_states_unrepresentable] shape fixed by manager ruling 1; the config/no_config pair predates this change, the SDKs refuse both together and the library gives no_config precedence.
    pub config: Option<Vec<String>>,
    /// Ignore every configuration file and `ONEHARNESS_*` override. `true`
    /// beside a non-empty `config` is refused before anything runs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "bool")]
    pub no_config: Option<bool>,
}

/// Options accepted by `OneHarness.historyList()` in the published Node SDK.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[schemars(rename = "HistoryListOptions")]
pub struct HistoryListOptions {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "String")]
    pub project: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "bool")]
    pub all_projects: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "String")]
    pub history_dir: Option<String>,
    /// Which dates of the index to read: the last `days` UTC days, from a
    /// UTC date on, or all time (every dated segment plus the legacy index an
    /// older oneharness kept). Omitted, the last 7 UTC days.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window: Option<crate::domain::history_index::HistoryWindow>,
    /// Narrow to one configured harness identity (`claude-code:work`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "String")]
    pub variant: Option<String>,
    /// Load configuration from exactly these files, in order, skipping
    /// user/project discovery; each later file overrides the earlier ones.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Vec<String>")]
    // llmlint: ignore[invalid_states_unrepresentable] shape fixed by manager ruling 1; the config/no_config pair predates this change, the SDKs refuse both together and the library gives no_config precedence.
    pub config: Option<Vec<String>>,
    /// Ignore every configuration file and `ONEHARNESS_*` override. `true`
    /// beside a non-empty `config` is refused before anything runs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "bool")]
    pub no_config: Option<bool>,
}

/// Options accepted by the language SDKs' continuous history iterators.
///
/// The CLI spells `labels` as repeated `--label key=value` arguments, while an
/// SDK can expose the validated map directly. Unknown fields remain a boundary
/// error, as they are for every other SDK input contract.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[schemars(rename = "HistoryWatchOptions")]
pub struct HistoryWatchOptions {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "HistoryId")]
    pub after: Option<HistoryId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "HistoryLabels")]
    pub labels: Option<HistoryLabels>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "String")]
    pub project: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "bool")]
    pub all_projects: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "String")]
    pub history_dir: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "bool")]
    pub events: Option<bool>,
    /// Where the watch starts when no `after` cursor is given: the beginning
    /// of the last `days` UTC days, of a UTC date, or of all time (the legacy
    /// index an older oneharness kept included). Omitted, the current UTC
    /// day. Refused beside `after`, the other answer to where a watch begins.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    // llmlint: ignore[invalid_states_unrepresentable] `after` is a released SDK field every existing watch caller passes, and manager ruling 1 fixes this window as a bare `HistoryWindow`; folding both into one union would break those callers, so the pair is refused at the boundary instead (capability `bind_refuse("after", …, "window")`, clap `conflicts_with`) and the library takes one `WatchStart`.
    pub window: Option<crate::domain::history_index::HistoryWindow>,
    /// Narrow to one configured harness identity (`claude-code:work`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "String")]
    pub variant: Option<String>,
    /// Follow one session: its id, or its name (the newest session so named
    /// whose labels match every label filter, including one still running, or
    /// the first to appear).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<crate::domain::history::HistorySessionSelector>,
    /// Load configuration from exactly these files, in order, skipping
    /// user/project discovery; each later file overrides the earlier ones.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Vec<String>")]
    // llmlint: ignore[invalid_states_unrepresentable] shape fixed by manager ruling 1; the config/no_config pair predates this change, the SDKs refuse both together and the library gives no_config precedence.
    pub config: Option<Vec<String>>,
    /// Ignore every configuration file and `ONEHARNESS_*` override. `true`
    /// beside a non-empty `config` is refused before anything runs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "bool")]
    pub no_config: Option<bool>,
}

/// Options accepted by the language SDKs' `detect()`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, Default)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[schemars(rename = "DetectOptions")]
pub struct DetectOptions {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Vec<String>")]
    pub harnesses: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "bool")]
    pub all: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Vec<String>")]
    pub exclude: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "BTreeMap<String, String>")]
    pub bins: Option<BTreeMap<String, String>>,
    /// Load configuration from exactly these files, in order, skipping
    /// user/project discovery; each later file overrides the earlier ones.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Vec<String>")]
    // llmlint: ignore[invalid_states_unrepresentable] shape fixed by manager ruling 1; the config/no_config pair predates this change, the SDKs refuse both together and the library gives no_config precedence.
    pub config: Option<Vec<String>>,
    /// Ignore every configuration file and `ONEHARNESS_*` override. `true`
    /// beside a non-empty `config` is refused before anything runs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "bool")]
    pub no_config: Option<bool>,
    /// Exit non-zero if any probed harness is not installed. The SDKs surface
    /// that as a thrown process error rather than a report a caller must
    /// re-check.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "bool")]
    pub require_available: Option<bool>,
}

/// Options accepted by the language SDKs' `config()`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, Default)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[schemars(rename = "ConfigOptions")]
pub struct ConfigOptions {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "String")]
    pub cwd: Option<String>,
    /// Load configuration from exactly these files, in order, skipping
    /// user/project discovery; each later file overrides the earlier ones.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Vec<String>")]
    // llmlint: ignore[invalid_states_unrepresentable] shape fixed by manager ruling 1; the config/no_config pair predates this change, the SDKs refuse both together and the library gives no_config precedence.
    pub config: Option<Vec<String>>,
    /// Ignore every configuration file and `ONEHARNESS_*` override. `true`
    /// beside a non-empty `config` is refused before anything runs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "bool")]
    pub no_config: Option<bool>,
}

/// Options accepted by the language SDKs' `sync()`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, Default)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[schemars(rename = "SyncOptions")]
pub struct SyncOptions {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "String")]
    pub cwd: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Vec<String>")]
    pub harnesses: Option<Vec<String>>,
    /// Report what would change and write nothing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "bool")]
    pub check: Option<bool>,
    /// Hold each harness's allow/deny list to exactly the configured one:
    /// stale and hand-added entries are removed, every other key is merged as
    /// usual, and the report names each added and removed entry.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "bool")]
    pub exact: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "bool")]
    pub global: Option<bool>,
    /// Load configuration from exactly these files, in order, skipping
    /// user/project discovery; each later file overrides the earlier ones.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Vec<String>")]
    // llmlint: ignore[invalid_states_unrepresentable] shape fixed by manager ruling 1; the config/no_config pair predates this change, the SDKs refuse both together and the library gives no_config precedence.
    pub config: Option<Vec<String>>,
    /// Ignore every configuration file and `ONEHARNESS_*` override. `true`
    /// beside a non-empty `config` is refused before anything runs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "bool")]
    pub no_config: Option<bool>,
}

/// Options accepted by the language SDKs' `init()`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, Default)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[schemars(rename = "InitOptions")]
pub struct InitOptions {
    /// Where to write the starter config. Absent means `oneharness.toml` in the
    /// working directory, exactly as the CLI's own default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "String")]
    pub path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "bool")]
    pub force: Option<bool>,
}

/// Options accepted by the language SDKs' `usage()`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, Default)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[schemars(rename = "UsageOptions")]
pub struct UsageOptions {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Vec<String>")]
    pub harnesses: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "bool")]
    pub all: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Vec<String>")]
    pub exclude: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "BTreeMap<String, String>")]
    pub bins: Option<BTreeMap<String, String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "String")]
    pub cwd: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "u64")]
    pub timeout_seconds: Option<u64>,
    /// Load configuration from exactly these files, in order, skipping
    /// user/project discovery; each later file overrides the earlier ones.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Vec<String>")]
    // llmlint: ignore[invalid_states_unrepresentable] shape fixed by manager ruling 1; the config/no_config pair predates this change, the SDKs refuse both together and the library gives no_config precedence.
    pub config: Option<Vec<String>>,
    /// Ignore every configuration file and `ONEHARNESS_*` override. `true`
    /// beside a non-empty `config` is refused before anything runs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "bool")]
    pub no_config: Option<bool>,
}

/// Options accepted by the language SDKs' `gate()` — the pre-tool gate an
/// installed hook invokes, driven directly so a consumer hosting its own hook
/// runner never has to shell out and parse.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[schemars(rename = "GateOptions")]
pub struct GateOptions {
    /// The harness whose hook protocol to speak.
    pub harness: NonEmptyString,
    /// The harness's pre-tool hook event, written to the gate's stdin.
    #[schemars(with = "String")]
    pub event: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "String")]
    pub deny_if_contains: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "String")]
    pub reason: Option<String>,
}

/// Options accepted by the language SDKs' `mock()`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[schemars(rename = "MockOptions")]
pub struct MockOptions {
    pub harness: NonEmptyString,
    /// The harness's pre-tool hook event, written to the responder's stdin.
    #[schemars(with = "String")]
    pub event: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "String")]
    pub rules: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "String")]
    pub spy_file: Option<String>,
}

/// Options accepted by the language SDKs' `interrupt()`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[schemars(rename = "InterruptOptions")]
pub struct InterruptOptions {
    /// The caller-owned session handle the target run was started with.
    pub session: NonEmptyString,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "String")]
    pub input: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "String")]
    pub session_dir: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "String")]
    pub cwd: Option<String>,
}

/// Options accepted by the language SDKs' `historyClear()`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, Default)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[schemars(rename = "HistoryClearOptions")]
pub struct HistoryClearOptions {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "String")]
    pub project: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "bool")]
    pub all_projects: Option<bool>,
    /// Actually delete. Absent or false reports what would be removed and
    /// removes nothing, so a caller can always look first.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "bool")]
    pub yes: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "String")]
    pub history_dir: Option<String>,
    /// Load configuration from exactly these files, in order, skipping
    /// user/project discovery; each later file overrides the earlier ones.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Vec<String>")]
    // llmlint: ignore[invalid_states_unrepresentable] shape fixed by manager ruling 1; the config/no_config pair predates this change, the SDKs refuse both together and the library gives no_config precedence.
    pub config: Option<Vec<String>>,
    /// Ignore every configuration file and `ONEHARNESS_*` override. `true`
    /// beside a non-empty `config` is refused before anything runs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "bool")]
    pub no_config: Option<bool>,
}

/// Options accepted by the language SDKs' `historyPointers()`: the pointer
/// file a run was given as `historyPointerFile`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[schemars(rename = "HistoryPointersOptions")]
pub struct HistoryPointersOptions {
    /// The pointer file to read. A file that does not exist reads as empty.
    pub file: NonEmptyString,
}

/// Options accepted by the language SDKs' `historyReindex()`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, Default)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[schemars(rename = "HistoryReindexOptions")]
pub struct HistoryReindexOptions {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "String")]
    pub history_dir: Option<String>,
    /// Load configuration from exactly these files, in order, skipping
    /// user/project discovery; each later file overrides the earlier ones.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Vec<String>")]
    // llmlint: ignore[invalid_states_unrepresentable] shape fixed by manager ruling 1; the config/no_config pair predates this change, the SDKs refuse both together and the library gives no_config precedence.
    pub config: Option<Vec<String>>,
    /// Ignore every configuration file and `ONEHARNESS_*` override. `true`
    /// beside a non-empty `config` is refused before anything runs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "bool")]
    pub no_config: Option<bool>,
}

/// Options accepted by the language SDKs' `historyMigrate()`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema, Default)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[schemars(rename = "HistoryMigrateOptions")]
pub struct HistoryMigrateOptions {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "String")]
    pub history_dir: Option<String>,
    /// Load configuration from exactly these files, in order, skipping
    /// user/project discovery; each later file overrides the earlier ones.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Vec<String>")]
    // llmlint: ignore[invalid_states_unrepresentable] shape fixed by manager ruling 1; the config/no_config pair predates this change, the SDKs refuse both together and the library gives no_config precedence.
    pub config: Option<Vec<String>>,
    /// Ignore every configuration file and `ONEHARNESS_*` override. `true`
    /// beside a non-empty `config` is refused before anything runs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "bool")]
    pub no_config: Option<bool>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(JsonSchema, Serialize)]
    struct OutputFixture {
        value: Option<String>,
    }

    #[test]
    fn output_schema_requires_serialized_option_keys_and_allows_null() {
        let schema = schema_for_serialize::<OutputFixture>();
        let value = schema.as_value();
        assert_eq!(value["required"], serde_json::json!(["value"]));
        assert_eq!(
            value["properties"]["value"]["type"],
            serde_json::json!(["string", "null"])
        );
    }

    fn non_empty(value: &str) -> NonEmptyString {
        NonEmptyString::try_from(value).expect("fixture is non-empty")
    }

    #[test]
    fn non_empty_string_rejects_an_empty_value_at_every_entry_point() {
        assert_eq!(
            NonEmptyString::try_from(String::new()),
            Err(EmptyStringError)
        );
        assert_eq!(NonEmptyString::try_from(""), Err(EmptyStringError));

        let error = serde_json::from_value::<NonEmptyString>(serde_json::json!(""))
            .expect_err("empty string should fail");
        assert!(error.to_string().contains("must be a non-empty string"));
    }

    #[test]
    fn non_empty_string_is_transparent_over_its_value() {
        let value = non_empty("node-session");
        assert_eq!(value.as_str(), "node-session");
        assert_eq!(value.to_string(), "node-session");
        assert_eq!(
            serde_json::to_value(&value).expect("serialize"),
            serde_json::json!("node-session")
        );
        assert_eq!(
            serde_json::from_value::<NonEmptyString>(serde_json::json!("node-session"))
                .expect("deserialize"),
            value
        );
        assert_eq!(value.into_string(), "node-session");
    }

    #[test]
    fn non_empty_string_schema_is_an_inline_string_with_a_minimum_length() {
        let schema = schemars::schema_for!(NonEmptyString);
        let value = schema.as_value();
        assert_eq!(value["type"], "string");
        assert_eq!(value["minLength"], 1);
    }

    #[test]
    fn literal_true_rejects_false_and_schemas_as_an_inline_const() {
        let error = serde_json::from_value::<LiteralTrue>(serde_json::json!(false))
            .expect_err("false is not `true`");
        assert!(error.to_string().contains("must be `true`"));

        assert_eq!(
            serde_json::from_value::<LiteralTrue>(serde_json::json!(true)).expect("true is valid"),
            LiteralTrue
        );
        assert_eq!(
            serde_json::to_value(LiteralTrue).expect("serialize"),
            serde_json::json!(true)
        );

        let schema = schemars::schema_for!(LiteralTrue);
        let value = schema.as_value();
        assert_eq!(value["type"], "boolean");
        assert_eq!(value["const"], true);
    }

    #[test]
    fn run_options_reject_an_empty_prompt() {
        let error = serde_json::from_value::<RunOptions>(serde_json::json!({ "prompt": "" }))
            .expect_err("empty prompt should fail");
        assert!(error.to_string().contains("must be a non-empty string"));
    }

    #[test]
    fn watch_options_validate_cursors_labels_and_unknown_fields() {
        let parsed = serde_json::from_value::<HistoryWatchOptions>(serde_json::json!({
            "after": "00000000-0000-7000-8000-000000000000",
            "labels": { "graph": "release" },
            "allProjects": true,
        }))
        .expect("valid watch options");
        assert_eq!(
            parsed
                .labels
                .as_ref()
                .expect("labels")
                .as_map()
                .get("graph")
                .map(String::as_str),
            Some("release")
        );
        assert!(serde_json::to_value(&parsed)
            .unwrap()
            .get("events")
            .is_none());
        let event_mode =
            serde_json::from_value::<HistoryWatchOptions>(serde_json::json!({ "events": true }))
                .expect("event watch mode is opt-in");
        assert_eq!(event_mode.events, Some(true));

        for invalid in [
            serde_json::json!({ "after": "not-a-cursor" }),
            serde_json::json!({ "labels": { "bad key": "release" } }),
            serde_json::json!({ "unknown": true }),
        ] {
            assert!(serde_json::from_value::<HistoryWatchOptions>(invalid).is_err());
        }
    }

    #[test]
    fn run_options_schema_requires_a_non_empty_prompt() {
        let schema = schemars::schema_for!(RunOptions);
        let value = schema.as_value();
        assert_eq!(value["required"], serde_json::json!(["prompt"]));
        assert_eq!(value["properties"]["prompt"]["type"], "string");
        assert_eq!(value["properties"]["prompt"]["minLength"], 1);
    }

    #[test]
    fn optional_fields_round_trip_and_are_omitted_when_absent() {
        let options = RunOptions {
            prompt: non_empty("inspect the repository"),
            harnesses: Some(vec!["codex".to_string()]),
            models: None,
            system: None,
            reasoning: None,
            resume: None,
            session: None,
            fork: None,
            mode: Some(PermissionMode::ReadOnly),
            cwd: None,
            timeout_seconds: Some(30),
            events: Some(true),
            history: None,
            history_name: None,
            history_dir: None,
            history_pointer_file: None,
            history_labels: None,
            env: None,
            bins: None,
            batch_prompts: None,
            prompt_files: None,
            mock_harnesses: None,
            all: None,
            exclude: None,
            system_file: None,
            session_dir: None,
            control: None,
            output_format: None,
            mock_rules: None,
            spy_file: None,
            schema: None,
            schema_max_retries: None,
            server_overloaded_max_retries: None,
            output_dir: None,
            permit_prompts: None,
            config: None,
            no_config: None,
            max_parallel: None,
            batch_strategy: None,
            run_mode: None,
            print_command: None,
            require_available: None,
            no_history: None,
            passthrough: None,
        };

        let value = serde_json::to_value(&options).expect("serialize SDK options");
        assert_eq!(value["timeoutSeconds"], 30);
        assert_eq!(value["mode"], "read-only");
        assert!(value.get("models").is_none());
        assert!(value.get("historyDir").is_none());
        assert_eq!(
            serde_json::from_value::<RunOptions>(value).expect("deserialize SDK options"),
            options
        );
    }

    #[test]
    fn unknown_fields_are_rejected() {
        let error = serde_json::from_value::<RunOptions>(serde_json::json!({
            "prompt": "hello",
            "harneses": ["codex"]
        }))
        .expect_err("misspelled option should fail");
        assert!(error.to_string().contains("unknown field `harneses`"));
    }

    #[test]
    fn history_lookup_optional_fields_round_trip_and_are_omitted_when_absent() {
        let lookup = HistoryLookup::Session(HistoryLookupBySession {
            session: non_empty("node-session"),
            last: None,
            project: None,
            all_projects: Some(true),
            history_dir: Some("/tmp/oneharness-history".to_string()),
            window: None,
            all: None,
            config: None,
            no_config: None,
        });

        let value = serde_json::to_value(&lookup).expect("serialize history lookup");
        assert_eq!(value["session"], "node-session");
        assert_eq!(value["allProjects"], true);
        assert_eq!(value["historyDir"], "/tmp/oneharness-history");
        assert!(value.get("last").is_none());
        assert!(value.get("project").is_none());
        assert_eq!(
            serde_json::from_value::<HistoryLookup>(value).expect("deserialize history lookup"),
            lookup
        );
    }

    #[test]
    fn history_lookup_by_last_round_trips_without_a_session() {
        let lookup = HistoryLookup::Last(HistoryLookupByLast {
            last: LiteralTrue,
            session: None,
            project: Some("oneharness".to_string()),
            all_projects: None,
            history_dir: None,
            window: None,
            all: None,
            config: None,
            no_config: None,
        });

        let value = serde_json::to_value(&lookup).expect("serialize history lookup");
        assert_eq!(value["last"], true);
        assert_eq!(value["project"], "oneharness");
        assert!(value.get("session").is_none());
        assert_eq!(
            serde_json::from_value::<HistoryLookup>(value).expect("deserialize history lookup"),
            lookup
        );
    }

    #[test]
    fn history_lookup_requires_a_selector() {
        for selectorless in [
            serde_json::json!({}),
            serde_json::json!({ "session": "" }),
            serde_json::json!({ "last": false }),
            serde_json::json!({ "session": "", "last": false }),
            serde_json::json!({ "project": "oneharness" }),
        ] {
            serde_json::from_value::<HistoryLookup>(selectorless.clone())
                .expect_err(&format!("{selectorless} selects no session"));
        }
    }

    #[test]
    fn history_lookup_accepts_every_way_to_select_a_session() {
        assert_eq!(
            serde_json::from_value::<HistoryLookup>(serde_json::json!({ "session": "x" }))
                .expect("a named session selects"),
            HistoryLookup::Session(HistoryLookupBySession {
                session: non_empty("x"),
                last: None,
                project: None,
                all_projects: None,
                history_dir: None,
                window: None,
                all: None,
                config: None,
                no_config: None,
            })
        );
        assert_eq!(
            serde_json::from_value::<HistoryLookup>(serde_json::json!({ "last": true }))
                .expect("the last session selects"),
            HistoryLookup::Last(HistoryLookupByLast {
                last: LiteralTrue,
                session: None,
                project: None,
                all_projects: None,
                history_dir: None,
                window: None,
                all: None,
                config: None,
                no_config: None,
            })
        );
    }

    #[test]
    fn history_lookup_gives_last_priority_over_a_name() {
        // `{session, last: true}` satisfies both variants. `Last` is declared
        // first, so it wins and the name rides along unselected — the priority
        // the SDK has always given `last`.
        assert_eq!(
            serde_json::from_value::<HistoryLookup>(
                serde_json::json!({ "session": "x", "last": true })
            )
            .expect("last selects even when a name is present"),
            HistoryLookup::Last(HistoryLookupByLast {
                last: LiteralTrue,
                session: Some("x".to_string()),
                project: None,
                all_projects: None,
                history_dir: None,
                window: None,
                all: None,
                config: None,
                no_config: None,
            })
        );
        // Dropping `last` to `false` fails the `Last` variant, so the same
        // caller asks for the named session instead.
        assert_eq!(
            serde_json::from_value::<HistoryLookup>(
                serde_json::json!({ "session": "x", "last": false })
            )
            .expect("an explicit `last: false` selects the name"),
            HistoryLookup::Session(HistoryLookupBySession {
                session: non_empty("x"),
                last: Some(false),
                project: None,
                all_projects: None,
                history_dir: None,
                window: None,
                all: None,
                config: None,
                no_config: None,
            })
        );
    }

    #[test]
    fn history_lookup_ignores_an_empty_name_that_does_not_select() {
        // `last: true` selects, so the name beside it is inert — an empty one is
        // meaningless rather than invalid, and stays accepted.
        assert_eq!(
            serde_json::from_value::<HistoryLookup>(
                serde_json::json!({ "session": "", "last": true })
            )
            .expect("an unselected name is unconstrained"),
            HistoryLookup::Last(HistoryLookupByLast {
                last: LiteralTrue,
                session: Some(String::new()),
                project: None,
                all_projects: None,
                history_dir: None,
                window: None,
                all: None,
                config: None,
                no_config: None,
            })
        );
    }

    #[test]
    fn history_lookup_rejects_unknown_fields() {
        let error = serde_json::from_value::<HistoryLookup>(serde_json::json!({
            "session": "node-session",
            "sesion": "node-session"
        }))
        .expect_err("misspelled selector should fail");
        assert!(
            error
                .to_string()
                .contains("did not match any variant of untagged enum HistoryLookup"),
            "unexpected error: {error}"
        );
    }

    /// Resolve one `anyOf` member of a schema document, following the local
    /// `$ref` schemars emits for a named variant.
    fn union_variant(document: &Value, index: usize) -> Value {
        let variant = document["anyOf"]
            .as_array()
            .expect("a union of variants")
            .get(index)
            .unwrap_or_else(|| panic!("variant {index} is missing"))
            .clone();
        let Some(reference) = variant.get("$ref").and_then(Value::as_str) else {
            return variant;
        };
        let name = reference
            .strip_prefix("#/$defs/")
            .unwrap_or_else(|| panic!("unexpected non-local reference {reference}"));
        document["$defs"][name].clone()
    }

    #[test]
    fn history_lookup_schema_is_a_union_that_requires_a_selector() {
        let schema = schemars::schema_for!(HistoryLookup);
        let document = schema.as_value();
        assert_eq!(
            document["anyOf"].as_array().map(Vec::len),
            Some(2),
            "a lookup selects a session in exactly two ways"
        );

        // `Last` is first, so a consumer matching the union in order gives
        // `last: true` the priority it has always had over a name. The name it
        // carries never selects, so it stays unconstrained.
        let by_last = union_variant(document, 0);
        assert_eq!(by_last["additionalProperties"], serde_json::json!(false));
        assert_eq!(by_last["required"], serde_json::json!(["last"]));
        assert_eq!(by_last["properties"]["last"]["const"], true);
        assert_eq!(by_last["properties"]["session"]["type"], "string");
        assert!(by_last["properties"]["session"].get("minLength").is_none());

        // Only the name that selects is constrained, and `last` stays an
        // ordinary boolean so a caller holding one can pass it through.
        let by_session = union_variant(document, 1);
        assert_eq!(by_session["additionalProperties"], serde_json::json!(false));
        assert_eq!(by_session["required"], serde_json::json!(["session"]));
        assert_eq!(by_session["properties"]["session"]["type"], "string");
        assert_eq!(by_session["properties"]["session"]["minLength"], 1);
        assert_eq!(by_session["properties"]["last"]["type"], "boolean");

        // Both ways to select carry the same non-selecting fields.
        for variant in [&by_session, &by_last] {
            for field in ["project", "allProjects", "historyDir"] {
                assert!(
                    variant["properties"].get(field).is_some(),
                    "{field} should be accepted by every variant"
                );
            }
        }
    }

    #[test]
    fn history_list_optional_fields_round_trip_and_are_omitted_when_absent() {
        let options = HistoryListOptions {
            project: None,
            all_projects: Some(true),
            history_dir: Some("/tmp/oneharness-history".to_string()),
            window: None,
            variant: None,
            config: None,
            no_config: None,
        };

        let value = serde_json::to_value(&options).expect("serialize history list options");
        assert_eq!(value["allProjects"], true);
        assert_eq!(value["historyDir"], "/tmp/oneharness-history");
        assert!(value.get("project").is_none());
        assert_eq!(
            serde_json::from_value::<HistoryListOptions>(value)
                .expect("deserialize history list options"),
            options
        );
    }

    #[test]
    fn history_list_options_default_to_every_field_absent() {
        assert_eq!(
            serde_json::from_value::<HistoryListOptions>(serde_json::json!({}))
                .expect("empty options are valid"),
            HistoryListOptions {
                project: None,
                all_projects: None,
                history_dir: None,
                window: None,
                variant: None,
                config: None,
                no_config: None,
            }
        );
    }

    #[test]
    fn history_list_options_reject_unknown_fields() {
        let error = serde_json::from_value::<HistoryListOptions>(serde_json::json!({
            "allProject": true
        }))
        .expect_err("misspelled option should fail");
        assert!(error.to_string().contains("unknown field `allProject`"));
    }
}
