//! The configuration document, the three layers over it, and what they resolve to.
//!
//! Precedence is file, then environment, then command-line flags, lowest to highest,
//! and every setting is reachable at all three — including every field of every named
//! source. That is one mechanism rather than three: each layer is flattened to the
//! same list of leaf settings (see [`layer`]), the stack is merged once, and the
//! result is deserialized into [`Config`]. Nothing per-verb decides precedence, so
//! nothing per-verb can get it wrong.
//!
//! Reading is [`discovery`]'s and nothing else's; everything else here is a function
//! of its arguments.
//!
//! One thing a leaf setting carries besides its value is load-bearing past this layer:
//! **a relative filesystem path a configuration document supplies is resolved against the
//! directory holding that document**, while one the environment or a flag supplies keeps
//! resolving against the process working directory. [`relative`] is where that happens and
//! why it cannot happen in the plugin that reads the path; `README.md`, under "Relative
//! paths in a configuration document", states it for a user.

mod discovery;
mod effective;
mod environment_layer;
mod error;
mod layer;
mod relative;
mod routes;

use std::collections::{BTreeMap, BTreeSet};
use std::num::NonZeroU32;
use std::path::Path;

use onetaskgraph_plugin_api::SourceName;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::secrets::Secrets;
use crate::subprocess::DocumentDir;
use crate::{Environment, PluginKind, plugin_kinds, registry::omitted_feature};

pub use discovery::{
    Document, PROJECT_DOCUMENT_NAME, SECRETS_RELATIVE_PATH, USER_DOCUMENT_RELATIVE_PATH, documents,
    read_optional, readable_documents, secrets_path, user_document_path,
};
pub use effective::EffectiveConfig;
pub use environment_layer::{ENVIRONMENT_PREFIX, HOST_VARIABLE, variable_for};
pub use error::ConfigError;
pub use layer::{Layer, Merged, Origin, Setting, SettingPath, merge, unflatten, value_from_text};
pub(crate) use relative::rebased;
pub use relative::resolve_document_relative_paths;
pub use routes::{Placement, RepositoryPattern, Route, RoutePurpose, Routes, SourceRoute};

/// The variable that moves the credentials file somewhere else.
pub const SECRETS_FILE_VARIABLE: &str = "ONETASKGRAPH_SECRETS_FILE";

/// How many items a page holds when nothing sets `page_size`.
pub const DEFAULT_PAGE_SIZE: NonZeroU32 = NonZeroU32::new(50).expect("50 is not zero");

/// Whether a command may prompt when nothing sets `interactive`.
pub const DEFAULT_INTERACTIVE: bool = true;

/// How output is rendered.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum OutputFormat {
    /// For a person reading a terminal.
    #[default]
    Text,
    /// For a program.
    Json,
}

/// One named source, as a document configures it.
///
/// Built by [`Config::from_document`], never deserialized directly: `plugin` is a
/// [`PluginKind`] rather than the string the document spelled, so a source naming a
/// plugin this build does not have cannot be represented here at all.
#[derive(Debug, Clone, PartialEq)]
pub struct SourceConfig {
    plugin: PluginKind,
    config: Value,
    document_dir: Option<DocumentDir>,
    routes: Vec<Route>,
    visibility: SourceVisibility,
    cache: SourceCacheConfig,
    public_alias: Option<onetaskgraph_plugin_api::Designator>,
    task_projection: Option<TaskProjectionConfig>,
    create: CreateSettings,
}

/// How a copy out of one source keeps a create safe to retry: its two settings, each in whole
/// seconds, read off the source's own block beside `cache` and `routes`.
///
/// `create_lease_seconds` is how long the intent a copy records on the item it copies, at
/// `onetaskgraph.creating`, stays its own before another attempt may take it over — read off
/// the copy's **source**. `create_quiescence_seconds` is how long after an intent was first
/// taken a retry waits for an earlier attempt's create to become listed before creating again
/// — read off the copy's **destination**, and only where that destination's lookup by create
/// key can lag a create. Both default to 120 and are left out of a configuration that does not
/// set them.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CreateSettings {
    /// Seconds an intent to create stays the holder's own; 120 when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    // llmlint: ignore[invalid_states_unrepresentable] Schema DTO, as every plugin configuration here is: the configuration schema both SDKs are generated from describes these as whole numbers, and `CreateSettings::checked` refuses zero and anything past `MAX_SECONDS` when the configuration loads, naming the key, before any engine is built — `the_create_settings_are_shown_by_their_layer_and_refused_out_of_range` drives it.
    pub create_lease_seconds: Option<u64>,
    /// Seconds after an intent was first taken that a retry waits for the earlier create to be
    /// listed; 120 when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    // llmlint: ignore[invalid_states_unrepresentable] As `create_lease_seconds` above.
    pub create_quiescence_seconds: Option<u64>,
}

