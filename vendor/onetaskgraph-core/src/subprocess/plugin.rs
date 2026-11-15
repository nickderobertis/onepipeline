//! Configuring a source that is another program.

use std::collections::BTreeMap;
use std::num::NonZeroU64;
use std::path::Path;

use onetaskgraph_plugin_api::{SecretResolver, SourceError, SourceName, SourcePlugin, TaskSource};
use schemars::{JsonSchema, Schema, schema_for};
use secrecy::ExposeSecret;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::source::{RequestDeadline, SubprocessSource};
use crate::secrets::CredentialName;

/// The name a configuration document's `plugin:` field names this kind by.
pub(crate) const KIND: &str = "subprocess";

/// The field of [`SubprocessConfig`] handed to the child as its `config:` block.
///
/// Named once because the configuration layer reads it too: it is the block whose
/// supplying document the handshake reports as `document_dir`.
pub(crate) const SETTINGS_FIELD: &str = "settings";

/// How to run a plugin that speaks `docs/plugin-protocol.md`.
///
/// `settings` is the seam that keeps this one plugin general: it is handed to the child
/// as its `config:` block verbatim, so what a Python source needs and what a Rust one
/// needs are that child's business rather than a field of this schema. Which is also why
/// the credentials a child may see are *named* here rather than inherited: §3.1 forbids a
/// plugin reading credentials from its own environment, and forwarding the engine's whole
/// environment would hand every plugin every secret on the host.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SubprocessConfig {
    /// The program to run.
    pub command: Program,
    /// Arguments to run it with.
    #[serde(default)]
    pub args: Vec<String>,
    /// The environment variables whose resolved values the handshake forwards.
    #[serde(default)]
    pub secrets: Vec<CredentialName>,
    /// This source's own settings, handed to the child verbatim.
    #[serde(default)]
    pub settings: Value,
    /// Defaults to 30 seconds and cannot be zero.
    #[serde(default = "default_deadline_ms")]
    pub deadline_ms: NonZeroU64,
}

fn default_deadline_ms() -> NonZeroU64 {
    RequestDeadline::DEFAULT.milliseconds()
}

/// The program that serves a source: a name that is not blank.
///
/// A newtype rather than a `String` checked on the way past, because the check has to hold
/// wherever one of these comes from. A blank command is not a source that fails later; it
/// is a source that was never configured, and the difference is the difference between a
/// sentence naming the field to fill in and a spawn error about an empty path.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(into = "String", try_from = "String")]
// schemars does not read `serde(into)`, and this is a string on the wire.
#[schemars(with = "String")]
pub struct Program(String);

impl Program {
    /// `command` when it names something, and nothing otherwise.
    #[must_use]
    pub fn new(command: &str) -> Option<Self> {
        (!command.trim().is_empty()).then(|| Self(command.to_owned()))
    }

    /// The program itself.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for Program {
    type Error = String;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(&value)
            .ok_or_else(|| "`command` must name the program that serves this source".to_owned())
    }
}

impl From<Program> for String {
    fn from(value: Program) -> Self {
        value.0
    }
}

/// The factory a configuration reaches through `plugin: subprocess`.
#[derive(Debug, Clone, Copy, Default)]
pub struct Plugin;

impl SourcePlugin for Plugin {
    fn kind(&self) -> &'static str {
        KIND
    }

    fn config_schema(&self) -> Schema {
        schema_for!(SubprocessConfig)
    }

    fn build(
        &self,
        name: &SourceName,
        config: &Value,
        secrets: &dyn SecretResolver,
    ) -> Result<Box<dyn TaskSource>, SourceError> {
        self.build_from_document(name, config, secrets, None)
    }
}

impl Plugin {
    /// [`SourcePlugin::build`], telling the child which document supplied its settings.
    ///
    /// `document_dir` is [`SourceConfig::document_dir`](crate::SourceConfig::document_dir):
    /// the engine reaches this rather than the trait method because the trait hands a
    /// plugin values and no origins, and this plugin alone passes an origin on — as
    /// `document_dir` in the handshake (`docs/plugin-protocol.md` §3).
    ///
    /// # Errors
    ///
    /// What [`SourcePlugin::build`] returns.
    pub fn build_from_document(
        &self,
        name: &SourceName,
        config: &Value,
        secrets: &dyn SecretResolver,
        document_dir: Option<&Path>,
    ) -> Result<Box<dyn TaskSource>, SourceError> {
        let config: SubprocessConfig =
            serde_json::from_value(config.clone()).map_err(|error| SourceError::Config {
                message: format!("source {name}: {error}"),
            })?;
        let forwarded = resolve_named(name, &config.secrets, secrets)?;
        SubprocessSource::connect_from_document(
            config.command.as_str(),
            &config.args,
            name,
            &config.settings,
            forwarded,
            RequestDeadline::from_millis(config.deadline_ms),
            document_dir,
        )
        .map(|source| Box::new(source) as Box<dyn TaskSource>)
    }
}

/// The values of exactly the variables this configuration names, and nothing else.
///
/// A named variable nothing defines is refused here rather than forwarded as absent: the
/// plugin asked for it, so a run that spawned anyway would fail later inside the child
/// with a message about a credential, and the thing the user has to fix is on this side.
fn resolve_named(
    name: &SourceName,
    named: &[CredentialName],
    secrets: &dyn SecretResolver,
) -> Result<BTreeMap<String, String>, SourceError> {
    let mut forwarded = BTreeMap::new();
    for variable in named {
        let value = secrets
            .get(variable.as_str())
            .ok_or_else(|| SourceError::Auth {
                message: format!(
                    "source {name}: nothing defines {variable}, which this source's \
                     `secrets` names; export it, or add it to the credentials file"
                ),
            })?;
        forwarded.insert(
            variable.as_str().to_owned(),
            value.expose_secret().to_owned(),
        );
    }
    Ok(forwarded)
}