impl CreateSettings {
    /// The default of each setting, in seconds.
    pub const DEFAULT_SECONDS: u64 = 120;

    /// The longest either setting may be, in seconds: a day. A lease or a wait longer than that
    /// is a mistake rather than a policy, and refusing it at load keeps every time an intent is
    /// spelled with well inside what a timestamp can hold.
    pub const MAX_SECONDS: u64 = 86_400;

    /// Refuse a setting of zero or past [`Self::MAX_SECONDS`], naming the source and the key.
    fn checked(self, source: &str) -> Result<Self, ConfigError> {
        for (key, value) in [
            ("create_lease_seconds", self.create_lease_seconds),
            ("create_quiescence_seconds", self.create_quiescence_seconds),
        ] {
            if let Some(value) = value
                && !(1..=Self::MAX_SECONDS).contains(&value)
            {
                return Err(ConfigError::setting(
                    format!("sources.{source}.{key}"),
                    format!(
                        "{value} is not a whole number of seconds from 1 to {}",
                        Self::MAX_SECONDS
                    ),
                    format!(
                        "set {key} to between 1 and {} seconds, or remove it for the default of {}.",
                        Self::MAX_SECONDS,
                        Self::DEFAULT_SECONDS
                    ),
                ));
            }
        }
        Ok(self)
    }

    /// How long an intent stays its holder's own.
    #[must_use]
    pub fn lease(self) -> chrono::TimeDelta {
        seconds(self.create_lease_seconds)
    }

    /// How long a retry waits for an earlier create to be listed.
    #[must_use]
    pub fn quiescence(self) -> chrono::TimeDelta {
        seconds(self.create_quiescence_seconds)
    }
}

/// A setting as a duration: [`CreateSettings::checked`] bounds it at load, and a value past the
/// bound is read as the bound rather than trusted.
fn seconds(setting: Option<u64>) -> chrono::TimeDelta {
    let bounded = setting
        .unwrap_or(CreateSettings::DEFAULT_SECONDS)
        .min(CreateSettings::MAX_SECONDS);
    chrono::TimeDelta::try_seconds(i64::try_from(bounded).unwrap_or(0)).unwrap_or_default()
}

impl SourceConfig {
    /// How a copy out of, or into, this source keeps a create safe to retry.
    #[must_use]
    pub fn create(&self) -> CreateSettings {
        self.create
    }

    /// Where, and on which host's authority, human task projections into this source keep
    /// their durable intents — `None` for a source that receives none.
    #[must_use]
    pub fn task_projection(&self) -> Option<&TaskProjectionConfig> {
        self.task_projection.as_ref()
    }

    /// The designator `public_alias` declares, `alias/<public_alias>`, for a source whose
    /// backend has no opaque identifier of its own: every host's configuration of the same
    /// tracker declares the same alias, so an opaque edge written under one host's name for
    /// it resolves under another's. `None` when the key is left off.
    #[must_use]
    pub fn public_alias(&self) -> Option<&onetaskgraph_plugin_api::Designator> {
        self.public_alias.as_ref()
    }

    /// Whether, and where, the store keeps a local cache of this source's tasks.
    #[must_use]
    pub fn cache(&self) -> &SourceCacheConfig {
        &self.cache
    }

    /// Who this configuration declares can read the source.
    ///
    /// A declaration, not the source's reality: a source declared private receives an item
    /// that may only be written somewhere private only while its backend, read at that
    /// write, agrees. `unknown`, which a source leaving the key off declares, is never
    /// private.
    #[must_use]
    pub fn visibility(&self) -> SourceVisibility {
        self.visibility
    }

    /// Where an item written to this source goes instead, in the order the entries are
    /// tried. Empty for a source that routes nothing, which is every source a
    /// configuration leaves `routes` off.
    #[must_use]
    pub fn routes(&self) -> &[Route] {
        &self.routes
    }

    /// The plugin kind that builds this source.
    #[must_use]
    pub fn plugin(&self) -> PluginKind {
        self.plugin
    }

    /// The plugin's own block.
    ///
    /// Opaque here on purpose — a plugin's fields are the plugin's, and typing them in
    /// the engine would put every plugin's shape in the engine. What holds instead is
    /// that [`Config::from_document`] checks each block against the schema its own
    /// plugin declares, and this field is not public, so no `SourceConfig` anybody can
    /// reach carries a block that plugin would refuse.
    #[must_use]
    pub fn config(&self) -> &Value {
        &self.config
    }

    /// The absolute directory holding the configuration document that supplied the
    /// block this source hands a child process, or `None` when no one document did.
    ///
    /// Only a `subprocess` source ever carries one: its `settings:` block is opaque here,
    /// so nothing inside it is rebased in this process, and this is what the child is
    /// told instead — `document_dir` in `docs/plugin-protocol.md` §3 — so the hosted
    /// plugin can resolve its own declared paths exactly as the in-process rule does.
    /// It is an engine-owned fact about where the block came from, which is why it is
    /// read from the merge's origins rather than being a setting anybody can write.
    #[must_use]
    pub fn document_dir(&self) -> Option<&Path> {
        self.document_dir.as_ref().map(DocumentDir::as_path)
    }
}

/// One named source as a document spells it, before its plugin name is checked.
///
/// `plugin` is optional here because a layer may refine a source another layer declares —
/// its `routes`, one `config` field — and the merge may hold no layer that declares it. Such
/// an entry is a refinement with nothing to refine: [`Config::from_document`] leaves the
/// source unconfigured and records its name, so the rest of the configuration still loads
/// and only a command naming that source is refused.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct SourceShape {
    #[serde(default)]
    plugin: Option<String>,
    #[serde(default = "empty_block")]
    config: Value,
    /// Read by [`routes::parse`] rather than by serde, so a refusal names the entry.
    #[serde(default)]
    routes: Value,
    #[serde(default)]
    visibility: SourceVisibility,
    #[serde(default)]
    cache: SourceCacheConfig,
    /// Read by [`public_alias`] rather than by serde, so a refusal names the pattern and never
    /// repeats the value.
    #[serde(default)]
    public_alias: Option<Value>,
    #[serde(default)]
    task_projection: Option<TaskProjectionConfig>,
    #[serde(default)]
    create_lease_seconds: Option<u64>,
    #[serde(default)]
    create_quiescence_seconds: Option<u64>,
}

/// A source's `task_projection`: the host-local authority over human task projections
/// written into it (see `docs/task-projection.md`).
///
/// One host is authoritative for a source's projections, and its intents live in one absolute
/// directory there; a process on another host is refused rather than arbitrating. `project` is
/// the existing project a session's or host's item lands under when its request names no plan
/// project.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TaskProjectionConfig {
    /// The authoritative host, compared with `ONETASKGRAPH_HOST` or the machine's hostname.
    pub host: crate::task_projection::HostIdentity,
    /// The absolute directory holding the durable intents.
    #[schemars(with = "String")]
    pub directory: AbsoluteDirectory,
    /// The existing project of this source a planless item is filed under.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<crate::task_projection::ProjectionProjectId>,
}

/// The designator a source's `public_alias` declares, refused when it is not a string matching
/// `^[A-Za-z0-9_-]{16,64}$`.
fn public_alias(
    key: &str,
    alias: Option<&Value>,
) -> Result<Option<onetaskgraph_plugin_api::Designator>, ConfigError> {
    let Some(alias) = alias else {
        return Ok(None);
    };
    let refused = |why: String| {
        ConfigError::setting(
            format!("{key}.public_alias"),
            why,
            "give it 16 to 64 of A-Z, a-z, 0-9, `_` and `-`, the same on every host that \
             configures this tracker, or remove it.",
        )
    };
    let Some(alias) = alias.as_str() else {
        return Err(refused("a public_alias is a string".to_owned()));
    };
    onetaskgraph_plugin_api::Designator::of_alias(alias)
        .map(Some)
        .map_err(refused)
}

/// Whether the store keeps a local cache of one source's tasks, and where: `cache` beside
/// `plugin` and `config` in its entry.
///
/// The cache is per host and shared by every process configuring the same remote — the
/// command line, both SDKs and a Rust caller of the engine — and it is keyed on the change
/// token a source gives each task, so it is served only at the version a caller holds or a
/// later one. `README.md`, under "A local cache of a remote source", states its rules.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SourceCacheConfig {
    /// Whether the cache is on. Off unless set.
    #[serde(default)]
    pub enabled: bool,
    /// The directory the cache lives in. By default
    /// `${XDG_CACHE_HOME:-$HOME/.cache}/onetaskgraph/<digest of the source's remote>`; a
    /// relative path a configuration document supplies is measured from that document's
    /// directory. Ignored by a source with no remote.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Option<String>")]
    pub dir: Option<std::path::PathBuf>,
}

/// An absolute directory, refused at load when a configuration gives a relative one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "std::path::PathBuf", into = "std::path::PathBuf")]
pub struct AbsoluteDirectory(std::path::PathBuf);

impl AbsoluteDirectory {
    /// The directory.
    #[must_use]
    pub fn as_path(&self) -> &Path {
        &self.0
    }
}

impl TryFrom<std::path::PathBuf> for AbsoluteDirectory {
    type Error = String;

    fn try_from(path: std::path::PathBuf) -> Result<Self, Self::Error> {
        if path.is_absolute() {
            Ok(Self(path))
        } else {
            Err(format!("{} is not an absolute directory", path.display()))
        }
    }
}

impl From<AbsoluteDirectory> for std::path::PathBuf {
    fn from(directory: AbsoluteDirectory) -> Self {
        directory.0
    }
}

/// Who a configuration declares can read one source: `visibility` beside `plugin` and
/// `config` in its entry.
///
/// Only `private` is private, and only once the source's backend agrees at the write it is
/// about; `unknown`, the default, is what a source that says nothing declares.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum SourceVisibility {
    /// Anybody can read what is written there.
    Public,
    /// Only those its backend admits can — which every write checks against the backend.
    Private,
    /// Nobody has said, which is never private.
    #[default]
    Unknown,
}

impl SourceVisibility {
    /// The value as the configuration spells it.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Public => "public",
            Self::Private => "private",
            Self::Unknown => "unknown",
        }
    }
}

impl std::fmt::Display for SourceVisibility {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// The commands a store asks about the public boundary: `write_policy` at the document's
/// root.
///
/// Both are generic command lines rather than a named tool, so the policy behind them stays
/// the caller's. `visibility_command` is handed `{"repository": "<host/owner/name>"}` on its
/// standard input and answers `{"visibility": "public" | "private" | "unknown"}`;
/// `check_command` is handed a whole [`PublicWriteInput`](crate::boundary::PublicWriteInput)
/// and passes a write only by exiting 0 with `{"verdict": "pass"}`. Either may be left out:
/// without a visibility command every repository is `unknown`, which is private, and without
/// a check command no write to a public destination passes.
///
/// `sanitize_command` is handed a whole
/// [`PublicSanitizeInput`](crate::boundary::PublicSanitizeInput) and answers the write's
/// rewritable entries with every declared replacement applied, and its verdict; with it, every
/// screened write is put to it **in place of** `check_command`, and what it answers is what is
/// written. `terms_command` answers a scope's resolved terms, which only `sources sanitize`
/// asks for. `replacements` is the caller's replacement layer, sent with every sanitisation
/// and above every declaration level the policy reads, a later entry winning; it is refused
/// at load without a `sanitize_command`, because nothing would apply it.
///
/// `sanitize_stored_on_write` opts a store into the read-first guard: a narrow write to a task
/// is screened with the task and every comment on it as stored, and what that rewrites is
/// written back first. Off, a narrow write screens only what it writes and a stored term is left
/// for `sources sanitize`. It is refused at load without a `sanitize_command` for the same
/// reason as `replacements`; a linking caller's own policy opts in through
/// `Engine::with_sanitize_stored_on_write`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WritePolicyConfig {
    /// The command that answers a repository's visibility.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub visibility_command: Option<PolicyCommand>,
    /// The command that decides whether a write may reach a public destination.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub check_command: Option<PolicyCommand>,
    /// The command that rewrites a write's declared terms and decides the result.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sanitize_command: Option<PolicyCommand>,
    /// The command that answers a scope's resolved terms.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terms_command: Option<PolicyCommand>,
    /// The caller's replacement layer: `{term, with}` or `{term, reject: true}`, in order.
    #[serde(
        default,
        skip_serializing_if = "Vec::is_empty",
        deserialize_with = "replacements"
    )]
    #[schemars(!skip_serializing_if)]
    pub replacements: Vec<privateterms::Replacement>,
    /// Whether a narrow write to a task first rewrites the terms already stored on it.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    #[schemars(!skip_serializing_if)]
    pub sanitize_stored_on_write: bool,
}

/// `write_policy.replacements` as a document, an environment variable or `--set` spells it: a
/// list, or a mapping keyed by each entry's index — what
/// `ONETASKGRAPH_WRITE_POLICY__REPLACEMENTS__0__TERM` produces, since neither the environment
/// nor `--set` can spell a list of objects. A term or a replacement the environment read as a
/// number or a list is read back as the text it was given.
fn replacements<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Vec<privateterms::Replacement>, D::Error> {
    use serde::de::Error as _;
    fn text(value: Value) -> Value {
        match value {
            Value::Number(number) => Value::String(number.to_string()),
            Value::Array(parts) => Value::String(
                parts
                    .into_iter()
                    .map(|part| match text(part) {
                        Value::String(part) => part,
                        other => other.to_string(),
                    })
                    .collect::<Vec<_>>()
                    .join(","),
            ),
            other => other,
        }
    }
    fn entry(value: Value) -> Value {
        match value {
            Value::Object(fields) => Value::Object(
                fields
                    .into_iter()
                    .map(|(key, value)| {
                        let value = if key == "reject" { value } else { text(value) };
                        (key, value)
                    })
                    .collect(),
            ),
            other => other,
        }
    }
    let entries: Vec<Value> = match Value::deserialize(deserializer)? {
        Value::Null => Vec::new(),
        Value::Array(entries) => entries,
        Value::Object(entries) => {
            let mut indexed = Vec::new();
            for (key, entry) in entries {
                let index = key.parse::<usize>().map_err(|_| {
                    D::Error::custom(format!(
                        "a replacement is addressed by its index in the list, and {key:?} is not one"
                    ))
                })?;
                indexed.push((index, entry));
            }
            indexed.sort_by_key(|(index, _)| *index);
            for (position, (index, _)) in indexed.iter().enumerate() {
                if *index != position {
                    return Err(D::Error::custom(format!(
                        "the replacements are numbered from 0 with no gaps, and entry {position} \
                         is missing"
                    )));
                }
            }
            indexed.into_iter().map(|(_, entry)| entry).collect()
        }
        _ => {
            return Err(D::Error::custom(
                "replacements is a list of `{term, with}` or `{term, reject: true}` entries",
            ));
        }
    };
    entries
        .into_iter()
        .map(|value| serde_json::from_value(entry(value)).map_err(D::Error::custom))
        .collect()
}

/// One command line: the program, then its arguments, never empty.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(try_from = "Vec<String>", into = "Vec<String>")]
pub struct PolicyCommand(Vec<String>);

impl PolicyCommand {
    /// The program to run.
    #[must_use]
    pub fn program(&self) -> &str {
        &self.0[0]
    }

    /// Its arguments, in order.
    #[must_use]
    pub fn arguments(&self) -> &[String] {
        &self.0[1..]
    }
}

impl TryFrom<Vec<String>> for PolicyCommand {
    type Error = String;

    fn try_from(words: Vec<String>) -> Result<Self, Self::Error> {
        match words.first() {
            Some(program) if !program.trim().is_empty() => Ok(Self(words)),
            _ => Err("a command is a list whose first entry names the program to run".to_owned()),
        }
    }
}

impl From<PolicyCommand> for Vec<String> {
    fn from(command: PolicyCommand) -> Self {
        command.0
    }
}

/// A plugin block nobody wrote, which is different from one nobody may write.
fn empty_block() -> Value {
    Value::Object(Map::new())
}

/// A validated configuration.
///
/// Built by [`Config::from_document`], never deserialized directly: a source's name
/// has to be checked against the pattern the environment mapping depends on, and
/// `default_sources` has to name sources that exist, and both are worth a message
/// that says which key is wrong rather than serde's own.
///
/// Its fields are read through the methods below rather than reached into, because
/// "validated" is a claim about the whole value: a public `sources` would let a caller
/// hold a `Config` whose `default_sources` names something it does not contain, and a
/// public `SourceConfig` would let one hold a block its own plugin refuses. Neither
/// state is representable while the only way in is [`Config::from_document`].
#[derive(Debug, Clone, PartialEq)]
pub struct Config {
    default_sources: Option<Vec<SourceName>>,
    page_size: NonZeroU32,
    output: OutputFormat,
    interactive: bool,
    sources: BTreeMap<SourceName, SourceConfig>,
    /// Sources some layer refines but no layer gives a `plugin`, which are not configured.
    incomplete: BTreeSet<SourceName>,
    write_policy: Option<WritePolicyConfig>,
    /// The directory every source's cache defaults to living under, read from the
    /// environment the configuration was loaded in: `$XDG_CACHE_HOME`, or `$HOME/.cache`.
    cache_home: Option<std::path::PathBuf>,
}

/// The document's own shape, before the checks serde cannot make.
#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct DocumentShape {
    #[serde(deserialize_with = "one_or_many")]
    default_sources: Option<Vec<String>>,
    page_size: NonZeroU32,
    output: OutputFormat,
    interactive: bool,
    sources: BTreeMap<String, SourceShape>,
    write_policy: Option<WritePolicyConfig>,
}

impl Default for DocumentShape {
    fn default() -> Self {
        Self {
            default_sources: None,
            page_size: DEFAULT_PAGE_SIZE,
            output: OutputFormat::default(),
            interactive: DEFAULT_INTERACTIVE,
            sources: BTreeMap::new(),
            write_policy: None,
        }
    }
}

/// Accept one name where a list is expected.
///
/// The environment layer reads a comma-separated value as a list, so
/// `ONETASKGRAPH_DEFAULT_SOURCES=work,notes` is one; a single name has no comma to
/// split on, and refusing `ONETASKGRAPH_DEFAULT_SOURCES=work` would make the layer
/// hold for two sources and not for one.
fn one_or_many<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<Vec<String>>, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum OneOrMany {
        One(String),
        Many(Vec<String>),
    }

    Ok(match Option::<OneOrMany>::deserialize(deserializer)? {
        None => None,
        Some(OneOrMany::One(name)) => Some(vec![name]),
        Some(OneOrMany::Many(names)) => Some(names),
    })
}

impl Config {
    /// Read one merged document into a validated configuration.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigError::Setting`] naming the offending key for an unknown
    /// field, a value of the wrong shape, a `plugin:` this build does not have, a
    /// source name that does not match
    /// [`SOURCE_NAME_PATTERN`](onetaskgraph_plugin_api::SOURCE_NAME_PATTERN), a
    /// `default_sources` entry naming a source nothing configures, or a `config:`
    /// block the source's own plugin refuses.
    ///
    /// A source entry no layer gives a `plugin` is not refused: it is left out of
    /// [`sources`](Self::sources) and named by [`incomplete`](Self::incomplete), and a
    /// `default_sources` entry or a route naming it is kept, to be refused by the command
    /// that would read or follow it.
    pub fn from_document(document: Value) -> Result<Self, ConfigError> {
        let shape: DocumentShape = serde_path_to_error::deserialize(document).map_err(|error| {
            let key = error.path().to_string();
            let key = if key.is_empty() || key == "." {
                "the document's root".to_owned()
            } else {
                key
            };
            ConfigError::setting(
                key,
                error.into_inner().to_string(),
                "correct that setting, or remove it — `onetaskgraph config show` lists \
                     every setting this build reads and the layer each came from.",
            )
        })?;

        let mut sources = BTreeMap::new();
        let mut incomplete = BTreeSet::new();
        for (name, source) in shape.sources {
            let key = format!("sources.{name}");
            let plugin = source
                .plugin
                .as_deref()
                .map(|plugin| {
                    PluginKind::parse(plugin).ok_or_else(|| plugin_not_in_this_build(&key, plugin))
                })
                .transpose()?;
            let name = SourceName::new(name).map_err(|error| {
                ConfigError::setting(
                    &key,
                    error.to_string(),
                    "rename the source to lower-case letters, digits and hyphens — an \
                     underscore would make the ONETASKGRAPH_SOURCES__<NAME>__ mapping \
                     ambiguous.",
                )
            })?;
            let Some(plugin) = plugin else {
                incomplete.insert(name);
                continue;
            };
            let routes = routes::parse(&name, &source.routes)?;
            let public_alias = public_alias(&key, source.public_alias.as_ref())?;
            let create = CreateSettings {
                create_lease_seconds: source.create_lease_seconds,
                create_quiescence_seconds: source.create_quiescence_seconds,
            }
            .checked(name.as_str())?;
            sources.insert(
                name,
                SourceConfig {
                    plugin,
                    config: source.config,
                    document_dir: None,
                    routes,
                    visibility: source.visibility,
                    cache: source.cache,
                    public_alias,
                    task_projection: source.task_projection,
                    create,
                },
            );
        }
        // A route may name an incomplete source as where it sends: the command that would
        // follow it is what refuses, so a store whose layers refine a destination they do not
        // declare still loads everywhere.
        let routes = routes_of(&sources, &incomplete);
        routes::check(
            &routes,
            &sources
                .keys()
                .chain(&incomplete)
                .cloned()
                .collect::<Vec<_>>(),
        )?;
        routes::check_classified(&routes, &visibilities_of(&sources))?;

        let default_sources = shape
            .default_sources
            .map(|names| resolve_default_sources(&names, &sources, &incomplete))
            .transpose()?;

        if let Some(policy) = &shape.write_policy
            && !policy.replacements.is_empty()
            && policy.sanitize_command.is_none()
        {
            return Err(ConfigError::setting(
                "write_policy.sanitize_command",
                "write_policy.replacements is set, but no write_policy.sanitize_command is \
                 configured to apply it, so no replacement would ever be made",
                "configure write_policy.sanitize_command — for example \
                 [onevcs, boundary, sanitize, --input, \"-\"] — or remove \
                 write_policy.replacements.",
            ));
        }
        if let Some(policy) = &shape.write_policy
            && policy.sanitize_stored_on_write
            && policy.sanitize_command.is_none()
        {
            return Err(ConfigError::setting(
                "write_policy.sanitize_command",
                "write_policy.sanitize_stored_on_write is set, but no \
                 write_policy.sanitize_command is configured to rewrite what is stored",
                "configure write_policy.sanitize_command — for example \
                 [onevcs, boundary, sanitize, --input, \"-\"] — or remove \
                 write_policy.sanitize_stored_on_write.",
            ));
        }
        let config = Self {
            default_sources,
            page_size: shape.page_size,
            output: shape.output,
            interactive: shape.interactive,
            sources,
            incomplete,
            write_policy: shape.write_policy,
            cache_home: None,
        };
        // Here rather than at the call site, so "a `Config` exists" means "every block in
        // it satisfies the schema its own plugin declares". Checked once at the boundary,
        // a mistyped per-source field cannot survive as far as the HTTP call that would
        // otherwise be the first thing to notice it.
        crate::resolve::validate_sources(&config)?;
        Ok(config)
    }

    /// How many items a page holds.
    #[must_use]
    pub fn page_size(&self) -> NonZeroU32 {
        self.page_size
    }

    /// How output is rendered.
    #[must_use]
    pub fn output(&self) -> OutputFormat {
        self.output
    }

    /// Whether a command may prompt for what it was not given — today, `template render`
    /// asking for each variable no answer covers.
    ///
    /// Off, a command never prompts: what it was not given is refused instead, so automation
    /// neither hangs on a prompt nor renders a required answer empty.
    #[must_use]
    pub fn interactive(&self) -> bool {
        self.interactive
    }

    /// Every configured source, in name order.
    #[must_use]
    pub fn sources(&self) -> &BTreeMap<SourceName, SourceConfig> {
        &self.sources
    }

    /// Every source some layer refines but no layer gives a `plugin`, in name order.
    ///
    /// None of them is configured: [`sources`](Self::sources) leaves each out, and a command
    /// naming one — by `--source`, a qualified id, `default_sources` or a route — is refused
    /// with [`EngineError::IncompleteSource`](crate::EngineError::IncompleteSource).
    #[must_use]
    pub fn incomplete(&self) -> &BTreeSet<SourceName> {
        &self.incomplete
    }

    /// Every source's routes, for placing an item written to one of them.
    #[must_use]
    pub fn routes(&self) -> Routes {
        routes_of(&self.sources, &self.incomplete)
    }

    /// The commands this store asks about the public boundary, when the document names any.
    #[must_use]
    pub fn write_policy(&self) -> Option<&WritePolicyConfig> {
        self.write_policy.as_ref()
    }

    /// Who each source is declared readable by, in name order.
    #[must_use]
    pub fn visibilities(&self) -> BTreeMap<SourceName, SourceVisibility> {
        visibilities_of(&self.sources)
    }

    /// The directory every source's cache defaults to living under, when the environment
    /// the configuration was loaded in names one.
    ///
    /// `None` for a configuration built by [`Config::from_document`], which reads no
    /// environment; [`Self::with_cache_home`] states one.
    #[must_use]
    pub fn cache_home(&self) -> Option<&Path> {
        self.cache_home.as_deref()
    }

    /// The same configuration, with every source's cache defaulting to live under `home`.
    #[must_use]
    pub fn with_cache_home(mut self, home: Option<std::path::PathBuf>) -> Self {
        self.cache_home = home;
        self
    }

    /// Which sources answer when a command names none, or `None` for every one.
    #[must_use]
    pub fn default_sources(&self) -> Option<&[SourceName]> {
        self.default_sources.as_deref()
    }

    /// The sources a command answers from when it names none, in a stable order.
    #[must_use]
    pub fn selected_sources(&self) -> Vec<SourceName> {
        self.default_sources
            .clone()
            .unwrap_or_else(|| self.sources.keys().cloned().collect())
    }
}

/// The routes every source declares, gathered for placing items.
fn routes_of(
    sources: &BTreeMap<SourceName, SourceConfig>,
    incomplete: &BTreeSet<SourceName>,
) -> Routes {
    let mut routes = Routes::default();
    for (name, source) in sources {
        routes.insert(name.clone(), source.routes.clone());
    }
    routes
        .with_visibility(visibilities_of(sources))
        .with_incomplete(incomplete.clone())
}

/// Who each source is declared readable by.
fn visibilities_of(
    sources: &BTreeMap<SourceName, SourceConfig>,
) -> BTreeMap<SourceName, SourceVisibility> {
    sources
        .iter()
        .map(|(name, source)| (name.clone(), source.visibility))
        .collect()
}

/// Check every `default_sources` entry against the sources that exist.
///
/// An entry naming an incomplete source is kept, so the commands that read the default
/// selection refuse it by name and the ones that name their own sources still run.
fn resolve_default_sources(
    names: &[String],
    sources: &BTreeMap<SourceName, SourceConfig>,
    incomplete: &BTreeSet<SourceName>,
) -> Result<Vec<SourceName>, ConfigError> {
    names
        .iter()
        .map(|name| {
            let selected = SourceName::new(name.clone()).map_err(|error| {
                ConfigError::setting(
                    "default_sources",
                    error.to_string(),
                    "name a configured source; `onetaskgraph config show` lists them.",
                )
            })?;
            if sources.contains_key(&selected) || incomplete.contains(&selected) {
                Ok(selected)
            } else {
                Err(ConfigError::setting(
                    "default_sources",
                    format!("no source named {name:?} is configured"),
                    format!(
                        "name one of the configured sources ({}), or configure {name:?} under \
                         `sources`.",
                        source_list(sources)
                    ),
                ))
            }
        })
        .collect()
}

/// The configured source names, for a message.
fn source_list(sources: &BTreeMap<SourceName, SourceConfig>) -> String {
    if sources.is_empty() {
        "none are".to_owned()
    } else {
        sources
            .keys()
            .map(SourceName::as_str)
            .collect::<Vec<_>>()
            .join(", ")
    }
}

/// A configuration, the credentials behind it, and where every setting came from.
#[derive(Debug, Clone)]
pub struct Loaded {
    /// The configuration itself.
    pub config: Config,
    /// Where a plugin's named credential is looked up. Read before sources resolve.
    pub secrets: Secrets,
    /// Every setting with the layer it came from, for `config show`.
    pub effective: EffectiveConfig,
}

/// The output format a run asked for, read from whichever layers can still be read.
///
/// For a run whose configuration did not load, which still owes its failure in the format
/// it asked for: `--json` on a command line beside a document that will not parse asks
/// for machine output as plainly as it does beside one that will. The same layers in the
/// same precedence as [`load`], each skipped on its own when it cannot be read or parsed,
/// so a layer that is itself the failure contributes nothing rather than hiding the others.
/// `working_directory` is `None` when there is none to search from, and then no project
/// document is read. Text when no readable layer sets a usable format.
#[must_use]
pub fn requested_output(
    working_directory: Option<&Path>,
    environment: &Environment,
    flags: &Layer,
) -> OutputFormat {
    let mut layers: Vec<Layer> = readable_documents(working_directory, environment)
        .into_iter()
        .filter_map(|document| {
            let parsed: Value = crate::yaml::from_str(&document.text).ok()?;
            Layer::from_document(document.path, &parsed).ok()
        })
        .collect();
    layers.extend(environment_layer::layer(environment).ok());
    layers.push(flags.clone());
    merge(&layers)
        .values()
        .find(|setting| setting.key.segments() == ["output"])
        .and_then(|setting| serde_json::from_value(setting.value.clone()).ok())
        .unwrap_or_default()
}

/// Load the configuration: documents, then the environment, then `flags`.
///
/// Each source's `config` block is checked against its plugin's declared schema
/// before this returns, so a mistyped per-source field is a load-time refusal rather
/// than a surprise inside the first call that source makes.
///
/// # Errors
///
/// Returns [`ConfigError`] for a document that cannot be read or parsed, and for any
/// setting that is unknown, unusable, or names a plugin this build does not have.
pub fn load(
    working_directory: &Path,
    environment: &Environment,
    flags: &Layer,
) -> Result<Loaded, ConfigError> {
    let mut layers = Vec::new();
    for document in documents(working_directory, environment)? {
        let parsed: Value =
            crate::yaml::from_str(&document.text).map_err(|error| ConfigError::Syntax {
                path: document.path.clone(),
                message: error.to_string(),
            })?;
        layers.push(Layer::from_document(document.path, &parsed)?);
    }
    layers.push(environment_layer::layer(environment)?);
    layers.push(flags.clone());

    let mut merged = merge(&layers);
    // Before the block reaches a plugin, and before `config show` reports it: a plugin is
    // handed values and no origins, so this is the only layer that can tell a path a
    // document supplied from one the environment or a flag did. See [`relative`].
    resolve_document_relative_paths(&mut merged)?;
    let mut config =
        Config::from_document(unflatten(&merged))?.with_cache_home(cache_home(environment));
    for (name, source) in &mut config.sources {
        if source.plugin == PluginKind::Subprocess {
            source.document_dir = relative::supplying_document_dir(&merged, name.as_str())?;
        }
    }

    // Before the sources are resolved, as the contract says: a plugin reads its
    // credential through this resolver, so it has to exist by the time one is built.
    let secrets = Secrets::load(environment.clone())?;

    Ok(Loaded {
        effective: EffectiveConfig::new(&merged, &config, secrets.report()),
        config,
        secrets,
    })
}

/// `$XDG_CACHE_HOME`, or `$HOME/.cache`, or nothing when neither is set — the XDG rule
/// [`discovery`] applies to configuration, applied to the cache.
#[must_use]
pub fn cache_home(environment: &Environment) -> Option<std::path::PathBuf> {
    if let Some(xdg) = environment.non_empty("XDG_CACHE_HOME") {
        return Some(std::path::PathBuf::from(xdg));
    }
    Some(std::path::PathBuf::from(environment.non_empty("HOME")?).join(".cache"))
}

/// The refusal of a source whose `plugin:` names no kind this build compiled.
///
/// A kind this crate could register but this build's features left out is named with the
/// feature that compiles it, because "no such plugin" would send the reader looking for a
/// typo in a name that is spelled correctly.
fn plugin_not_in_this_build(key: &str, plugin: &str) -> ConfigError {
    let kinds = plugin_kinds().join(", ");
    match omitted_feature(plugin) {
        Some(feature) => ConfigError::setting(
            format!("{key}.plugin"),
            format!("the {plugin:?} plugin is not compiled into this binary"),
            format!(
                "use a build of onetaskgraph-core with its `{feature}` feature enabled, or \
                 one of: {kinds}."
            ),
        ),
        None => ConfigError::setting(
            format!("{key}.plugin"),
            format!("no plugin named {plugin:?} is built into this binary"),
            format!("use one of: {kinds}."),
        ),
    }
}
