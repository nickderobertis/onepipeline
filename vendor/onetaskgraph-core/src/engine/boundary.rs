//! The public boundary: what may be written where, decided once for every writer.
//!
//! Every write this engine makes passes through here before its first mutation. Two facts
//! decide it, and the store owns only the first:
//!
//! - **Where it lands.** A source's configuration declares its `visibility`, and a source
//!   declared private is held to its backend's own live answer at every write
//!   ([`TaskSource::visibility`]) — read once per destination for a whole copy, before its
//!   first write, and held for its writes ([`Engine::hold_visibility`]). An item that may
//!   only be written somewhere private is written only there.
//! - **What it says.** A write reaching a destination that is not verified private is put to
//!   the caller's [`WritePolicy`], which decides whether its text names anything private.
//!   The store never derives a term itself, and never decides a repository's visibility
//!   itself: both are the policy's, so a linking caller supplies its own and the command line
//!   runs the commands `write_policy` names ([`CommandWritePolicy`]).
//!
//! **The boundary is active** when the configuration names a `write_policy`, when any source
//! declares a `visibility`, or when a linking caller supplies a [`WritePolicy`]
//! ([`Engine::with_write_policy`]). An inactive store still persists, copies and routes
//! classification, and still refuses an explicitly private item anywhere not declared private
//! — but it consults no repository's visibility and reads nothing extra before a narrow write,
//! so a store that has not opted in answers and spends exactly what it did before. A store that
//! wants repositories screened has to configure `write_policy` or declare a visibility.
//!
//! [`TaskSource::visibility`]: onetaskgraph_plugin_api::TaskSource::visibility

use std::collections::BTreeMap;
use std::io::Write;
use std::process::Stdio;
use std::sync::{Arc, LazyLock};

use onetaskgraph_plugin_api::{
    Classification, Comment, CommentBody, Document, Label, NativeId, PageRequest, Project,
    Repository, SourceError, Task, Visibility, WriteTarget,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::config::{PolicyCommand, SourceVisibility, WritePolicyConfig};
use crate::resolve::ResolvedSource;

use super::{Engine, EngineError};

/// Whether a repository is public, as a caller's policy answers.
///
/// Only `public` is public: `unknown` is what a policy answers when it cannot say, and the
/// store treats it exactly as `private`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum RepositoryVisibility {
    /// Anybody can read it.
    Public,
    /// It is private, or internal to an organisation.
    Private,
    /// Nobody has said. Treated as private.
    Unknown,
}

impl RepositoryVisibility {
    /// Whether this is [`Public`](Self::Public), the one value that is.
    #[must_use]
    pub const fn is_public(self) -> bool {
        matches!(self, Self::Public)
    }
}

/// What one write is about to put somewhere, put to [`WritePolicy::check_public_write`].
///
/// Serialized exactly as onevcs's `BoundaryInput` reads it, and the reconciliation journey
/// holds the two to one schema: `scope` is left out when absent, which a check reads as every
/// private identity it knows of, and an empty list derives no term at all.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PublicWriteInput {
    /// Where it is going.
    pub destination: RepositoryVisibility,
    /// Prose and file contents.
    #[serde(default)]
    pub text: Vec<String>,
    /// Paths it names or writes.
    #[serde(default)]
    pub paths: Vec<String>,
    /// Titles, labels, metadata and every other short field.
    #[serde(default)]
    pub metadata: Vec<String>,
    /// Which repositories' private terms the check derives, as the caller scoped it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<Vec<String>>,
}

/// What a check decided about one write.
///
/// Every reason is neutral — it says where the check stopped and never what it found — so a
/// refusal can be printed wherever the write was asked for.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "verdict", rename_all = "kebab-case")]
pub enum WriteVerdict {
    /// Nothing private was found.
    Pass,
    /// Something private was found, and the write must not go ahead.
    Refuse {
        /// Where, in neutral words.
        reason: String,
    },
    /// The check could not decide, which is never a pass.
    Unavailable {
        /// Why, in neutral words.
        reason: String,
    },
}

/// A policy that could not be asked at all.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{message}")]
pub struct PolicyError {
    /// What went wrong, in neutral words.
    pub message: String,
}

impl PolicyError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

/// One replacement entry of the caller's layer: `{term, with}` or `{term, reject: true}`.
///
/// The private-terms specification's own shape, never a copy of it.
pub use privateterms::Replacement;

/// One resolved term, as [`WritePolicy::declared_terms`] answers it: private output, held in
/// memory and never written anywhere.
pub type DeclaredTerm = privateterms::ResolvedRule;

pub use privateterms::{Locator, LocatorKind};

/// Entries a sanitisation checks and never rewrites: every id, a metadata key, a status, a
/// priority, a repository and an asset's name.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FixedEntries {
    /// Prose checked as it stands.
    #[serde(default)]
    pub text: Vec<String>,
    /// Paths checked as they stand.
    #[serde(default)]
    pub paths: Vec<String>,
    /// Short fields checked as they stand.
    #[serde(default)]
    pub metadata: Vec<String>,
}

impl FixedEntries {
    fn is_empty(&self) -> bool {
        self.text.is_empty() && self.paths.is_empty() && self.metadata.is_empty()
    }
}

/// One identifier a diagnostic of this write would print, screened in the write's own call.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LocatorEntry {
    /// What it names.
    pub kind: LocatorKind,
    /// The identifier itself.
    pub value: String,
}

/// What one write is about to put somewhere, put to [`WritePolicy::sanitize_public_write`].
///
/// Serialized exactly as onevcs's `SanitizeInput` reads it, held to the pinned schema: the
/// rewritable entries, the `fixed` ones, the caller's `replacements` layer and the `locators`
/// its diagnostics would print. An empty member is left out, which the schema reads as empty.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PublicSanitizeInput {
    /// Where it is going.
    pub destination: RepositoryVisibility,
    /// Prose that may be rewritten.
    #[serde(default)]
    pub text: Vec<String>,
    /// Paths that may be rewritten.
    #[serde(default)]
    pub paths: Vec<String>,
    /// Short fields that may be rewritten.
    #[serde(default)]
    pub metadata: Vec<String>,
    /// Entries checked and never rewritten.
    #[serde(default, skip_serializing_if = "FixedEntries::is_empty")]
    pub fixed: FixedEntries,
    /// Which repositories' private terms are derived, as the caller scoped it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<Vec<String>>,
    /// The caller's replacement layer, over every declaration level: a later entry wins.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[schemars(with = "Vec<Value>")]
    pub replacements: Vec<Replacement>,
    /// The identifiers this write's diagnostics would print.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub locators: Vec<LocatorEntry>,
}

/// Which list an entry still carrying a term after replacement is in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum SanitizeSurface {
    /// A `text` entry.
    Text,
    /// A `paths` entry.
    Path,
    /// A `metadata` entry.
    Metadata,
    /// A `locators` entry.
    Locators,
}

impl SanitizeSurface {
    /// The word a neutral reason names it by.
    fn word(self) -> &'static str {
        match self {
            Self::Text => "text",
            Self::Path => "path",
            Self::Metadata => "metadata",
            Self::Locators => "identifiers",
        }
    }
}

/// One entry still carrying a term after replacement: where it is, never what.
// llmlint: ignore[invalid_states_unrepresentable] The shape is onevcs's pinned `Unfixed`, field for field, which the reconciliation journey holds to the released schema; a locator entry with `fixed: false` is refused by `answer_problem` rather than made unrepresentable here, because a type of its own would no longer be that schema's.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Unfixed {
    /// Which list.
    pub surface: SanitizeSurface,
    /// Its index in that list — in `fixed`'s list where `fixed` is true.
    pub index: u64,
    /// Whether it is an entry nothing rewrites: a `fixed` one, or a locator.
    pub fixed: bool,
}

/// What a sanitisation decided, and the entries as they may be written.
///
/// `text`, `paths` and `metadata` are the input's rewritable entries after replacement, the
/// same length and order; `locators` the input's locators screened, the same length and order,
/// and empty when the policy could not decide.
// llmlint: ignore[invalid_states_unrepresentable] Contract 5 states this struct's fields exactly — a `WriteVerdict` beside the rewritten entries — and `op-link-sanitize` implements it from there; a verdict contradicting its payload is refused by `answer_problem` for every policy rather than unrepresentable, because reshaping it would break the agreed interface.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SanitizeVerdict {
    /// Whether what is answered may be written.
    #[serde(flatten)]
    pub verdict: WriteVerdict,
    /// The `text` entries after replacement.
    #[serde(default)]
    pub text: Vec<String>,
    /// The `paths` entries after replacement.
    #[serde(default)]
    pub paths: Vec<String>,
    /// The `metadata` entries after replacement.
    #[serde(default)]
    pub metadata: Vec<String>,
    /// How many spans were rewritten.
    #[serde(default)]
    pub replaced: u32,
    /// Every entry still carrying a term.
    #[serde(default)]
    pub unfixed: Vec<Unfixed>,
    /// The locators screened.
    #[serde(default)]
    pub locators: Vec<Locator>,
}

/// What [`WritePolicy::declared_terms`] is asked: a scope and the caller's replacement layer,
/// serialized exactly as onevcs's `TermsRequest`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TermsRequest {
    /// Which private repositories the terms come from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<Vec<String>>,
    /// The caller's replacement layer.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[schemars(with = "Vec<Value>")]
    pub replacements: Vec<Replacement>,
}

/// The caller's half of the public boundary: which repositories are public, and whether a
/// write may reach a public destination.
///
/// A linking caller implements it directly and hands it to [`Engine::with_write_policy`];
/// the command line and both SDKs run the commands a configuration's `write_policy` names
/// through [`CommandWritePolicy`]. Either way the store applies one rule over its answers,
/// and **nothing here is allowed to pass by failing**: an error answering a repository's
/// visibility makes it private, and an error checking a write makes the check unavailable.
#[async_trait::async_trait]
pub trait WritePolicy: Send + Sync {
    /// Whether `repository` is public.
    ///
    /// # Errors
    ///
    /// [`PolicyError`] when it cannot be asked; the store treats that as `unknown`.
    async fn repository_visibility(
        &self,
        repository: &Repository,
    ) -> Result<RepositoryVisibility, PolicyError>;

    /// Whether `input` may be written to its destination.
    ///
    /// # Errors
    ///
    /// [`PolicyError`] when it cannot be asked; the store treats that as unavailable.
    async fn check_public_write(
        &self,
        input: &PublicWriteInput,
    ) -> Result<WriteVerdict, PolicyError>;

    /// Rewrite `input`'s rewritable entries with the declared replacements and decide the
    /// result.
    ///
    /// The default makes one [`check_public_write`](Self::check_public_write) over every entry,
    /// rewritable and fixed, and every locator's value as fixed metadata, and answers the
    /// entries unchanged: the locators all plain on a pass, all redacted on a refusal or an
    /// unavailable check, since it cannot say which entry the check stopped at.
    ///
    /// # Errors
    ///
    /// [`PolicyError`] when it cannot be asked; the store treats that as unavailable.
    async fn sanitize_public_write(
        &self,
        input: &PublicSanitizeInput,
    ) -> Result<SanitizeVerdict, PolicyError> {
        let check = PublicWriteInput {
            destination: input.destination,
            text: input
                .text
                .iter()
                .chain(&input.fixed.text)
                .cloned()
                .collect(),
            paths: input
                .paths
                .iter()
                .chain(&input.fixed.paths)
                .cloned()
                .collect(),
            metadata: input
                .metadata
                .iter()
                .chain(&input.fixed.metadata)
                .cloned()
                .chain(input.locators.iter().map(|locator| locator.value.clone()))
                .collect(),
            scope: input.scope.clone(),
        };
        let verdict = self.check_public_write(&check).await?;
        let locators = match verdict {
            WriteVerdict::Pass => input
                .locators
                .iter()
                .map(|locator| Locator::Plain(locator.value.clone()))
                .collect(),
            WriteVerdict::Refuse { .. } => redacted(&input.locators),
            WriteVerdict::Unavailable { .. } => Vec::new(),
        };
        Ok(SanitizeVerdict {
            verdict,
            text: input.text.clone(),
            paths: input.paths.clone(),
            metadata: input.metadata.clone(),
            replaced: 0,
            unfixed: Vec::new(),
            locators,
        })
    }

    /// The resolved terms of `request`'s scope, or `None` when this policy cannot list them.
    ///
    /// The terms are private output: the store holds them in memory for one sweep and never
    /// writes one anywhere. The default cannot list them.
    ///
    /// # Errors
    ///
    /// [`PolicyError`] when it cannot be asked.
    async fn declared_terms(
        &self,
        _request: &TermsRequest,
    ) -> Result<Option<Vec<DeclaredTerm>>, PolicyError> {
        Ok(None)
    }
}

/// Every locator of `locators`, redacted and numbered from 1 in order.
fn redacted(locators: &[LocatorEntry]) -> Vec<Locator> {
    let kinds: Vec<(LocatorKind, &str)> = locators
        .iter()
        .map(|locator| (locator.kind, locator.value.as_str()))
        .collect();
    privateterms::redact_all(&kinds, 1)
}

/// The policy a store with no `write_policy` asks: every repository unknown, every check
/// unavailable — so an active store without one treats every repository's item as private,
/// which only a destination verified private takes, and writes nothing at all to any other.
#[derive(Debug, Clone, Copy, Default)]
pub struct MissingWritePolicy;

#[async_trait::async_trait]
impl WritePolicy for MissingWritePolicy {
    async fn repository_visibility(
        &self,
        _repository: &Repository,
    ) -> Result<RepositoryVisibility, PolicyError> {
        Ok(RepositoryVisibility::Unknown)
    }

    async fn check_public_write(
        &self,
        _input: &PublicWriteInput,
    ) -> Result<WriteVerdict, PolicyError> {
        Ok(WriteVerdict::Unavailable {
            reason: "no write_policy.check_command is configured".to_owned(),
        })
    }
}

/// The schemas onevcs's `boundary` commands exchange, as the released `onevcs boundary schema
/// --json` printed them. What this adapter sends and accepts is validated against them, and
/// the reconciliation journey fails the moment the released command prints anything else.
pub const ONEVCS_BOUNDARY_SCHEMA: &str = include_str!("boundary/onevcs-boundary-schema-v4.json");

/// The `schema_version` of [`ONEVCS_BOUNDARY_SCHEMA`], which an answer is held to.
pub const ONEVCS_BOUNDARY_SCHEMA_VERSION: u64 = 4;

/// One compiled validator per schema half, built once.
struct Validators {
    inspect_input: jsonschema::Validator,
    inspect_output: jsonschema::Validator,
    check_input: jsonschema::Validator,
    check_output: jsonschema::Validator,
    sanitize_input: jsonschema::Validator,
    sanitize_output: jsonschema::Validator,
    terms_input: jsonschema::Validator,
    terms_output: jsonschema::Validator,
    terms_unavailable: jsonschema::Validator,
}

static VALIDATORS: LazyLock<Validators> = LazyLock::new(|| {
    let schema: Value =
        serde_json::from_str(ONEVCS_BOUNDARY_SCHEMA).expect("the pinned boundary schema is JSON");
    assert_eq!(
        schema.get("schema_version").and_then(Value::as_u64),
        Some(ONEVCS_BOUNDARY_SCHEMA_VERSION),
        "the pinned boundary schema is the version this adapter speaks"
    );
    let compile = |pointer: &str| {
        jsonschema::validator_for(schema.pointer(pointer).expect("the pinned schema has it"))
            .expect("the pinned boundary schema compiles")
    };
    // The terms branch allows extra properties, so an accompanying verdict must
    // independently satisfy the unavailable branch before its reason is diagnostic text.
    let mut terms_unavailable = schema["terms"]["output"].clone();
    terms_unavailable
        .as_object_mut()
        .expect("an object schema")
        .remove("anyOf");
    terms_unavailable["$ref"] = Value::String("#/$defs/TermsUnavailable".to_owned());
    Validators {
        inspect_input: compile("/inspect/input"),
        inspect_output: compile("/inspect/output"),
        check_input: compile("/check/input"),
        check_output: compile("/check/output"),
        sanitize_input: compile("/sanitize/input"),
        sanitize_output: compile("/sanitize/output"),
        terms_input: compile("/terms/input"),
        terms_output: compile("/terms/output"),
        terms_unavailable: jsonschema::validator_for(&terms_unavailable)
            .expect("the pinned unavailable terms schema compiles"),
    }
});

/// The policy behind `write_policy`: two generic commands, each handed one JSON document on
/// its standard input.
///
/// - `visibility_command` is handed `{"repository": "<host/owner/name>"}` and answers
///   `{"visibility": …}` on its standard output, exiting 0. Anything else — no command, a
///   non-zero exit, an answer the pinned schema refuses — is `unknown`, which is private.
/// - `check_command` is handed the whole [`PublicWriteInput`] and **passes a write only by
///   exiting 0 with `{"verdict": "pass"}`**. Exit 1 with a refusal is a refusal; every other
///   outcome, a missing command included, is unavailable.
///
/// Both the documents sent and the answers read are held to onevcs's versioned boundary
/// schema ([`ONEVCS_BOUNDARY_SCHEMA`]), so a command speaking anything else is not believed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandWritePolicy {
    visibility_command: Option<PolicyCommand>,
    check_command: Option<PolicyCommand>,
    sanitize_command: Option<PolicyCommand>,
    terms_command: Option<PolicyCommand>,
}

impl CommandWritePolicy {
    /// The policy a configuration's `write_policy` names.
    #[must_use]
    pub fn new(config: &WritePolicyConfig) -> Self {
        Self {
            visibility_command: config.visibility_command.clone(),
            check_command: config.check_command.clone(),
            sanitize_command: config.sanitize_command.clone(),
            terms_command: config.terms_command.clone(),
        }
    }
}

/// One command run to completion over `input`: its exit code, if it exited, and its stdout.
///
/// Run on a thread of its own and awaited through a channel, as a hosted plugin's process is,
/// so the engine starts no runtime of its own and blocks none its caller started.
async fn run(
    command: &PolicyCommand,
    input: &Value,
) -> Result<(Option<i32>, Vec<u8>), PolicyError> {
    let program = command.program().to_owned();
    let arguments = command.arguments().to_vec();
    let document = serde_json::to_vec(input).expect("a JSON value serializes");
    let (answered, answer) = tokio::sync::oneshot::channel();
    let runner = std::thread::Builder::new().name("onetaskgraph-policy".to_owned());
    runner
        .spawn(move || {
            let ran = (|| {
                let mut child = std::process::Command::new(&program)
                    .args(&arguments)
                    .stdin(Stdio::piped())
                    .stdout(Stdio::piped())
                    .stderr(Stdio::null())
                    .spawn()
                    .map_err(|error| {
                        PolicyError::new(format!(
                            "the command {program} could not be started: {error}"
                        ))
                    })?;
                if let Some(mut stdin) = child.stdin.take() {
                    // A command that exits without reading its input closes the pipe; what it
                    // answered is still read below, and decides.
                    let _ = stdin.write_all(&document);
                }
                let output = child.wait_with_output().map_err(|error| {
                    PolicyError::new(format!("the command {program} could not be read: {error}"))
                })?;
                Ok((output.status.code(), output.stdout))
            })();
            let _ = answered.send(ran);
        })
        .map_err(|error| {
            PolicyError::new(format!(
                "the command {} could not be run: {error}",
                command.program()
            ))
        })?;
    answer
        .await
        .map_err(|_| PolicyError::new("the command's runner stopped without an answer"))?
}

/// `bytes` as one JSON document the validator accepts, or `None`.
fn validated(bytes: &[u8], validator: &jsonschema::Validator) -> Option<Value> {
    let answer: Value = serde_json::from_slice(bytes).ok()?;
    validator.is_valid(&answer).then_some(answer)
}

#[async_trait::async_trait]
impl WritePolicy for CommandWritePolicy {
    async fn repository_visibility(
        &self,
        repository: &Repository,
    ) -> Result<RepositoryVisibility, PolicyError> {
        let Some(command) = &self.visibility_command else {
            return Ok(RepositoryVisibility::Unknown);
        };
        let request = serde_json::json!({ "repository": repository.as_str() });
        debug_assert!(VALIDATORS.inspect_input.is_valid(&request));
        let (code, stdout) = run(command, &request).await?;
        if code != Some(0) {
            return Err(PolicyError::new(format!(
                "the visibility command {} did not answer",
                command.program()
            )));
        }
        let answer = validated(&stdout, &VALIDATORS.inspect_output).ok_or_else(|| {
            PolicyError::new(format!(
                "the visibility command {} answered something the boundary schema refuses",
                command.program()
            ))
        })?;
        serde_json::from_value(answer["visibility"].clone())
            .map_err(|error| PolicyError::new(error.to_string()))
    }

    async fn check_public_write(
        &self,
        input: &PublicWriteInput,
    ) -> Result<WriteVerdict, PolicyError> {
        let Some(command) = &self.check_command else {
            return MissingWritePolicy.check_public_write(input).await;
        };
        let document = serde_json::to_value(input).expect("a write input serializes");
        if !VALIDATORS.check_input.is_valid(&document) {
            return Ok(WriteVerdict::Unavailable {
                reason: "the write could not be put in the shape the boundary schema reads"
                    .to_owned(),
            });
        }
        let (code, stdout) = run(command, &document).await?;
        Ok(check_verdict(code, &stdout))
    }

    /// Without a `sanitize_command`, the default: one check through `check_command`, exactly as
    /// before. With one, `boundary sanitize`'s answer, held to the pinned schema.
    async fn sanitize_public_write(
        &self,
        input: &PublicSanitizeInput,
    ) -> Result<SanitizeVerdict, PolicyError> {
        let Some(command) = &self.sanitize_command else {
            return default_sanitize(self, input).await;
        };
        let document = serde_json::to_value(input).expect("a sanitise input serializes");
        if !VALIDATORS.sanitize_input.is_valid(&document) {
            return Ok(unavailable(
                "the write could not be put in the shape the boundary schema reads",
            ));
        }
        let (code, stdout) = run(command, &document).await?;
        Ok(sanitize_verdict(input, code, &stdout))
    }

    async fn declared_terms(
        &self,
        request: &TermsRequest,
    ) -> Result<Option<Vec<DeclaredTerm>>, PolicyError> {
        let Some(command) = &self.terms_command else {
            return Ok(None);
        };
        let document = serde_json::to_value(request).expect("a terms request serializes");
        if !VALIDATORS.terms_input.is_valid(&document) {
            return Err(PolicyError::new(
                "the terms request could not be put in the shape the boundary schema reads",
            ));
        }
        let (code, stdout) = run(command, &document).await?;
        let answer = validated(&stdout, &VALIDATORS.terms_output);
        // An undecided verdict is believed first, beside terms or not: no answer that says it
        // could not decide is read as having decided.
        let terms = match (code, answer) {
            (_, Some(answer)) if VALIDATORS.terms_unavailable.is_valid(&answer) => {
                return Err(PolicyError::new(format!(
                    "the terms command could not decide ({})",
                    answer["reason"].as_str().unwrap_or("no reason given")
                )));
            }
            (Some(0), Some(answer))
                if answer.get("verdict").is_none() && answer.get("terms").is_some() =>
            {
                answer["terms"].clone()
            }
            (code, _) => {
                return Err(PolicyError::new(format!(
                    "the terms command {} {} without terms this store believes",
                    command.program(),
                    code.map_or_else(|| "was stopped".to_owned(), |code| format!("exited {code}"))
                )));
            }
        };
        let terms: Vec<TermWire> = serde_json::from_value(terms).map_err(|_| {
            PolicyError::new("the terms command answered terms this store cannot read")
        })?;
        Ok(Some(terms.into_iter().map(TermWire::resolved).collect()))
    }
}

/// [`WritePolicy::sanitize_public_write`]'s default body, callable from an implementation that
/// overrides it only some of the time.
async fn default_sanitize<P: WritePolicy + ?Sized>(
    policy: &P,
    input: &PublicSanitizeInput,
) -> Result<SanitizeVerdict, PolicyError> {
    struct Defaulted<'a, P: ?Sized>(&'a P);
    #[async_trait::async_trait]
    impl<P: WritePolicy + ?Sized> WritePolicy for Defaulted<'_, P> {
        async fn repository_visibility(
            &self,
            repository: &Repository,
        ) -> Result<RepositoryVisibility, PolicyError> {
            self.0.repository_visibility(repository).await
        }
        async fn check_public_write(
            &self,
            input: &PublicWriteInput,
        ) -> Result<WriteVerdict, PolicyError> {
            self.0.check_public_write(input).await
        }
    }
    Defaulted(policy).sanitize_public_write(input).await
}

/// One term as `boundary terms` answers it, read into the specification's own resolved rule.
#[derive(Deserialize)]
struct TermWire {
    term: String,
    mode: privateterms::TermMode,
    case_sensitive: bool,
    #[serde(flatten)]
    action: ActionWire,
}

/// What a term's hits get, as `boundary terms` spells it: replaced by its `with`, or refused.
#[derive(Deserialize)]
#[serde(tag = "action", rename_all = "kebab-case")]
enum ActionWire {
    Replace { with: String },
    Reject,
}

impl TermWire {
    fn resolved(self) -> DeclaredTerm {
        privateterms::ResolvedRule {
            rule: privateterms::TermRule {
                term: self.term,
                mode: self.mode,
                case_sensitive: self.case_sensitive,
            },
            action: match self.action {
                ActionWire::Replace { with } => privateterms::Action::Replace(with),
                ActionWire::Reject => privateterms::Action::Reject,
            },
            origin: "policy".to_owned(),
        }
    }
}

/// What is wrong with `answer` as an answer to `input`, whichever policy gave it — `None` when
/// nothing is: a decided answer holds one rewritten entry per entry sent and one screened
/// locator per locator sent, each plain locator the very value sent, each `unfixed` entry
/// naming an entry that exists, and a pass leaves none unfixed. An undecided answer is taken
/// as it is.
fn answer_problem(input: &PublicSanitizeInput, answer: &SanitizeVerdict) -> Option<&'static str> {
    if let WriteVerdict::Unavailable { .. } = answer.verdict {
        return None;
    }
    if answer.text.len() != input.text.len()
        || answer.paths.len() != input.paths.len()
        || answer.metadata.len() != input.metadata.len()
    {
        return Some("the policy answered entries that do not match those sent");
    }
    if answer.locators.len() != input.locators.len()
        || answer
            .locators
            .iter()
            .zip(&input.locators)
            .any(|(answered, sent)| match answered {
                Locator::Plain(value) => value != &sent.value,
                Locator::Redacted { kind, ordinal } => *kind != sent.kind || *ordinal == 0,
            })
    {
        return Some("the policy answered locators that do not match those sent");
    }
    // One value sent twice on one surface is one value: answered two ways, the write could not
    // say which it was passed as.
    for (sent, answered) in [
        (&input.text, &answer.text),
        (&input.paths, &answer.paths),
        (&input.metadata, &answer.metadata),
    ] {
        let mut seen = std::collections::HashMap::new();
        for (value, rewritten) in sent.iter().zip(answered) {
            if seen
                .insert(value, rewritten)
                .is_some_and(|earlier| earlier != rewritten)
            {
                return Some("the policy answered one value two ways");
            }
        }
    }
    // A short field that reads as a path is sent on both surfaces, and is written once.
    let as_metadata: std::collections::HashMap<&String, &String> =
        input.metadata.iter().zip(&answer.metadata).collect();
    if input
        .paths
        .iter()
        .zip(&answer.paths)
        .any(|(value, rewritten)| {
            as_metadata
                .get(value)
                .is_some_and(|other| *other != rewritten)
        })
    {
        return Some("the policy answered one value two ways");
    }
    if answer.verdict == WriteVerdict::Pass
        && (!answer.unfixed.is_empty()
            || answer
                .locators
                .iter()
                .any(|locator| matches!(locator, Locator::Redacted { .. })))
    {
        return Some("the policy passed a write it says still carries a term");
    }
    let within = |unfixed: &Unfixed| {
        let length = match (unfixed.surface, unfixed.fixed) {
            (SanitizeSurface::Text, false) => input.text.len(),
            (SanitizeSurface::Path, false) => input.paths.len(),
            (SanitizeSurface::Metadata, false) => input.metadata.len(),
            (SanitizeSurface::Text, true) => input.fixed.text.len(),
            (SanitizeSurface::Path, true) => input.fixed.paths.len(),
            (SanitizeSurface::Metadata, true) => input.fixed.metadata.len(),
            (SanitizeSurface::Locators, true) => input.locators.len(),
            // A locator is never rewritten, so one named unfixed but rewritable is no locator.
            (SanitizeSurface::Locators, false) => 0,
        };
        usize::try_from(unfixed.index).is_ok_and(|index| index < length)
    };
    if !answer.unfixed.iter().all(within) {
        return Some("the policy named an unfixed entry that was never sent");
    }
    if answer.unfixed.iter().any(|unfixed| {
        unfixed.surface == SanitizeSurface::Locators
            && usize::try_from(unfixed.index)
                .ok()
                .and_then(|index| answer.locators.get(index))
                .is_some_and(|locator| matches!(locator, Locator::Plain(_)))
    }) {
        return Some("the policy left an identifier it says carries a term unredacted");
    }
    None
}

/// A sanitisation that could not decide: nothing may be written, and no locator is answered.
fn unavailable(reason: impl Into<String>) -> SanitizeVerdict {
    SanitizeVerdict {
        verdict: WriteVerdict::Unavailable {
            reason: reason.into(),
        },
        text: Vec::new(),
        paths: Vec::new(),
        metadata: Vec::new(),
        replaced: 0,
        unfixed: Vec::new(),
        locators: Vec::new(),
    }
}

/// What a sanitise command's exit `code` and standard output amount to for `input` — the one
/// reading of a `boundary sanitize` answer, held to the pinned schema.
///
/// **Only an explicit pass passes**: exit 0 with `{"verdict": "pass"}` whose entries and
/// locators are as many as were sent. Exit 1 with a refusal is a refusal naming where the
/// first term left is and never what it is; anything else is unavailable.
#[must_use]
pub fn sanitize_verdict(
    input: &PublicSanitizeInput,
    code: Option<i32>,
    stdout: &[u8],
) -> SanitizeVerdict {
    let Some(answer) = validated(stdout, &VALIDATORS.sanitize_output) else {
        return unavailable(format!(
            "the sanitise command {} without a verdict this store believes",
            code.map_or_else(|| "was stopped".to_owned(), |code| format!("exited {code}"))
        ));
    };
    let verdict = answer["verdict"].as_str().unwrap_or_default();
    if verdict == "unavailable" {
        return unavailable(format!(
            "the check could not decide ({})",
            answer["reason"].as_str().unwrap_or("no reason given")
        ));
    }
    if !matches!((code, verdict), (Some(0), "pass") | (Some(1), "refuse")) {
        return unavailable("the sanitise command's exit status disagrees with its verdict");
    }
    let strings = |name: &str| -> Vec<String> {
        answer[name]
            .as_array()
            .map(|values| {
                values
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default()
    };
    let (text, paths, metadata) = (strings("text"), strings("paths"), strings("metadata"));
    let locators: Vec<Locator> = answer
        .get("locators")
        .cloned()
        .map(serde_json::from_value)
        .transpose()
        .ok()
        .flatten()
        .unwrap_or_default();
    if text.len() != input.text.len()
        || paths.len() != input.paths.len()
        || metadata.len() != input.metadata.len()
        || locators.len() != input.locators.len()
    {
        return unavailable("the sanitise command answered entries that do not match those sent");
    }
    let unfixed: Vec<Unfixed> =
        serde_json::from_value(answer["unfixed"].clone()).unwrap_or_default();
    let verdict = if verdict == "pass" {
        WriteVerdict::Pass
    } else {
        let surface: Option<SanitizeSurface> =
            serde_json::from_value(answer["surface"].clone()).ok();
        WriteVerdict::Refuse {
            reason: format!(
                "it carries a term of a private repository in its {}",
                surface.map_or("content", SanitizeSurface::word)
            ),
        }
    };
    SanitizeVerdict {
        verdict,
        text,
        paths,
        metadata,
        replaced: answer["replaced"]
            .as_u64()
            .and_then(|replaced| u32::try_from(replaced).ok())
            .unwrap_or_default(),
        unfixed,
        locators,
    }
}

/// What a check command's exit `code` and standard output amount to — the one reading of a
/// `boundary check` answer, held to the pinned schema.
///
/// **Only an explicit pass passes**: exit 0 with `{"verdict": "pass"}`. Exit 1 with a refusal
/// is a refusal naming where the check stopped and never what it found; an `unavailable`
/// answer, any other exit, a missing exit and anything the schema refuses are unavailable.
#[must_use]
pub fn check_verdict(code: Option<i32>, stdout: &[u8]) -> WriteVerdict {
    let answer = validated(stdout, &VALIDATORS.check_output);
    let field = |name: &str| {
        answer
            .as_ref()
            .and_then(|answer| answer.get(name))
            .and_then(Value::as_str)
            .map(str::to_owned)
    };
    match (code, field("verdict").as_deref()) {
        (Some(0), Some("pass")) => WriteVerdict::Pass,
        (Some(1), Some("refuse")) => WriteVerdict::Refuse {
            reason: format!(
                "it carries a term of a private repository in its {}",
                field("surface").unwrap_or_else(|| "content".to_owned())
            ),
        },
        (_, Some("unavailable")) => WriteVerdict::Unavailable {
            reason: format!(
                "the check could not decide ({})",
                field("reason").unwrap_or_else(|| "no reason given".to_owned())
            ),
        },
        (code, _) => WriteVerdict::Unavailable {
            reason: format!(
                "the check command {} without a verdict this store believes",
                code.map_or_else(|| "was stopped".to_owned(), |code| format!("exited {code}"))
            ),
        },
    }
}

/// How the boundary stands for one engine: active or not, the policy it asks, and the term
/// scope its checks carry.
#[derive(Clone)]
pub(crate) struct Boundary {
    active: bool,
    policy: Arc<dyn WritePolicy>,
    scope: Option<Vec<String>>,
    /// The caller's replacement layer every sanitisation carries: the configuration's, then the
    /// command line's for one invocation.
    replacements: Vec<Replacement>,
    /// Whether sanitise-on-refusal writes nothing and only answers what it would change.
    sanitize_dry_run: bool,
    /// What the policy can do with a write.
    mode: PolicyMode,
    /// Whether a narrow write to a task is screened with the task as stored, and what that
    /// rewrites written back first: `write_policy.sanitize_stored_on_write`, off by default.
    sanitize_stored_on_write: bool,
    /// What the screened writes of this command replaced, until [`Engine::take_sanitization`].
    sanitized: Arc<std::sync::Mutex<Sanitization>>,
    /// What the policy has answered in this command, until [`Engine::end_command`].
    screened: Arc<std::sync::Mutex<Screened>>,
    /// One line per dependency edge a write left out because it names something private,
    /// until [`Engine::take_withheld_edges`] takes them.
    withheld: Arc<std::sync::Mutex<Vec<String>>>,
}

/// What a store's policy can do with a write.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PolicyMode {
    /// The configured commands check and rewrite nothing: every write is screened as before.
    Checks,
    /// The configuration names a `sanitize_command`, the one policy known to rewrite: a re-copy's
    /// origin search may ask it for a replaced spelling.
    SanitizeCommand,
    /// A linking caller's own policy, which may rewrite: nothing beyond a write's own call is
    /// asked of it.
    Linked,
}

/// What one command has learned from the policy, so it asks nothing twice: each repository's
/// visibility it answered, and every input a check passed whole.
///
/// Nothing here outlives the command — [`Engine::end_command`] empties it — and nothing is
/// kept that the policy did not affirmatively answer: a repository it could not answer is
/// asked again, and an unavailable or refused check passes nothing. A write is taken as passed
/// only when one input a check passed held every value it exposes, so values that passed apart
/// are never taken as having passed together, and an exposure that changes later in the command
/// is screened again.
#[derive(Default)]
struct Screened {
    visibility: std::collections::HashMap<Repository, RepositoryVisibility>,
    passed: Vec<PassedInput>,
}

/// The values of one input a check passed, by surface and by whether it was rewritable — the
/// input as sent, the caller's replacements included, since those are the store's for the
/// whole command — and what it rewrote.
#[derive(Default)]
struct PassedInput {
    surfaces: [std::collections::HashSet<String>; 7],
    rewrite: Rewrite,
}

impl PassedInput {
    fn lists(exposure: &Exposure) -> [Vec<String>; 7] {
        [
            exposure.text.clone(),
            exposure.paths.clone(),
            exposure.metadata.clone(),
            exposure.fixed.text.clone(),
            exposure.fixed.paths.clone(),
            exposure.fixed.metadata.clone(),
            exposure
                .locators
                .iter()
                .map(|locator| format!("{}\u{0}{}", locator.kind.name(), locator.value))
                .collect(),
        ]
    }

    fn of(exposure: &Exposure, rewrite: Rewrite) -> Self {
        Self {
            surfaces: Self::lists(exposure).map(|list| list.into_iter().collect()),
            rewrite,
        }
    }

    fn covers(&self, exposure: &Exposure) -> bool {
        Self::lists(exposure)
            .iter()
            .zip(&self.surfaces)
            .all(|(values, passed)| values.iter().all(|value| passed.contains(value)))
    }
}

/// What the screened writes of one command replaced, and what sanitise-on-refusal changed:
/// the `sanitization` member a write's answer carries when it is not empty.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, JsonSchema)]
pub struct Sanitization {
    /// Spans the writes' own sanitisations rewrote.
    pub replaced: u32,
    /// Each field or comment of an item sanitisation changed beyond the write itself.
    pub side_effect: Vec<SideEffect>,
    /// Whether nothing was written, the changes only reported.
    pub dry_run: bool,
}

/// A command's object response with the sanitization report its writes produced.
/// Empty reports are omitted, preserving the original answer when no field changed.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct WriteAnswer<T> {
    #[serde(flatten)]
    answer: T,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    sanitization: Option<Sanitization>,
}

impl<T> WriteAnswer<T> {
    /// Attach a nonempty report to an object response; an empty report leaves it unchanged.
    #[must_use]
    pub fn new(answer: T, sanitization: Sanitization) -> Self {
        Self {
            answer,
            sanitization: (!sanitization.is_empty()).then_some(sanitization),
        }
    }
}

impl Sanitization {
    /// Whether there is nothing to report, which leaves a write's answer as it was.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.replaced == 0 && self.side_effect.is_empty()
    }

    /// The `sanitization` member of a write's answer, as the `Sanitization` schema root
    /// describes it.
    #[must_use]
    pub fn to_json(&self) -> Value {
        serde_json::to_value(self).expect("a sanitisation serializes")
    }
}

/// What one engine's screened writes replaced and sanitised, shared with the engine.
#[derive(Debug, Clone)]
pub struct SanitizationLog(Arc<std::sync::Mutex<Sanitization>>);

impl SanitizationLog {
    /// Take everything recorded so far, leaving nothing.
    #[must_use]
    pub fn take(&self) -> Sanitization {
        self.0
            .lock()
            .map(|mut sanitized| std::mem::take(&mut *sanitized))
            .unwrap_or_default()
    }
}

/// One field or comment of an item a sanitisation changed, named by screened locators.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, JsonSchema)]
pub struct SideEffect {
    /// The item, by its qualified id as screened.
    pub item: Locator,
    /// The field — `content`, `title`, `label`, `metadata:<key>` — or `comment:<id>`.
    pub field: Locator,
}

/// What a write run with `--sanitize-dry-run` answers when it would have sanitised its item
/// first, having written nothing: whether the write would then pass, and what would change.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, JsonSchema)]
pub struct SanitizeDryRun {
    /// Whether the write would pass once its item was sanitised.
    pub would_pass: bool,
    /// What sanitising the item would change.
    pub sanitization: Sanitization,
}

impl Default for Boundary {
    fn default() -> Self {
        Self {
            active: false,
            policy: Arc::new(MissingWritePolicy),
            scope: None,
            replacements: Vec::new(),
            sanitize_dry_run: false,
            mode: PolicyMode::Checks,
            sanitize_stored_on_write: false,
            sanitized: Arc::default(),
            screened: Arc::default(),
            withheld: Arc::default(),
        }
    }
}

impl Boundary {
    /// The boundary a configuration asks for.
    pub(crate) fn configured(
        write_policy: Option<&WritePolicyConfig>,
        visibilities: &BTreeMap<onetaskgraph_plugin_api::SourceName, SourceVisibility>,
    ) -> Self {
        Self {
            active: write_policy.is_some()
                || visibilities
                    .values()
                    .any(|declared| *declared != SourceVisibility::Unknown),
            policy: match write_policy {
                Some(config) => Arc::new(CommandWritePolicy::new(config)),
                None => Arc::new(MissingWritePolicy),
            },
            scope: None,
            replacements: write_policy
                .map(|config| config.replacements.clone())
                .unwrap_or_default(),
            sanitize_dry_run: false,
            mode: if write_policy.is_some_and(|config| config.sanitize_command.is_some()) {
                PolicyMode::SanitizeCommand
            } else {
                PolicyMode::Checks
            },
            sanitize_stored_on_write: write_policy
                .is_some_and(|config| config.sanitize_stored_on_write),
            sanitized: Arc::default(),
            screened: Arc::default(),
            withheld: Arc::default(),
        }
    }

    /// Forget everything the policy answered: what [`Engine::end_command`] does, so the next
    /// command asks again.
    pub(crate) fn forget(&self) {
        if let Ok(mut screened) = self.screened.lock() {
            *screened = Screened::default();
        }
    }

    /// Note one edge a write left out, once however often it is shown.
    pub(crate) fn withheld(&self, line: String) {
        if let Ok(mut withheld) = self.withheld.lock()
            && !withheld.contains(&line)
        {
            withheld.push(line);
        }
    }

    /// What one input a check of this scope passed in this command rewrote, when it held every
    /// value of `exposure`.
    fn passed(&self, exposure: &Exposure) -> Option<Rewrite> {
        self.screened.lock().ok().and_then(|screened| {
            screened
                .passed
                .iter()
                .find(|input| input.covers(exposure))
                .map(|input| Rewrite {
                    replaced: 0,
                    ..input.rewrite.clone()
                })
        })
    }

    fn pass(&self, exposure: &Exposure, rewrite: &Rewrite) {
        if let Ok(mut screened) = self.screened.lock() {
            screened
                .passed
                .push(PassedInput::of(exposure, rewrite.clone()));
        }
        if rewrite.replaced > 0
            && let Ok(mut sanitized) = self.sanitized.lock()
        {
            sanitized.replaced = sanitized.replaced.saturating_add(rewrite.replaced);
        }
    }
}

/// What one write puts in front of a reader, by the surfaces a check reads: what replacement
/// may rewrite, what it never rewrites, and the identifiers a diagnostic of it would print.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Exposure {
    text: Vec<String>,
    paths: Vec<String>,
    metadata: Vec<String>,
    fixed: FixedEntries,
    locators: Vec<LocatorEntry>,
}

impl Exposure {
    /// Prose a write carries — a comment's body, a rendering — which replacement rewrites.
    pub(crate) fn text(text: impl Into<String>) -> Self {
        Self {
            text: vec![text.into()],
            ..Self::default()
        }
    }

    /// Short fields a write carries that replacement never rewrites — an id, a status, a
    /// priority, a key.
    pub(crate) fn metadata(values: impl IntoIterator<Item = String>) -> Self {
        Self {
            fixed: FixedEntries {
                metadata: values.into_iter().collect(),
                ..FixedEntries::default()
            },
            ..Self::default()
        }
    }

    /// Short fields a write carries that replacement rewrites — the id a create makes up from
    /// its source's.
    pub(crate) fn short(values: impl IntoIterator<Item = String>) -> Self {
        Self {
            metadata: values.into_iter().collect(),
            ..Self::default()
        }
    }

    pub(crate) fn and(mut self, other: Self) -> Self {
        self.text.extend(other.text);
        self.paths.extend(other.paths);
        self.metadata.extend(other.metadata);
        self.fixed.text.extend(other.fixed.text);
        self.fixed.paths.extend(other.fixed.paths);
        self.fixed.metadata.extend(other.fixed.metadata);
        for locator in other.locators {
            if !self.locators.contains(&locator) {
                self.locators.push(locator);
            }
        }
        self
    }

    /// The multiset union of `self` and `other`: every value either holds, each as many times
    /// as the one of the two holding it more often holds it, `self`'s order first.
    ///
    /// What a batch of writes is screened as: exactly the values each write's own screening
    /// would hold and nothing else, so two writes carrying the same text send it once. A check
    /// decides on whether a value is there, and a rewrite maps each value it changes, so neither
    /// reads the same value twice differently.
    ///
    /// Under a policy that only checks, rewriting nothing — [`PolicyMode::Checks`] — a value is
    /// counted over each surface as the check reads it, rewritable and fixed together as one
    /// list, so the batch's check is handed exactly the union of what each write's own would be.
    /// A policy that rewrites keeps the two apart, so a value one write may have rewritten never
    /// stands in for the same value another write holds where nothing rewrites it.
    fn union(mut self, other: Self, mode: PolicyMode) -> Self {
        let checked = mode == PolicyMode::Checks;
        type Tally = std::collections::HashMap<String, usize>;
        fn tally<'a>(lists: impl IntoIterator<Item = &'a [String]>) -> Tally {
            let mut tally = Tally::new();
            for value in lists.into_iter().flatten() {
                *tally.entry(value.clone()).or_default() += 1;
            }
            tally
        }
        fn joined(held: &mut Vec<String>, more: Vec<String>, counted: &Tally, seen: &mut Tally) {
            for value in more {
                let times = seen.entry(value.clone()).or_default();
                *times += 1;
                if *times > counted.get(&value).copied().unwrap_or(0) {
                    held.push(value);
                }
            }
        }
        for (held, fixed, more, fixed_more) in [
            (
                &mut self.text,
                &mut self.fixed.text,
                other.text,
                other.fixed.text,
            ),
            (
                &mut self.paths,
                &mut self.fixed.paths,
                other.paths,
                other.fixed.paths,
            ),
            (
                &mut self.metadata,
                &mut self.fixed.metadata,
                other.metadata,
                other.fixed.metadata,
            ),
        ] {
            if checked {
                let counted = tally([held.as_slice(), fixed.as_slice()]);
                let mut seen = Tally::new();
                joined(held, more, &counted, &mut seen);
                joined(fixed, fixed_more, &counted, &mut seen);
            } else {
                let counted = tally([held.as_slice()]);
                joined(held, more, &counted, &mut Tally::new());
                let counted = tally([fixed.as_slice()]);
                joined(fixed, fixed_more, &counted, &mut Tally::new());
            }
        }
        for locator in other.locators {
            if !self.locators.contains(&locator) {
                self.locators.push(locator);
            }
        }
        self
    }

    /// Paths a write names that replacement never rewrites — an image asset's name.
    pub(crate) fn paths(mut self, paths: impl IntoIterator<Item = String>) -> Self {
        self.fixed.paths.extend(paths);
        self
    }

    /// The identifiers a diagnostic of this write would print, in the order they are sent.
    pub(crate) fn locators(&self) -> &[LocatorEntry] {
        &self.locators
    }

    /// One identifier a diagnostic of this write would print, screened with it.
    pub(crate) fn locate(mut self, kind: LocatorKind, value: impl Into<String>) -> Self {
        let locator = LocatorEntry {
            kind,
            value: value.into(),
        };
        if !self.locators.contains(&locator) {
            self.locators.push(locator);
        }
        self
    }

    /// The names of the image assets a write stores, which are paths.
    pub(crate) fn assets(self, assets: &[onetaskgraph_plugin_api::AssetPayload]) -> Self {
        self.paths(assets.iter().map(|asset| asset.name.as_str().to_owned()))
    }

    /// The answers a rendering stores beside its content, where its source keeps them: each
    /// name fixed, each string of its value rewritable.
    pub(crate) fn answers(mut self, answers: Option<&BTreeMap<String, Value>>) -> Self {
        for (key, value) in answers.into_iter().flatten() {
            self.fixed.metadata.push(key.clone());
            strings(value, &mut self);
        }
        self
    }

    /// One metadata entry a write sets: its key, fixed, and every string in its value.
    pub(crate) fn of_entry(key: &str, value: &Value) -> Self {
        let mut exposure = Self::metadata([key.to_owned()]);
        strings(value, &mut exposure);
        exposure
    }

    /// Everything a targeted update writes.
    pub(crate) fn of_update(update: &onetaskgraph_plugin_api::TaskUpdate) -> Self {
        let mut exposure = Self {
            text: update.content.iter().cloned().collect(),
            ..Self::default()
        };
        exposure.metadata.extend(update.title.iter().cloned());
        exposure
            .fixed
            .metadata
            .extend(update.status.iter().map(|status| status.name.clone()));
        exposure.fixed.metadata.extend(
            update
                .priority
                .iter()
                .map(|priority| priority.as_str().to_owned()),
        );
        for (key, value) in &update.metadata_set {
            exposure = exposure.and(Self::of_entry(key.as_str(), value));
        }
        exposure
            .fixed
            .metadata
            .extend(update.delivers.iter().flatten().map(ToString::to_string));
        exposure.fixed.metadata.extend(
            update
                .depends_on
                .iter()
                .flatten()
                .map(|edge| edge.to.id().to_owned()),
        );
        exposure.fixed.metadata.extend(
            update
                .repositories
                .iter()
                .flatten()
                .map(|repository| repository.as_str().to_owned()),
        );
        exposure
            .fixed
            .metadata
            .extend(update.assignees.iter().flatten().cloned());
        exposure
    }

    /// Everything a task carries that a reader of it would read.
    pub(crate) fn of_task(task: &Task) -> Self {
        let mut exposure = Self::of_record(
            &task.title,
            task.content.as_deref(),
            &task.labels,
            &task.metadata,
            &task.repositories,
        );
        exposure.fixed.metadata.push(task.status.name.clone());
        exposure
            .fixed
            .metadata
            .push(task.priority.as_str().to_owned());
        exposure
            .fixed
            .metadata
            .extend(task.project.iter().map(|id| id.0.clone()));
        exposure.fixed.metadata.extend(
            task.delivers
                .iter()
                .chain(&task.delivered_by)
                .map(ToString::to_string),
        );
        // A person a write names is a fixed entry: replacement never renames anybody.
        exposure
            .fixed
            .metadata
            .extend(task.assignees.iter().flatten().cloned());
        exposure
    }

    /// Name a fixed metadata entry by the same layout `of_task` constructs.
    pub(super) fn task_fixed_field(task: &Task, mut index: usize) -> ChangedField {
        let labels = task
            .labels
            .iter()
            .filter(|label| label.id.as_str() != label.name)
            .count();
        if index < labels {
            return ChangedField::Label;
        }
        index -= labels;
        if index < task.repositories.len() {
            return ChangedField::Repositories;
        }
        index -= task.repositories.len();
        for (key, value) in &task.metadata {
            let count = Self::of_record(
                "",
                None,
                &[],
                &BTreeMap::from([(key.clone(), value.clone())]),
                &[],
            )
            .fixed
            .metadata
            .len();
            if index < count {
                return ChangedField::Metadata(key.clone());
            }
            index -= count;
        }
        match index {
            0 => ChangedField::Status,
            1 => ChangedField::Priority,
            _ => {
                index -= 2;
                if task.project.is_some() {
                    if index == 0 {
                        return ChangedField::Project;
                    }
                    index -= 1;
                }
                if index < task.delivers.len() {
                    return ChangedField::Delivers;
                }
                index -= task.delivers.len();
                if index < task.delivered_by.len() {
                    return ChangedField::DeliveredBy;
                }
                index -= task.delivered_by.len();
                if index < task.assignees.as_ref().map_or(0, Vec::len) {
                    return ChangedField::Assignees;
                }
                ChangedField::Id
            }
        }
    }

    /// Everything a project carries that a reader of it would read.
    pub(crate) fn of_project(project: &Project) -> Self {
        let mut exposure = Self::of_record(
            &project.title,
            project.content.as_deref(),
            &project.labels,
            &project.metadata,
            &project.repositories,
        );
        exposure.fixed.metadata.push(project.status.name.clone());
        exposure
    }

    /// Everything a document carries that a reader of it would read.
    pub(crate) fn of_document(document: &Document) -> Self {
        let mut exposure = Self::of_record(
            &document.title,
            document.content.as_deref(),
            &document.labels,
            &document.metadata,
            &document.repositories,
        );
        exposure
            .fixed
            .metadata
            .extend(document.project.iter().map(|id| id.0.clone()));
        exposure
    }

    fn of_record(
        title: &str,
        content: Option<&str>,
        labels: &[Label],
        metadata: &BTreeMap<String, Value>,
        repositories: &[Repository],
    ) -> Self {
        let mut exposure = Self {
            text: content.map(str::to_owned).into_iter().collect(),
            ..Self::default()
        };
        exposure.metadata.push(title.to_owned());
        exposure
            .metadata
            .extend(labels.iter().map(|label| label.name.clone()));
        // A label's id is written beside its name: one that spells the name is rewritten with
        // it, and any other is an id, which is never rewritten.
        for label in labels
            .iter()
            .filter(|label| label.id.as_str() != label.name)
        {
            exposure.fixed.metadata.push(label.id.as_str().to_owned());
        }
        exposure.fixed.metadata.extend(
            repositories
                .iter()
                .map(|repository| repository.as_str().to_owned()),
        );
        for (key, value) in metadata {
            exposure.fixed.metadata.push(key.clone());
            strings(value, &mut exposure);
        }
        exposure
    }

    /// The document a sanitisation of this exposure sends.
    fn input(
        &self,
        scope: Option<Vec<String>>,
        replacements: &[Replacement],
    ) -> PublicSanitizeInput {
        PublicSanitizeInput {
            destination: RepositoryVisibility::Public,
            text: self.text.clone(),
            paths: self.paths.clone(),
            metadata: self.metadata.clone(),
            fixed: self.fixed.clone(),
            scope,
            replacements: replacements.to_vec(),
            locators: self.locators.clone(),
        }
    }
}

/// Every string inside `value`, which replacement rewrites, and every key and number, which it
/// never does. A string that reads as a path — the template reference a rendering records, an
/// asset's place — is a path as well, so a check reading paths reads it there too.
fn strings(value: &Value, exposure: &mut Exposure) {
    match value {
        Value::String(text) => {
            if text.contains('/') || text.contains('\\') {
                exposure.paths.push(text.clone());
            }
            exposure.metadata.push(text.clone());
        }
        Value::Array(values) => values.iter().for_each(|value| strings(value, exposure)),
        Value::Object(fields) => {
            for (key, value) in fields {
                exposure.fixed.metadata.push(key.clone());
                strings(value, exposure);
            }
        }
        Value::Number(number) => exposure.fixed.metadata.push(number.to_string()),
        Value::Bool(_) | Value::Null => {}
    }
}

/// What a passing sanitisation rewrote: each rewritable value it changed, by the surface it was
/// sent on, and how many spans. Applied to what a write sends, so what is written is exactly
/// what the policy passed; an identity rewrite changes nothing.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Rewrite {
    text: std::collections::HashMap<String, String>,
    paths: std::collections::HashMap<String, String>,
    metadata: std::collections::HashMap<String, String>,
    replaced: u32,
}

impl Rewrite {
    pub(super) fn of(exposure: &Exposure, answer: &SanitizeVerdict) -> Self {
        let changed = |sent: &[String], answered: &[String]| {
            sent.iter()
                .zip(answered)
                .filter(|(sent, answered)| sent != answered)
                .map(|(sent, answered)| (sent.clone(), answered.clone()))
                .collect()
        };
        Self {
            text: changed(&exposure.text, &answer.text),
            paths: changed(&exposure.paths, &answer.paths),
            metadata: changed(&exposure.metadata, &answer.metadata),
            replaced: answer.replaced,
        }
    }

    /// Whether it rewrites nothing.
    pub(crate) fn is_identity(&self) -> bool {
        self.text.is_empty() && self.paths.is_empty() && self.metadata.is_empty()
    }

    /// Prose as it is written.
    pub(crate) fn text(&self, value: &str) -> String {
        self.text
            .get(value)
            .cloned()
            .unwrap_or_else(|| value.to_owned())
    }

    /// A short field as it is written: as rewritten among the short fields, or — for one that
    /// reads as a path and was rewritten only there — as rewritten among the paths.
    pub(crate) fn short(&self, value: &str) -> String {
        self.metadata
            .get(value)
            .or_else(|| self.paths.get(value))
            .cloned()
            .unwrap_or_else(|| value.to_owned())
    }

    /// Every string inside `value` as it is written; keys and numbers as they were.
    pub(crate) fn value(&self, value: &mut Value) {
        match value {
            Value::String(text) => *text = self.short(text),
            Value::Array(values) => values.iter_mut().for_each(|value| self.value(value)),
            Value::Object(fields) => fields.values_mut().for_each(|value| self.value(value)),
            Value::Number(_) | Value::Bool(_) | Value::Null => {}
        }
    }

    /// Every metadata value as it is written.
    pub(crate) fn metadata_values(&self, metadata: &mut BTreeMap<String, Value>) {
        metadata.values_mut().for_each(|value| self.value(value));
    }

    fn record(
        &self,
        title: &mut String,
        content: &mut Option<String>,
        labels: &mut [Label],
        metadata: &mut BTreeMap<String, Value>,
    ) {
        *title = self.short(title);
        if let Some(content) = content {
            *content = self.text(content);
        }
        for label in labels {
            if label.id.as_str() == label.name {
                label.id = NativeId::from(self.short(label.id.as_str()).as_str());
            }
            label.name = self.short(&label.name);
        }
        self.metadata_values(metadata);
    }

    /// A task as it is written.
    pub(crate) fn task(&self, task: &mut Task) {
        if self.is_identity() {
            return;
        }
        self.record(
            &mut task.title,
            &mut task.content,
            &mut task.labels,
            &mut task.metadata,
        );
    }

    /// A project as it is written.
    pub(crate) fn project(&self, project: &mut Project) {
        if self.is_identity() {
            return;
        }
        self.record(
            &mut project.title,
            &mut project.content,
            &mut project.labels,
            &mut project.metadata,
        );
    }

    /// A document as it is written.
    pub(crate) fn document(&self, document: &mut Document) {
        if self.is_identity() {
            return;
        }
        self.record(
            &mut document.title,
            &mut document.content,
            &mut document.labels,
            &mut document.metadata,
        );
    }

    /// A targeted update as it is written.
    pub(crate) fn update(&self, update: &mut onetaskgraph_plugin_api::TaskUpdate) {
        if self.is_identity() {
            return;
        }
        if let Some(title) = &mut update.title {
            *title = self.short(title);
        }
        if let Some(content) = &mut update.content {
            *content = self.text(content);
        }
        update
            .metadata_set
            .values_mut()
            .for_each(|value| self.value(value));
    }

    /// A rendering's answers as they are written.
    pub(crate) fn answers(&self, answers: &mut BTreeMap<String, Value>) {
        answers.values_mut().for_each(|value| self.value(value));
    }
}

/// One write, as the boundary is asked about it.
pub(crate) struct Outbound<'a> {
    /// How the write names the item, for a refusal: a qualified id, or "the new task".
    pub(crate) item: String,
    /// The item's classification, already tightened by its repositories — and, for a document
    /// or a project, by what it inherits. A task's is its own.
    pub(crate) classification: Classification,
    /// Where in the destination it lands.
    pub(crate) target: WriteTarget<'a>,
    /// What it puts there.
    pub(crate) exposure: &'a Exposure,
}

impl Engine {
    /// The same engine, asking `policy` about the public boundary — which makes the boundary
    /// active, whatever the configuration says. What an earlier policy answered is forgotten,
    /// so none of it stands in for this one's answer.
    #[must_use]
    pub fn with_write_policy(mut self, policy: Arc<dyn WritePolicy>) -> Self {
        self.boundary.active = true;
        self.boundary.policy = policy;
        self.boundary.mode = PolicyMode::Linked;
        self.boundary.forget();
        self
    }

    /// The same engine, deriving every check's terms from `scope`: `None` for every private
    /// identity the policy knows of, an empty list for none. A check passed under an earlier
    /// scope is forgotten, because it derived other terms.
    #[must_use]
    pub fn with_term_scope(mut self, scope: Option<Vec<String>>) -> Self {
        self.boundary.scope = scope;
        self.boundary.forget();
        self
    }

    /// The same engine, sending `replacements` after the configuration's own with every
    /// sanitisation — the command line's `--replace` and `--reject` for one invocation. A later
    /// entry wins. What was passed under the earlier list is forgotten.
    #[must_use]
    pub fn with_replacements(mut self, replacements: Vec<Replacement>) -> Self {
        self.boundary.replacements.extend(replacements);
        self.boundary.forget();
        self
    }

    /// The same engine, screening a narrow write to a task together with the task and its
    /// comments as stored and writing back what that rewrites before the write is made:
    /// `write_policy.sanitize_stored_on_write`, for a linking caller's own policy. Off, a narrow
    /// write screens only what it writes, and a term already stored is `sources sanitize`'s.
    #[must_use]
    pub fn with_sanitize_stored_on_write(mut self, enabled: bool) -> Self {
        self.boundary.sanitize_stored_on_write = enabled;
        self
    }

    /// The same engine, writing nothing when a write is refused for a term already on its
    /// item, and answering what sanitising that item would change instead. Asking this is
    /// asking what is stored, so a narrow write under a policy that can rewrite reads the task
    /// and its comments whether or not `sanitize_stored_on_write` is on.
    #[must_use]
    pub fn with_sanitize_dry_run(mut self, dry_run: bool) -> Self {
        self.boundary.sanitize_dry_run = dry_run;
        self
    }

    /// `origin` as a copy into `destination` writes it, when that is another spelling: asked of
    /// a configured `sanitize_command` only, only when `destination` is screened and only for an
    /// origin a copy writes there — never one naming a source declared private, which is
    /// withheld — and `None` whenever the answer is the origin as it stands or did not pass.
    pub(crate) async fn origin_as_written(
        &self,
        destination: &ResolvedSource,
        origin: &crate::GlobalId,
    ) -> Option<String> {
        if !self.boundary.active
            || self.boundary.mode != PolicyMode::SanitizeCommand
            || self.declared(destination.name()) == SourceVisibility::Private
            || self.declared(&origin.source) == SourceVisibility::Private
        {
            return None;
        }
        let origin = origin.to_string();
        let origin = origin.as_str();
        let exposure = Exposure::short([origin.to_owned()]);
        let answer = self.screen(&exposure).await;
        if answer.verdict != WriteVerdict::Pass {
            return None;
        }
        let written = Rewrite::of(&exposure, &answer).short(origin);
        (written != origin).then_some(written)
    }

    /// The policy this engine asks about the public boundary.
    pub(crate) fn boundary_policy(&self) -> Arc<dyn WritePolicy> {
        Arc::clone(&self.boundary.policy)
    }

    /// The term scope every sanitisation carries.
    pub(crate) fn term_scope(&self) -> Option<Vec<String>> {
        self.boundary.scope.clone()
    }

    /// The caller's replacement layer every sanitisation carries, in order.
    #[must_use]
    pub fn replacements(&self) -> &[Replacement] {
        &self.boundary.replacements
    }

    /// Take what the screened writes since the last take replaced and sanitised — what a write
    /// command answers as its `sanitization` member, left out when [`Sanitization::is_empty`].
    #[must_use]
    pub fn take_sanitization(&self) -> Sanitization {
        self.sanitization_log().take()
    }

    /// A handle on what this engine's screened writes replaced and sanitised, which outlives a
    /// borrow of the engine: what the command line reads once a verb has answered.
    #[must_use]
    pub fn sanitization_log(&self) -> SanitizationLog {
        SanitizationLog(Arc::clone(&self.boundary.sanitized))
    }

    /// Take the edges a copy left out of what it wrote because each names something private —
    /// an item of a source declared private, or one classified private, whose source named no
    /// tracker an opaque reference could point into — one line each, naming the item, the far
    /// end and why, in the order they were left out. What the command line prints on standard
    /// error after a copy; nothing is left to take afterwards.
    #[must_use]
    pub fn take_withheld_edges(&self) -> Vec<String> {
        self.boundary
            .withheld
            .lock()
            .map(|mut withheld| std::mem::take(&mut *withheld))
            .unwrap_or_default()
    }

    /// Whether the boundary is active for this engine (see the module documentation).
    #[must_use]
    pub fn boundary_active(&self) -> bool {
        self.boundary.active
    }

    /// Refuse a write to `destination` that names, at the caller's own request, a private
    /// item — a `--depends-on` or a `--delivers` of a create or an update — when `destination`
    /// is not itself declared private.
    ///
    /// Such a reference is a qualified id, `<source>:<id>`, and the source's name is in no
    /// term list unless it happens to match a private repository: so it is refused here,
    /// whatever the check would say, rather than written where it names somewhere private.
    /// An item of a source declared private is refused by its source alone; while the boundary
    /// is active, every other item of another source is read for its classification with what
    /// it inherits (see [`Engine::reference_class`]), refused when private and when that cannot
    /// be established. A copy and a delivered task's back-reference withhold the same ids
    /// instead (see [`Engine::withholds`]), because there the id is what the engine carries
    /// along rather than what the caller asked to be written. One naming `destination` itself
    /// is that source's own item and names nothing elsewhere.
    pub(crate) async fn refuse_private_references(
        &self,
        destination: &onetaskgraph_plugin_api::SourceName,
        item: &str,
        references: &[(crate::GlobalId, onetaskgraph_plugin_api::ItemKind)],
    ) -> Result<(), EngineError> {
        if self.declared(destination) == SourceVisibility::Private {
            return Ok(());
        }
        let mut known = std::collections::HashMap::new();
        for (reference, kind) in references {
            if &reference.source == destination {
                continue;
            }
            if self.declared(&reference.source) == SourceVisibility::Private {
                return Err(EngineError::PrivateReference {
                    item: item.to_owned(),
                    destination: destination.to_string(),
                    named: reference.source.to_string(),
                });
            }
            if !self.boundary.active {
                continue;
            }
            match self.reference_class(reference, *kind, &mut known).await {
                Ok(Classification::Private) => {
                    return Err(EngineError::PrivateItemReference {
                        item: item.to_owned(),
                        destination: destination.to_string(),
                        reference: reference.to_string(),
                    });
                }
                Ok(Classification::Public) => {}
                Err(why) => {
                    return Err(EngineError::ReferenceUnclassified {
                        item: item.to_owned(),
                        reference: reference.to_string(),
                        destination: destination.to_string(),
                        why,
                    });
                }
            }
        }
        Ok(())
    }

    /// `edges`, as a write to `destination` at the caller's own request holds them: each far
    /// end naming a private item rewritten to the opaque form — its source's designator and its
    /// native id — and every other refused before anything is written.
    ///
    /// A far end is private when its source is declared private, or — while the boundary is
    /// active — when it reads as classified private (see [`Engine::reference_class`]). One
    /// whose source has a designator is written opaque, once the id it names is confirmed as the
    /// item's own native id by one read of it there: a prefixed human key, a URL or a slug would
    /// carry what the item is called, and is refused with
    /// [`EngineError::PrivateReferenceContent`] naming the form and never its value. One whose
    /// source has no designator is refused as [`refuse_private_references`] refuses it. An end
    /// given opaque already is held to the same test at the source its designator resolves to,
    /// and refused when none does. One in `destination` itself and every one written somewhere
    /// declared private are held as given.
    ///
    /// [`refuse_private_references`]: Self::refuse_private_references
    pub(crate) async fn publicly_named(
        &self,
        destination: &onetaskgraph_plugin_api::SourceName,
        item: &str,
        edges: Vec<onetaskgraph_plugin_api::DependencyEdge>,
    ) -> Result<Vec<onetaskgraph_plugin_api::DependencyEdge>, EngineError> {
        if self.declared(destination) == SourceVisibility::Private {
            return Ok(edges);
        }
        let mut known = std::collections::HashMap::new();
        let mut held = Vec::with_capacity(edges.len());
        for mut edge in edges {
            if edge.to.is_native() {
                held.push(edge);
                continue;
            }
            // Given opaque already, it is held to the same test as one this write makes opaque:
            // its id the native id of an item the tracker its designator names really holds.
            if let Some(designator) = edge.to.designator().cloned() {
                let (found, _) = self.designated(&designator).await;
                let Some(source) = found else {
                    return Err(EngineError::PrivateReferenceContent {
                        item: item.to_owned(),
                        destination: destination.to_string(),
                        form: ReferenceForm::Unconfirmed,
                    });
                };
                let far = crate::GlobalId::new(
                    source,
                    onetaskgraph_plugin_api::NativeId(edge.to.id().to_owned()),
                );
                self.native_or_refused(destination, item, &far, edge.to.kind)
                    .await?;
                held.push(edge);
                continue;
            }
            let far: crate::GlobalId =
                edge.to
                    .id()
                    .parse()
                    .map_err(|_| EngineError::ReferenceUnclassified {
                        item: item.to_owned(),
                        reference: edge.to.id().to_owned(),
                        destination: destination.to_string(),
                        why: "it is not a qualified id".to_owned(),
                    })?;
            if &far.source == destination {
                held.push(edge);
                continue;
            }
            let declared = self.declared(&far.source) == SourceVisibility::Private;
            let private = declared
                || (self.boundary.active
                    && match self.reference_class(&far, edge.to.kind, &mut known).await {
                        Ok(class) => class == Classification::Private,
                        Err(why) => {
                            return Err(EngineError::ReferenceUnclassified {
                                item: item.to_owned(),
                                reference: far.to_string(),
                                destination: destination.to_string(),
                                why,
                            });
                        }
                    });
            if !private {
                held.push(edge);
                continue;
            }
            let designator = self.designator(&far.source).await.map_err(|error| {
                EngineError::ReferenceUnclassified {
                    item: item.to_owned(),
                    reference: far.to_string(),
                    destination: destination.to_string(),
                    why: format!("its source could not say which tracker it is: {error}"),
                }
            })?;
            let Some(designator) = designator else {
                return Err(if declared {
                    EngineError::PrivateReference {
                        item: item.to_owned(),
                        destination: destination.to_string(),
                        named: far.source.to_string(),
                    }
                } else {
                    EngineError::PrivateItemReference {
                        item: item.to_owned(),
                        destination: destination.to_string(),
                        reference: far.to_string(),
                    }
                });
            };
            self.native_or_refused(destination, item, &far, edge.to.kind)
                .await?;
            edge.to = onetaskgraph_plugin_api::DependencyEndpoint::opaque(
                designator,
                far.native.0,
                edge.to.kind,
            )
            .expect("a qualified id names a native id that is not empty");
            held.push(edge);
        }
        Ok(held)
    }

    /// Refuse `far` unless it names a private item by its own native id, naming the form
    /// [`native_form`](Self::native_form) found instead.
    async fn native_or_refused(
        &self,
        destination: &onetaskgraph_plugin_api::SourceName,
        item: &str,
        far: &crate::GlobalId,
        kind: onetaskgraph_plugin_api::ItemKind,
    ) -> Result<(), EngineError> {
        match self.native_form(far, kind).await? {
            None => Ok(()),
            Some(form) => Err(EngineError::PrivateReferenceContent {
                item: item.to_owned(),
                destination: destination.to_string(),
                form,
            }),
        }
    }

    /// `None` when `far` names a private item by its own native id, and otherwise the form it
    /// names it by instead: a URL and a prefixed human key by their form alone, and anything
    /// else unless one read of it at its source answers an item whose id is exactly that.
    ///
    /// # Errors
    ///
    /// A source that is not configured, or that failed the read.
    pub(crate) async fn native_form(
        &self,
        far: &crate::GlobalId,
        kind: onetaskgraph_plugin_api::ItemKind,
    ) -> Result<Option<ReferenceForm>, EngineError> {
        let id = far.native.as_str();
        if let Some(form) = plainly_not_native(id) {
            return Ok(Some(form));
        }
        let failed = |error| EngineError::SourceFailed {
            name: far.source.to_string(),
            error,
        };
        let source = self.readable(&far.source)?;
        let read = match kind {
            onetaskgraph_plugin_api::ItemKind::Task => source
                .source()
                .get_task(&far.native)
                .await
                .map_err(failed)?
                .map(|task| task.id),
            onetaskgraph_plugin_api::ItemKind::Project => source
                .source()
                .get_project(&far.native)
                .await
                .map_err(failed)?
                .map(|project| project.id),
        };
        Ok(match read {
            Some(native) if native == far.native => None,
            _ if slug(id) => Some(ReferenceForm::Slug),
            _ => Some(ReferenceForm::NotNative),
        })
    }

    /// Whether a write to `destination` withholds an id naming `far`: an item of another source
    /// declared private, or one of the items in `private` — those this command knows are
    /// classified private — while `destination` is not itself declared private.
    ///
    /// A withheld id is left out of what that write carries — a dependency edge's far end, a
    /// `delivers` entry a copy carries along, a delivered task's `delivered_by` — and stays
    /// in the private store's own record, exactly as an `onetaskgraph.origin` or an
    /// `onetaskgraph.copies` link naming a private source does. Nothing is withheld while no
    /// source is declared private and no item is classified private.
    pub(crate) fn withholds(
        &self,
        destination: &onetaskgraph_plugin_api::SourceName,
        far: &crate::GlobalId,
        private: &[crate::GlobalId],
    ) -> bool {
        &far.source != destination
            && self.declared(destination) != SourceVisibility::Private
            && (self.declared(&far.source) == SourceVisibility::Private || private.contains(far))
    }

    /// Who `source` is declared readable by.
    pub(crate) fn declared(
        &self,
        source: &onetaskgraph_plugin_api::SourceName,
    ) -> SourceVisibility {
        self.routes.visibility(source)
    }

    /// `declared`, tightened by `repositories` — consulted only while the boundary is active,
    /// and any repository the policy does not answer `public` for makes it private.
    pub(crate) async fn classify(
        &self,
        declared: Classification,
        repositories: &[Repository],
    ) -> Classification {
        if !self.boundary.active || declared == Classification::Private {
            return declared;
        }
        for repository in repositories {
            let known = self
                .boundary
                .screened
                .lock()
                .ok()
                .and_then(|screened| screened.visibility.get(repository).copied());
            let visibility = match known {
                Some(visibility) => visibility,
                None => match self.boundary.policy.repository_visibility(repository).await {
                    Ok(visibility) => {
                        if let Ok(mut screened) = self.boundary.screened.lock() {
                            screened.visibility.insert(repository.clone(), visibility);
                        }
                        visibility
                    }
                    // Never held: a repository the policy could not answer is asked again.
                    Err(_) => RepositoryVisibility::Unknown,
                },
            };
            if !visibility.is_public() {
                return Classification::Private;
            }
        }
        declared
    }

    /// Admit one write to `destination`, or refuse it before anything is mutated:
    /// [`preflight`](Self::preflight), then [`at_write`](Self::at_write).
    pub(crate) async fn admit(
        &self,
        destination: &ResolvedSource,
        outbound: &Outbound<'_>,
    ) -> Result<Rewrite, EngineError> {
        let rewrite = self.preflight(destination, outbound).await?;
        self.at_write(destination, &outbound.item, &outbound.target)
            .await?;
        Ok(rewrite)
    }

    /// Everything about one write that can be settled before anything is written: a private
    /// item refused where it is not declared private, and — while the boundary is active — a
    /// write to a destination not declared private put to the caller's check.
    ///
    /// A destination declared private is not checked for terms: its reality is held to its
    /// declaration at the write itself ([`at_write`](Self::at_write)), and a write there that
    /// finds it not private is refused there.
    pub(crate) async fn preflight(
        &self,
        destination: &ResolvedSource,
        outbound: &Outbound<'_>,
    ) -> Result<Rewrite, EngineError> {
        if self.placed(destination, outbound)? {
            return self
                .check(destination, &outbound.item, outbound.exposure)
                .await;
        }
        Ok(Rewrite::default())
    }

    /// [`preflight`](Self::preflight)'s half that asks nothing: a private item refused where it
    /// is not declared private, and whether the write still has to be put to the check.
    pub(crate) fn placed(
        &self,
        destination: &ResolvedSource,
        outbound: &Outbound<'_>,
    ) -> Result<bool, EngineError> {
        let name = destination.name();
        self.refuse_private_landing(name, &outbound.item, outbound.classification)?;
        Ok(self.declared(name) != SourceVisibility::Private && self.boundary.active)
    }

    /// Refuse an item of `classification`, named `item`, landing in `destination` when it is
    /// private and `destination` is not declared private — the half of
    /// [`placed`](Self::placed) that a write and `task locate` both make, asking nothing.
    pub(crate) fn refuse_private_landing(
        &self,
        destination: &onetaskgraph_plugin_api::SourceName,
        item: &str,
        classification: Classification,
    ) -> Result<(), EngineError> {
        let declared = self.declared(destination);
        if classification == Classification::Private && declared != SourceVisibility::Private {
            return Err(EngineError::NotPrivateDestination {
                item: item.to_owned(),
                destination: destination.to_string(),
                declared,
            });
        }
        Ok(())
    }

    /// Put every write of `writes` — each its destination, how it names its item, and what it
    /// exposes — to the caller's policy before any of them is made: one sanitisation of all of
    /// them together — a batch, each list the multiset union of theirs (see
    /// [`Exposure::union`]) under the one scope they share — answering what each value is
    /// written as. A copy of one item is a batch of one. A union one earlier sanitisation of this
    /// command passed whole is not sent again.
    ///
    /// A batch the policy refuses, or cannot decide, refuses the whole copy on that one answer:
    /// the policy is asked nothing more, so a copy starts one check however it ends, and the
    /// refusal names the copy and its first destination as the policy screened them, never which
    /// write or what of it the policy stopped at.
    pub(crate) async fn check_all(
        &self,
        writes: &[(&ResolvedSource, String, Exposure)],
    ) -> Result<Rewrite, EngineError> {
        let Some((first, _, _)) = writes.first() else {
            return Ok(Rewrite::default());
        };
        if let [(destination, item, exposure)] = writes {
            return self.check(destination, item, exposure).await;
        }
        let mut union = Exposure::default();
        for (destination, item, exposure) in writes {
            union = union.union(
                located(destination, item, exposure.clone()),
                self.boundary.mode,
            );
        }
        if let Some(rewrite) = self.boundary.passed(&union) {
            return Ok(rewrite);
        }
        let answer = self.screen(&union).await;
        if answer.verdict == WriteVerdict::Pass {
            let rewrite = Rewrite::of(&union, &answer);
            self.boundary.pass(&union, &rewrite);
            return Ok(rewrite);
        }
        Err(refusal(
            first,
            &format!("this copy's {} writes", writes.len()),
            &union,
            &answer,
        ))
    }

    /// The check every write makes before its first mutation: a destination declared private
    /// is read live — outside a copy, at every write, because nothing read for an earlier
    /// write says what the backend is now; inside one, once per destination for the whole
    /// copy, its preflight's read held for every write of it, so a destination whose
    /// visibility changes during a copy is seen by the next.
    pub(crate) async fn at_write(
        &self,
        destination: &ResolvedSource,
        item: &str,
        target: &WriteTarget<'_>,
    ) -> Result<(), EngineError> {
        if self.declared(destination.name()) != SourceVisibility::Private {
            return Ok(());
        }
        self.verify_private(destination, item, target).await
    }

    /// Hold a source declared private to its backend's live answer.
    async fn verify_private(
        &self,
        destination: &ResolvedSource,
        item: &str,
        target: &WriteTarget<'_>,
    ) -> Result<(), EngineError> {
        self.verify_private_with_project(destination, item, target, None)
            .await
    }

    pub(super) async fn verify_private_with_project(
        &self,
        destination: &ResolvedSource,
        item: &str,
        target: &WriteTarget<'_>,
        project: Option<&WriteTarget<'_>>,
    ) -> Result<(), EngineError> {
        if self.declared(destination.name()) != SourceVisibility::Private {
            return Ok(());
        }
        let visibility = match project {
            Some(project) => {
                destination
                    .source()
                    .visibility_with_project(target, project)
                    .await
            }
            None => destination.source().visibility(target).await,
        };
        match visibility {
            Ok(Visibility::Private) => Ok(()),
            Ok(reality) => Err(EngineError::DestinationNotPrivate {
                item: item.to_owned(),
                destination: destination.name().to_string(),
                reality,
            }),
            Err(error) => Err(EngineError::VisibilityUnreadable {
                item: item.to_owned(),
                destination: destination.name().to_string(),
                error,
            }),
        }
    }

    /// Put one write to a destination not verified private to the caller's policy, answering
    /// what each value it exposes is written as.
    ///
    /// A write every value of whose exposure a sanitisation already passed in this command is
    /// not sent again.
    pub(crate) async fn check(
        &self,
        destination: &ResolvedSource,
        item: &str,
        exposure: &Exposure,
    ) -> Result<Rewrite, EngineError> {
        let exposure = located(destination, item, exposure.clone());
        if let Some(rewrite) = self.boundary.passed(&exposure) {
            return Ok(rewrite);
        }
        let answer = self.screen(&exposure).await;
        if answer.verdict == WriteVerdict::Pass {
            let rewrite = Rewrite::of(&exposure, &answer);
            self.boundary.pass(&exposure, &rewrite);
            return Ok(rewrite);
        }
        Err(refusal(destination, item, &exposure, &answer))
    }

    /// What the caller's policy says of `exposure`, one it could not be asked unavailable.
    pub(crate) async fn screen(&self, exposure: &Exposure) -> SanitizeVerdict {
        let input = exposure.input(self.boundary.scope.clone(), &self.boundary.replacements);
        let answer = self
            .boundary
            .policy
            .sanitize_public_write(&input)
            .await
            .unwrap_or_else(|error| unavailable(error.message));
        match answer_problem(&input, &answer) {
            Some(problem) => unavailable(problem),
            // An undecided answer screened nothing: whatever locators it carries are never
            // printed, and a refusal of it redacts them all.
            None if matches!(answer.verdict, WriteVerdict::Unavailable { .. }) => SanitizeVerdict {
                locators: Vec::new(),
                ..answer
            },
            None => answer,
        }
    }

    /// `classification`, held against the project `project` of `source` it is filed under —
    /// refused when it is private and the project is held public, because a public project
    /// never holds a private member — and, for a document (`held` [`Held::Document`]),
    /// tightened by that project.
    ///
    /// **A task is never tightened by its project**: who may read a task is its own
    /// declaration and its own repositories, so a public task stays public under a private
    /// project and makes that project private by nothing. A document is the project's own
    /// prose, and is as private as the project it is filed under.
    ///
    /// The project is read always while the boundary is active, and for a private item while it
    /// is not, which is refused under a public project either way. A public item in an inactive
    /// store inherits nothing, although its project may be private by hand: reading the project
    /// would be a request a store that has not opted in never made.
    ///
    /// While the boundary is active and `source` is not declared private, what the project is
    /// is everything it holds — its record, its repositories, its tasks and its documents. A
    /// project `source` does not hold, or cannot read, is [`EngineError::ProjectUnclassified`]
    /// for an item `Absent::Unclassifies`, which is a create naming it; a copy carries a
    /// project id the destination holds no counterpart of as an opaque value, its item already
    /// classified at its source with that project, so `Absent::Carried` inherits nothing from
    /// it. A source declared private takes the item whatever the project is, so it is spared
    /// that reading.
    pub(crate) async fn filed_under(
        &self,
        source: &ResolvedSource,
        project: &NativeId,
        item: &str,
        classification: Classification,
        held: Held,
        absent: Absent,
    ) -> Result<Classification, EngineError> {
        if !self.boundary.active && classification.is_public() {
            return Ok(classification);
        }
        let inherited = |filed: Classification| {
            if held == Held::Task {
                classification
            } else {
                classification.strictest(filed)
            }
        };
        if self.boundary.active && self.declared(source.name()) != SourceVisibility::Private {
            let filed = match self
                .source_project_class(
                    &crate::GlobalId::new(source.name().clone(), project.clone()),
                    &mut std::collections::HashMap::new(),
                )
                .await
            {
                Err(EngineError::ProjectUnclassified { .. }) if absent == Absent::Carried => {
                    return Ok(classification);
                }
                other => other?,
            };
            if classification == Classification::Private && filed.is_public() {
                return Err(EngineError::PrivateMemberOfPublicProject {
                    item: item.to_owned(),
                    project: format!("{}:{}", source.name(), project),
                });
            }
            return Ok(inherited(filed));
        }
        let Some(record) = source
            .source()
            .get_project(project)
            .await
            .map_err(|error| EngineError::SourceRefused {
                name: source.name().to_string(),
                error,
            })?
        else {
            return Ok(classification);
        };
        let filed = self
            .classify(record.classification, &record.repositories)
            .await;
        if classification == Classification::Private && filed.is_public() {
            return Err(EngineError::PrivateMemberOfPublicProject {
                item: item.to_owned(),
                project: format!("{}:{}", source.name(), project),
            });
        }
        Ok(inherited(filed))
    }

    /// Admit one narrow write to an item `destination` already holds.
    ///
    /// A destination verified private takes it with no read beyond its reality. Any other —
    /// while the boundary is active — has the item read once for its classification, refuses
    /// a private one, and puts what the write adds to the check; a term already stored on the
    /// item is left for `sources sanitize`, unless the store opted in to
    /// `sanitize_stored_on_write`. An inactive boundary reads nothing and admits it.
    pub(crate) async fn admit_existing(
        &self,
        destination: &ResolvedSource,
        item: &str,
        id: &NativeId,
        held: Held,
        exposure: &Exposure,
    ) -> Result<Rewrite, EngineError> {
        let target = WriteTarget::Existing(id);
        if self.declared(destination.name()) == SourceVisibility::Private {
            self.verify_private(destination, item, &target).await?;
            return Ok(Rewrite::default());
        }
        if !self.boundary.active {
            return Ok(Rewrite::default());
        }
        if held == Held::Task && self.can_rewrite() {
            let task = destination.source().get_task(id).await.map_err(|error| {
                EngineError::SourceRefused {
                    name: destination.name().to_string(),
                    error,
                }
            })?;
            let Some(task) = task else {
                return Ok(Rewrite::default());
            };
            let task = checked_task_identity(destination, id, task)?;
            if self.sanitizes_whole_items() {
                return self
                    .admit_whole_task(destination, item, id, task, exposure)
                    .await;
            }
            self.refuse_stored_private(destination, item, &task).await?;
            return self
                .admit_known(
                    destination,
                    item,
                    id,
                    task.classification,
                    &task.repositories,
                    exposure,
                )
                .await;
        }
        let stored = match held {
            Held::Task => destination.source().get_task(id).await.map(|task| {
                task.map(|task| (task.classification, task.repositories, task.project))
            }),
            Held::Project => destination.source().get_project(id).await.map(|project| {
                project.map(|project| (project.classification, project.repositories, None))
            }),
            Held::Document => destination.source().get_document(id).await.map(|document| {
                document.map(|document| {
                    (
                        document.classification,
                        document.repositories,
                        document.project,
                    )
                })
            }),
        }
        .map_err(|error| EngineError::SourceRefused {
            name: destination.name().to_string(),
            error,
        })?;
        self.admit_stored(destination, item, id, held, stored, exposure)
            .await
    }

    /// [`admit_existing`](Self::admit_existing), for an item this command has just read
    /// itself: `stored` is its classification, repositories and project as that read answered
    /// them, so the item is not read again. What it inherits is read as there.
    pub(crate) async fn admit_read(
        &self,
        destination: &ResolvedSource,
        item: &str,
        id: &NativeId,
        held: Held,
        stored: (Classification, Vec<Repository>, Option<NativeId>),
        exposure: &Exposure,
    ) -> Result<Rewrite, EngineError> {
        if self.declared(destination.name()) == SourceVisibility::Private {
            self.verify_private(destination, item, &WriteTarget::Existing(id))
                .await?;
            return Ok(Rewrite::default());
        }
        if !self.boundary.active {
            return Ok(Rewrite::default());
        }
        self.admit_stored(destination, item, id, held, Some(stored), exposure)
            .await
    }

    async fn admit_stored(
        &self,
        destination: &ResolvedSource,
        item: &str,
        id: &NativeId,
        held: Held,
        stored: Option<(Classification, Vec<Repository>, Option<NativeId>)>,
        exposure: &Exposure,
    ) -> Result<Rewrite, EngineError> {
        // An item the source does not hold is the write's own refusal to make, in its own
        // words; nothing of it can be private.
        let Some((declared, repositories, filed)) = stored else {
            return Ok(Rewrite::default());
        };
        // What it inherits tightens it too: a document's project, and a project's own tasks
        // and documents. A task inherits nothing: it is as private as its own record says.
        let mut known = std::collections::HashMap::new();
        let inherited = match (held, filed) {
            (Held::Task, _) => Classification::Public,
            (Held::Project, _) => {
                self.source_project_class(
                    &crate::GlobalId::new(destination.name().clone(), id.clone()),
                    &mut known,
                )
                .await?
            }
            (_, Some(filed)) => {
                self.source_project_class(
                    &crate::GlobalId::new(destination.name().clone(), filed),
                    &mut known,
                )
                .await?
            }
            (_, None) => Classification::Public,
        };
        self.admit_known(
            destination,
            item,
            id,
            declared.strictest(inherited),
            &repositories,
            exposure,
        )
        .await
    }

    /// Admit one write to an item `destination` holds, whose classification and repositories
    /// the caller has already read — so nothing more is read here but a private destination's
    /// reality.
    pub(crate) async fn admit_known(
        &self,
        destination: &ResolvedSource,
        item: &str,
        id: &NativeId,
        declared: Classification,
        repositories: &[Repository],
        exposure: &Exposure,
    ) -> Result<Rewrite, EngineError> {
        let classification = self.classify(declared, repositories).await;
        self.admit(
            destination,
            &Outbound {
                item: item.to_owned(),
                classification,
                target: WriteTarget::Existing(id),
                exposure,
            },
        )
        .await
    }
}

/// `exposure`, with the identifiers a refusal of it prints: the destination's name, and the
/// item's qualified id when it is one of that destination's — a copy names its item by its
/// source's id, which the write does not put anywhere.
pub(super) fn located(destination: &ResolvedSource, item: &str, exposure: Exposure) -> Exposure {
    let exposure = exposure.locate(LocatorKind::Source, destination.name().as_str());
    match item.parse::<crate::GlobalId>() {
        Ok(id) if &id.source == destination.name() => exposure.locate(LocatorKind::Item, item),
        _ => exposure,
    }
}

/// The refusal of a write `answer` did not pass, naming its item and destination only as the
/// policy screened them: verbatim where they passed, redacted where they carry a term or the
/// policy could not decide.
pub(super) fn refusal(
    destination: &ResolvedSource,
    item: &str,
    exposure: &Exposure,
    answer: &SanitizeVerdict,
) -> EngineError {
    let screened = if answer.locators.len() == exposure.locators.len() {
        answer.locators.clone()
    } else {
        redacted(&exposure.locators)
    };
    let shown = |kind: LocatorKind, value: &str| {
        exposure
            .locators
            .iter()
            .position(|locator| locator.kind == kind && locator.value == value)
            .and_then(|index| screened.get(index))
            .map(ToString::to_string)
    };
    let destination_shown = shown(LocatorKind::Source, destination.name().as_str())
        .unwrap_or_else(|| destination.name().to_string());
    let item_shown = shown(LocatorKind::Item, item).unwrap_or_else(|| item.to_owned());
    // Only a policy that says which entry it stopped at can say an identifier carries a term:
    // the default body redacts every locator of a refusal, naming none.
    let identifier = answer
        .unfixed
        .iter()
        .any(|unfixed| unfixed.surface == SanitizeSurface::Locators);
    match &answer.verdict {
        WriteVerdict::Unavailable { reason } => EngineError::BoundaryUnavailable {
            item: item_shown,
            destination: destination_shown,
            reason: reason.clone(),
        },
        WriteVerdict::Refuse { reason } => EngineError::BoundaryRefused {
            item: item_shown,
            destination: destination_shown,
            reason: if identifier {
                privateterms::LOCATOR_UNFIXABLE_REASON.to_owned()
            } else {
                reason.clone()
            },
        },
        WriteVerdict::Pass => EngineError::BoundaryUnavailable {
            item: item_shown,
            destination: destination_shown,
            reason: "the check passed nothing it was asked about".to_owned(),
        },
    }
}

/// Every comment of `id` its source holds, every page, oldest first — none for a source whose
/// tasks hold none.
///
/// # Errors
///
/// [`EngineError::SourceRefused`] when any page cannot be read: a sanitisation never decides
/// on part of an item's comments.
pub(super) async fn every_comment(
    destination: &ResolvedSource,
    id: &NativeId,
) -> Result<Vec<Comment>, EngineError> {
    if !destination.source().capabilities().comments.is_native() {
        return Ok(Vec::new());
    }
    let mut comments = Vec::new();
    let mut cursor = None;
    loop {
        let page = destination
            .source()
            .task_comments(
                id,
                &PageRequest {
                    cursor: cursor.take(),
                    limit: 100,
                },
            )
            .await;
        let page = page.map_err(|error| EngineError::SourceRefused {
            name: destination.name().to_string(),
            error,
        })?;
        // A task gone between its read and its comments' has none left to sanitise.
        let Some(page) = page else {
            return Ok(comments);
        };
        comments.extend(page.items);
        match page.next {
            Some(next) => cursor = Some(next),
            None => return Ok(comments),
        }
    }
}

/// What a stored task and its comments put in front of a reader, laid out so a sanitisation's
/// answer can be read back field by field: the content first among the prose, then each
/// comment's body; the title, each label's name and each metadata string among the short
/// fields; and the identifiers a diagnostic of a field would print.
pub(super) struct StoredTask {
    pub(super) exposure: Exposure,
    pub(super) depends_on: Vec<onetaskgraph_plugin_api::DependencyEdge>,
}

impl StoredTask {
    pub(super) async fn read(
        destination: &ResolvedSource,
        item: &str,
        task: &Task,
        comments: &[Comment],
    ) -> Result<Self, EngineError> {
        let mut depends_on = Vec::new();
        let mut cursor = None;
        loop {
            let page = destination
                .source()
                .task_dependencies(
                    &task.id,
                    onetaskgraph_plugin_api::Direction::DependsOn,
                    &PageRequest {
                        cursor: cursor.take(),
                        limit: 100,
                    },
                )
                .await
                .map_err(|error| EngineError::SourceRefused {
                    name: destination.name().to_string(),
                    error,
                })?;
            depends_on.extend(page.items);
            match page.next {
                Some(next) => cursor = Some(next),
                None => break,
            }
        }
        let mut exposure = Exposure::of_task(task);
        for edge in &depends_on {
            exposure
                .fixed
                .metadata
                .extend([edge.from.id().to_owned(), edge.to.id().to_owned()]);
            exposure = exposure
                .locate(LocatorKind::Item, edge.from.id())
                .locate(LocatorKind::Item, edge.to.id());
        }
        for comment in comments {
            exposure = exposure
                .and(Exposure::text(comment.body.clone()))
                .locate(LocatorKind::Comment, comment.id.as_str());
        }
        for key in task.metadata.keys() {
            exposure = exposure.locate(LocatorKind::MetadataKey, key.as_str());
        }
        Ok(Self {
            exposure: located(destination, item, exposure),
            depends_on,
        })
    }
}

/// One field or comment of a task a sanitisation changed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum ChangedField {
    Repositories,
    Status,
    Priority,
    Project,
    Delivers,
    DeliveredBy,
    Assignees,
    Id,
    Content,
    Title,
    Label,
    /// The value under one metadata key.
    Metadata(String),
    /// One comment, by its id.
    Comment(NativeId),
}

/// A task as sanitising it would write it, each comment with its new body, and what changed.
pub(super) type SanitisedTask = (
    Task,
    Vec<(Comment, String)>,
    Vec<ChangedField>,
    Vec<onetaskgraph_plugin_api::DependencyEdge>,
    Vec<Comment>,
);

/// What a sanitising write wrote, or would: the fields it changed, the task as written, and —
/// when the record moved and was screened again — that screening, which names its fields.
pub(super) enum Sanitised {
    Unchanged,
    Previewed {
        first: ChangedField,
        rest: Vec<ChangedField>,
    },
    Written {
        first: ChangedField,
        rest: Vec<ChangedField>,
        task: Box<Task>,
        screening: Option<Box<(Exposure, Vec<Locator>)>>,
    },
}

impl Sanitised {
    pub(super) fn fields(&self) -> impl Iterator<Item = &ChangedField> {
        let (first, rest): (Option<&ChangedField>, &[ChangedField]) = match self {
            Self::Unchanged => (None, &[]),
            Self::Previewed { first, rest } | Self::Written { first, rest, .. } => {
                (Some(first), rest)
            }
        };
        first.into_iter().chain(rest)
    }

    pub(super) fn was_written(&self) -> bool {
        matches!(self, Self::Written { .. })
    }

    pub(super) fn has_changes(&self) -> bool {
        !matches!(self, Self::Unchanged)
    }

    pub(super) fn screening(&self) -> Option<&(Exposure, Vec<Locator>)> {
        match self {
            Self::Written { screening, .. } => screening.as_deref(),
            _ => None,
        }
    }
}

/// One kind of field replacement cannot fix, as a refusal names it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnfixableKind {
    /// The item's `repositories`.
    Repositories,
    /// An id the item carries.
    Id,
    /// A non-rewritable field, including statuses, priorities, metadata keys and ids.
    FixedField,
    /// An identifier a diagnostic would print.
    Identifier,
    /// A term with no replacement declared for it.
    Unreplaced,
}

impl std::fmt::Display for UnfixableKind {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::Repositories => "repositories",
            Self::Id => "an id",
            Self::FixedField => "a fixed field",
            Self::Identifier => "an identifier",
            Self::Unreplaced => "a term no replacement is declared for",
        })
    }
}

/// The kinds of field a refusal names as beyond replacement, in the order first met; never
/// empty.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnfixableKinds(Vec<UnfixableKind>);

impl std::fmt::Display for UnfixableKinds {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for (index, kind) in self.0.iter().enumerate() {
            if index > 0 {
                formatter.write_str(", ")?;
            }
            kind.fmt(formatter)?;
        }
        Ok(())
    }
}

/// The fields and comments of `task` that `rewrite` changes, each with the record as it should
/// then be written; `None` when it changes none.
pub(super) fn sanitised_task(
    task: &Task,
    comments: &[Comment],
    rewrite: &Rewrite,
    depends_on: &[onetaskgraph_plugin_api::DependencyEdge],
) -> Option<SanitisedTask> {
    let mut rewritten = task.clone();
    rewrite.task(&mut rewritten);
    let mut fields = Vec::new();
    if rewritten.content != task.content {
        fields.push(ChangedField::Content);
    }
    if rewritten.title != task.title {
        fields.push(ChangedField::Title);
    }
    if rewritten.labels != task.labels {
        fields.push(ChangedField::Label);
    }
    for (key, value) in &rewritten.metadata {
        if task.metadata.get(key) != Some(value) {
            fields.push(ChangedField::Metadata(key.clone()));
        }
    }
    let edited: Vec<(Comment, String)> = comments
        .iter()
        .filter_map(|comment| {
            let body = rewrite.text(&comment.body);
            (body != comment.body).then(|| (comment.clone(), body))
        })
        .collect();
    for (comment, _) in &edited {
        fields.push(ChangedField::Comment(comment.id.clone()));
    }
    (!fields.is_empty()).then_some((
        rewritten,
        edited,
        fields,
        depends_on.to_vec(),
        comments.to_vec(),
    ))
}

/// The field of `task` — or the comment of `comments` — the `index`th entry of `surface` was,
/// in the layout [`StoredTask::read`] sends: the content then each comment's body as prose; the
/// title, each label's name, then each metadata value's strings, key by key, as short fields;
/// and those strings that read as paths, key by key, as paths.
pub(super) fn stored_field(
    task: &Task,
    comments: &[Comment],
    surface: SanitizeSurface,
    index: usize,
) -> Option<ChangedField> {
    fn strings(value: &Value, paths: bool) -> usize {
        match value {
            Value::String(text) => usize::from(!paths || text.contains('/') || text.contains('\\')),
            Value::Array(values) => values.iter().map(|value| strings(value, paths)).sum(),
            Value::Object(fields) => fields.values().map(|value| strings(value, paths)).sum(),
            _ => 0,
        }
    }
    let by_key = |mut index: usize, paths: bool| {
        for (key, value) in &task.metadata {
            let count = strings(value, paths);
            if index < count {
                return Some(ChangedField::Metadata(key.clone()));
            }
            index -= count;
        }
        None
    };
    match surface {
        SanitizeSurface::Text => {
            let offset = usize::from(task.content.is_some());
            if index < offset {
                Some(ChangedField::Content)
            } else {
                comments
                    .get(index - offset)
                    .map(|comment| ChangedField::Comment(comment.id.clone()))
            }
        }
        SanitizeSurface::Metadata => match index {
            0 => Some(ChangedField::Title),
            index if index <= task.labels.len() => Some(ChangedField::Label),
            index => by_key(index - 1 - task.labels.len(), false),
        },
        SanitizeSurface::Path => by_key(index, true),
        SanitizeSurface::Locators => None,
    }
}

/// How a report names one changed field: a fixed word as it is — `content`, `title`, `label` —
/// and a metadata key or a comment id by its screened form, `metadata:<key>` or
/// `comment:<id>` where it passed.
pub(super) fn screened_field(
    field: &ChangedField,
    exposure: &Exposure,
    answered: &[Locator],
) -> Locator {
    let screened = |kind: LocatorKind, value: &str, prefix: &str| match screened_locator(
        exposure, answered, kind, value,
    ) {
        Locator::Plain(value) => Locator::Plain(format!("{prefix}{value}")),
        redacted @ Locator::Redacted { .. } => redacted,
    };
    match field {
        ChangedField::Repositories => Locator::Plain("repositories".to_owned()),
        ChangedField::Status => Locator::Plain("status".to_owned()),
        ChangedField::Priority => Locator::Plain("priority".to_owned()),
        ChangedField::Project => Locator::Plain("project".to_owned()),
        ChangedField::Delivers => Locator::Plain("delivers".to_owned()),
        ChangedField::DeliveredBy => Locator::Plain("delivered_by".to_owned()),
        ChangedField::Assignees => Locator::Plain("assignees".to_owned()),
        ChangedField::Id => Locator::Plain("id".to_owned()),
        ChangedField::Content => Locator::Plain("content".to_owned()),
        ChangedField::Title => Locator::Plain("title".to_owned()),
        ChangedField::Label => Locator::Plain("label".to_owned()),
        ChangedField::Metadata(key) => screened(LocatorKind::MetadataKey, key, "metadata:"),
        ChangedField::Comment(id) => screened(LocatorKind::Comment, id.as_str(), "comment:"),
    }
}

/// `answered`'s screening of `value` of `kind` among `exposure`'s locators, or every locator
/// redacted when the answer screened none. A value absent from the input is also redacted.
pub(super) fn screened_locator(
    exposure: &Exposure,
    answered: &[Locator],
    kind: LocatorKind,
    value: &str,
) -> Locator {
    let answered = if answered.len() == exposure.locators.len() {
        answered.to_vec()
    } else {
        redacted(&exposure.locators)
    };
    exposure
        .locators
        .iter()
        .position(|locator| locator.kind == kind && locator.value == value)
        .and_then(|index| answered.get(index).cloned())
        .unwrap_or(Locator::Redacted { kind, ordinal: 1 })
}

impl Engine {
    /// Whether the policy can rewrite: a `sanitize_command`, or a linking caller's own policy.
    fn can_rewrite(&self) -> bool {
        self.boundary.mode != PolicyMode::Checks
    }

    /// Whether a narrow write to an existing task is screened with the task as it stands, so a
    /// term already on it is sanitised rather than written past: when the policy can rewrite
    /// and either the store opted in or this invocation asked what that would change.
    fn sanitizes_whole_items(&self) -> bool {
        self.can_rewrite()
            && (self.boundary.sanitize_stored_on_write || self.boundary.sanitize_dry_run)
    }

    /// Sanitise the task `id` as stored ahead of a narrow write whose value is composed from
    /// what the task holds — an attestation's digest — so that value is composed over the task
    /// as the write will leave it, rather than over text the write's own sanitisation then
    /// replaces. Only where that write would sanitise the stored task (`sanitize_stored_on_write`,
    /// a policy that can rewrite, an active boundary and a destination not declared private);
    /// anywhere else, and in a `--sanitize-dry-run` — which the write itself answers — it reads
    /// and writes nothing, and answers `false`. The write then finds the task clean and
    /// sanitises nothing more; `true` says the task was screened, and may since have changed.
    pub(crate) async fn sanitise_stored_task(
        &self,
        destination: &ResolvedSource,
        id: &crate::GlobalId,
    ) -> Result<bool, EngineError> {
        if !self.boundary.active
            || !self.boundary.sanitize_stored_on_write
            || self.boundary.sanitize_dry_run
            || !self.can_rewrite()
            || self.declared(destination.name()) == SourceVisibility::Private
        {
            return Ok(false);
        }
        self.admit_existing(
            destination,
            &id.to_string(),
            &id.native,
            Held::Task,
            &Exposure::default(),
        )
        .await
        .map(|_| true)
    }

    /// Refuse a narrow write to a task already private by its own record, naming `repositories`
    /// only when they already named a private repository. Its locators are screened by one call
    /// on this refusal's path alone.
    async fn refuse_stored_private(
        &self,
        destination: &ResolvedSource,
        item: &str,
        task: &Task,
    ) -> Result<(), EngineError> {
        let repository_classification = self
            .classify(Classification::Public, &task.repositories)
            .await;
        if task.classification.strictest(repository_classification) != Classification::Private {
            return Ok(());
        }
        let locators = located(destination, item, Exposure::default());
        let answer = self.screen(&locators).await;
        let shown = |kind: LocatorKind, value: &str| {
            screened_locator(&locators, &answer.locators, kind, value).to_string()
        };
        let item = shown(LocatorKind::Item, item);
        let shown_destination = shown(LocatorKind::Source, destination.name().as_str());
        let declared = self.declared(destination.name());
        Err(if repository_classification == Classification::Private {
            EngineError::NotPrivateDestinationPredates {
                item,
                destination: shown_destination,
                declared,
            }
        } else {
            EngineError::NotPrivateDestination {
                item,
                destination: shown_destination,
                declared,
            }
        })
    }

    /// Admit one narrow write to the task `task`, as `destination` holds it, by one sanitisation
    /// of the write together with the task and every comment on it: what the write adds, and
    /// what it leaves standing beside it in public.
    ///
    /// A term already on the task that the answer rewrites is sanitised before the write is
    /// made — the task and its comments written back, conditional on the version the read
    /// answered — and reported in the write's `sanitization`. One the answer cannot fix refuses
    /// the write saying the term was already on the item before it; a task already private by
    /// its own record is refused by the classification rule, naming `repositories` only when
    /// they already named a private repository.
    // Keep the whole-item recovery state behind one future allocation: embedding it in
    // every narrow-write future repeats its optimizer work in each release build.
    #[inline(never)]
    fn admit_whole_task<'a>(
        &'a self,
        destination: &'a ResolvedSource,
        item: &'a str,
        id: &'a NativeId,
        task: Task,
        exposure: &'a Exposure,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<Rewrite, EngineError>> + Send + 'a>,
    > {
        Box::pin(self.admit_whole_task_inner(destination, item, id, task, exposure))
    }

    async fn admit_whole_task_inner(
        &self,
        destination: &ResolvedSource,
        item: &str,
        id: &NativeId,
        task: Task,
        exposure: &Exposure,
    ) -> Result<Rewrite, EngineError> {
        self.refuse_stored_private(destination, item, &task).await?;
        let comments = every_comment(destination, id).await?;
        let stored = StoredTask::read(destination, item, &task, &comments).await?;
        let write = located(destination, item, exposure.clone());
        let whole = write.clone().and(stored.exposure.clone());
        if let Some(rewrite) = self.boundary.passed(&whole) {
            return Ok(rewrite);
        }
        let answer = self.screen(&whole).await;
        match &answer.verdict {
            WriteVerdict::Pass => {
                let rewrite = Rewrite::of(&whole, &answer);
                if let Some(sanitised) =
                    sanitised_task(&task, &comments, &rewrite, &stored.depends_on)
                {
                    let written = self
                        .sanitise_task(
                            destination,
                            item,
                            id,
                            &task,
                            sanitised,
                            (&whole, &answer.locators),
                        )
                        .await?;
                    if let Some(written) = written {
                        self.settle_after_sanitising(destination, item, id, &written, &write)
                            .await?;
                    }
                    // Held as passed only now the item stands sanitised: a write whose
                    // sanitisation failed is screened afresh, and sanitised again, next time.
                    if !self.boundary.sanitize_dry_run {
                        self.boundary.pass(&whole, &rewrite);
                    }
                    if self.boundary.sanitize_dry_run {
                        return Err(EngineError::SanitizeDryRun {
                            item: screened_locator(
                                &whole,
                                &answer.locators,
                                LocatorKind::Item,
                                item,
                            )
                            .to_string(),
                            would_pass: true,
                        });
                    }
                } else {
                    self.boundary.pass(&whole, &rewrite);
                }
                Ok(rewrite)
            }
            WriteVerdict::Refuse { reason } => {
                // Whether the term left is the task's own, as it stood before this write: an
                // answer that says which entry names one in the task's part; one that cannot
                // say is asked about the write alone, on this refusal's path only.
                let offset = |surface: SanitizeSurface, fixed: bool| match (surface, fixed) {
                    (SanitizeSurface::Text, false) => write.text.len(),
                    (SanitizeSurface::Path, false) => write.paths.len(),
                    (SanitizeSurface::Metadata, false) => write.metadata.len(),
                    (SanitizeSurface::Text, true) => write.fixed.text.len(),
                    (SanitizeSurface::Path, true) => write.fixed.paths.len(),
                    (SanitizeSurface::Metadata, true) => write.fixed.metadata.len(),
                    (SanitizeSurface::Locators, _) => usize::MAX,
                };
                let predates = if answer.unfixed.is_empty() {
                    self.screen(&write).await.verdict == WriteVerdict::Pass
                } else {
                    answer.unfixed.iter().any(|unfixed| {
                        usize::try_from(unfixed.index)
                            .is_ok_and(|index| index >= offset(unfixed.surface, unfixed.fixed))
                    })
                };
                if !predates {
                    return Err(refusal(destination, item, &whole, &answer));
                }
                // What replacement can fix is fixed, once; what it cannot is named. A record
                // one of whose own identifiers carries a term is written nothing, as a sweep
                // writes it nothing: no write of it could be named without printing the term.
                let rewrite = Rewrite::of(&whole, &answer);
                let named = answer
                    .unfixed
                    .iter()
                    .any(|unfixed| unfixed.surface == SanitizeSurface::Locators);
                if !named
                    && let Some(sanitised) =
                        sanitised_task(&task, &comments, &rewrite, &stored.depends_on)
                {
                    self.sanitise_task(
                        destination,
                        item,
                        id,
                        &task,
                        sanitised,
                        (&whole, &answer.locators),
                    )
                    .await?;
                }
                let mut unfixable = Vec::new();
                for unfixed in &answer.unfixed {
                    let kind = match (unfixed.surface, unfixed.fixed) {
                        (SanitizeSurface::Locators, _) => UnfixableKind::Identifier,
                        (SanitizeSurface::Metadata, true) => {
                            let at = usize::try_from(unfixed.index).unwrap_or(usize::MAX);
                            let value = whole.fixed.metadata.get(at);
                            if value.is_some_and(|value| {
                                task.repositories
                                    .iter()
                                    .any(|repository| repository.as_str() == value)
                            }) {
                                UnfixableKind::Repositories
                            } else {
                                UnfixableKind::FixedField
                            }
                        }
                        (_, true) => UnfixableKind::FixedField,
                        (_, false) => UnfixableKind::Unreplaced,
                    };
                    if !unfixable.contains(&kind) {
                        unfixable.push(kind);
                    }
                }
                let screened = |kind: LocatorKind, value: &str| {
                    screened_locator(&whole, &answer.locators, kind, value)
                };
                Err(EngineError::BoundaryRefusedPredates {
                    item: screened(LocatorKind::Item, item).to_string(),
                    destination: screened(LocatorKind::Source, destination.name().as_str())
                        .to_string(),
                    reason: reason.clone(),
                    unfixable: UnfixableKinds(if unfixable.is_empty() {
                        vec![UnfixableKind::Unreplaced]
                    } else {
                        unfixable
                    }),
                })
            }
            WriteVerdict::Unavailable { .. } => Err(refusal(destination, item, &whole, &answer)),
        }
    }

    /// Write `sanitised` back over `task` — unless this is a dry run, which only reports it — on
    /// condition that the task is still at the version `task` was read at.
    ///
    /// A task that moved meanwhile is read again once and sanitised from that fresh record, so
    /// a change stored in between is kept; one that moves a second time is refused as a
    /// conflict, nothing overwritten. Every field and comment written is reported by its
    /// screened locator.
    async fn sanitise_task(
        &self,
        destination: &ResolvedSource,
        item: &str,
        id: &NativeId,
        task: &Task,
        sanitised: SanitisedTask,
        screened: (&Exposure, &[Locator]),
    ) -> Result<Option<Task>, EngineError> {
        let (exposure, answered) = screened;
        let shown_item = screened_locator(exposure, answered, LocatorKind::Item, item);
        let result = self
            .sanitise_written(
                destination,
                item,
                id,
                task,
                sanitised,
                self.boundary.sanitize_dry_run,
            )
            .await
            .map_err(|conflict| match conflict {
                EngineError::SanitizeConflict { .. } => EngineError::SanitizeConflict {
                    item: shown_item.to_string(),
                },
                other => other,
            })?;
        if let Ok(mut sanitized) = self.boundary.sanitized.lock() {
            sanitized.dry_run |= self.boundary.sanitize_dry_run;
            // A record read again is named by the screening of what was read, which holds the
            // keys and comments another writer added.
            let (exposure, answered) = result
                .screening()
                .map_or((exposure, answered), |(exposure, answered)| {
                    (exposure, answered.as_slice())
                });
            for field in result.fields() {
                sanitized.side_effect.push(SideEffect {
                    item: shown_item.clone(),
                    field: screened_field(field, exposure, answered),
                });
            }
        }
        Ok(match result {
            Sanitised::Written { task, .. } => Some(*task),
            _ => None,
        })
    }

    /// Write `sanitised` back over `task` unless `dry_run`, on condition that the task is still
    /// at the version `task` was read at, answering the field words of what was — or would be
    /// — written.
    ///
    /// A task that moved meanwhile is read again once and sanitised from that fresh record, so
    /// a change stored in between is kept; one that moves a second time is
    /// [`EngineError::SanitizeConflict`], nothing overwritten.
    pub(super) async fn sanitise_written(
        &self,
        destination: &ResolvedSource,
        item: &str,
        id: &NativeId,
        task: &Task,
        sanitised: SanitisedTask,
        dry_run: bool,
    ) -> Result<Sanitised, EngineError> {
        let (mut rewritten, mut edited, mut fields, mut depends_on, original_comments) = sanitised;
        let mut screening = None;
        if !dry_run {
            let fresh = self.read_held_task(destination, id).await?;
            // A comment can change without moving its task, including one that was clean
            // at screening. Compare the entire comment snapshot before rebuilding once.
            let fresh_comments = every_comment(destination, id).await?;
            if fresh.version != task.version
                || !holds_as_written(&fresh, task)
                || fresh_comments != original_comments
            {
                let comments = fresh_comments;
                let stored = StoredTask::read(destination, item, &fresh, &comments).await?;
                let answer = self.screen(&stored.exposure).await;
                if answer.verdict != WriteVerdict::Pass {
                    return Err(refusal(destination, item, &stored.exposure, &answer));
                }
                let rewrite = Rewrite::of(&stored.exposure, &answer);
                let Some(again) = sanitised_task(&fresh, &comments, &rewrite, &stored.depends_on)
                else {
                    return Ok(Sanitised::Unchanged);
                };
                (rewritten, edited, fields, depends_on, _) = again;
                screening = Some((stored.exposure, answer.locators));
                let latest = self.read_held_task(destination, id).await?;
                let latest_comments = every_comment(destination, id).await?;
                if latest.version != fresh.version
                    || !holds_as_written(&latest, &fresh)
                    || latest_comments != comments
                {
                    return Err(EngineError::SanitizeConflict {
                        item: item.to_owned(),
                    });
                }
            }
            let mut fields = fields.into_iter();
            let Some(first) = fields.next() else {
                return Ok(Sanitised::Unchanged);
            };
            self.write_sanitised(destination, id, &rewritten, &edited, depends_on)
                .await?;
            return Ok(Sanitised::Written {
                first,
                rest: fields.collect(),
                task: Box::new(rewritten),
                screening: screening.map(Box::new),
            });
        }
        let mut fields = fields.into_iter();
        Ok(match fields.next() {
            Some(first) => Sanitised::Previewed {
                first,
                rest: fields.collect(),
            },
            None => Sanitised::Unchanged,
        })
    }

    /// Hold the write a sanitisation was made for to the record that sanitisation wrote: read
    /// the task back, and where another writer changed it since — a plugin answers no version
    /// for a write, so what is compared is what it holds — put the record as it now stands to
    /// the policy once more, beside the write, and go on over it only when that answer rewrites
    /// nothing of it and a second read finds it standing still. Anything else is
    /// [`EngineError::SanitizeConflict`], the write not made, so the other writer's change is
    /// never overwritten.
    async fn settle_after_sanitising(
        &self,
        destination: &ResolvedSource,
        item: &str,
        id: &NativeId,
        written: &Task,
        write: &Exposure,
    ) -> Result<(), EngineError> {
        let fresh = self.read_held_task(destination, id).await?;
        if holds_as_written(&fresh, written) {
            return Ok(());
        }
        let comments = every_comment(destination, id).await?;
        let stored = StoredTask::read(destination, item, &fresh, &comments).await?;
        let whole = write.clone().and(stored.exposure);
        let answer = self.screen(&whole).await;
        let shown = screened_locator(&whole, &answer.locators, LocatorKind::Item, item);
        if answer.verdict != WriteVerdict::Pass {
            return Err(refusal(destination, item, &whole, &answer));
        }
        let rewrite = Rewrite::of(&whole, &answer);
        let latest = self.read_held_task(destination, id).await?;
        if sanitised_task(&fresh, &comments, &rewrite, &stored.depends_on).is_some()
            || latest.version != fresh.version
            || !holds_as_written(&latest, &fresh)
        {
            return Err(EngineError::SanitizeConflict {
                item: shown.to_string(),
            });
        }
        Ok(())
    }

    pub(super) async fn read_held_task(
        &self,
        destination: &ResolvedSource,
        id: &NativeId,
    ) -> Result<Task, EngineError> {
        let task = destination
            .source()
            .get_task(id)
            .await
            .map_err(|error| EngineError::SourceRefused {
                name: destination.name().to_string(),
                error,
            })?
            .ok_or_else(|| EngineError::NoSuchTask {
                id: format!("{}:{id}", destination.name()),
            })?;
        checked_task_identity(destination, id, task)
    }

    /// One task's sanitised fields and comments, written through the source's own write side:
    /// the task whole with every forward edge it holds, then each comment's new body.
    async fn write_sanitised(
        &self,
        destination: &ResolvedSource,
        id: &NativeId,
        task: &Task,
        comments: &[(Comment, String)],
        depends_on: Vec<onetaskgraph_plugin_api::DependencyEdge>,
    ) -> Result<(), EngineError> {
        let refused = |error| EngineError::SourceRefused {
            name: destination.name().to_string(),
            error,
        };
        // Every comment's new body is checked before anything is written, so a replacement
        // that would empty one leaves the task as it was rather than half sanitised.
        let bodies = comments
            .iter()
            .map(|(comment, body)| {
                CommentBody::new(body.clone()).map(|body| (comment.id.clone(), body))
            })
            .collect::<Result<Vec<_>, _>>()
            .map_err(refused)?;
        super::stored::forget(
            destination,
            onetaskgraph_plugin_api::MetadataRecord::Task,
            id,
        );
        let written = destination
            .source()
            .write_task(&onetaskgraph_plugin_api::ItemWrite {
                target: Some(id.clone()),
                item: task.clone(),
                depends_on,
            })
            .await
            .map_err(refused)?;
        if &written != id {
            return Err(refused(SourceError::Malformed {
                message: "a sanitising write was answered as a write of another task".to_owned(),
            }));
        }
        for (comment, body) in &bodies {
            let edited = destination
                .source()
                .edit_comment(id, comment, body)
                .await
                .map_err(refused)?;
            match edited {
                None => {
                    return Err(refused(SourceError::Refused {
                        message: format!(
                            "comment {comment} of {id} was gone before its sanitising write"
                        ),
                    }));
                }
                Some(edited) if &edited.id != comment => {
                    return Err(refused(SourceError::Malformed {
                        message: "a sanitising comment edit was answered with another comment"
                            .to_owned(),
                    }));
                }
                Some(_) => {}
            }
        }
        Ok(())
    }
}

fn checked_task_identity(
    destination: &ResolvedSource,
    id: &NativeId,
    task: Task,
) -> Result<Task, EngineError> {
    if &task.id != id {
        return Err(EngineError::SourceRefused {
            name: destination.name().to_string(),
            error: SourceError::Malformed {
                message: "a task read was answered with another task".to_owned(),
            },
        });
    }
    Ok(task)
}

/// Whether `read` holds what `written` says, every field a person edits compared and the
/// tokens a backend stamps on a write left out.
fn holds_as_written(read: &Task, written: &Task) -> bool {
    read.title == written.title
        && read.content == written.content
        && read.status == written.status
        && read.priority == written.priority
        && read.metadata == written.metadata
        && read.repositories == written.repositories
        && read.project == written.project
        && read.classification == written.classification
        && read.delivers == written.delivers
        && read.delivered_by == written.delivered_by
        && read
            .labels
            .iter()
            .map(|label| &label.name)
            .eq(written.labels.iter().map(|label| &label.name))
}

/// Which kind of record a narrow write is about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Held {
    Task,
    Project,
    Document,
}

/// What [`Engine::filed_under`] makes of a project its destination does not hold.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Absent {
    /// The item is created naming it, so what it inherits cannot be established.
    Unclassifies,
    /// A copy carries it as its source's opaque project id, and the item was already
    /// classified at its source.
    Carried,
}

/// The form `id` names an item by when that form alone says it is no native id — an address,
/// or a human key with a prefix — and `None` when only a read of it can say.
pub(crate) fn plainly_not_native(id: &str) -> Option<ReferenceForm> {
    if id.contains("://") || id.starts_with("www.") {
        Some(ReferenceForm::Url)
    } else if prefixed_key(id) {
        Some(ReferenceForm::PrefixedKey)
    } else {
        None
    }
}

/// Whether `id` reads as a human key with a team or project prefix: letters and digits, a
/// dash, and a number — `ABC-123`.
fn prefixed_key(id: &str) -> bool {
    id.split_once('-').is_some_and(|(prefix, number)| {
        prefix
            .chars()
            .next()
            .is_some_and(|first| first.is_ascii_alphabetic())
            && prefix.chars().all(|c| c.is_ascii_alphanumeric())
            && !number.is_empty()
            && number.chars().all(|c| c.is_ascii_digit())
    })
}

/// Whether `id` reads as a slug: words of lower-case letters and digits joined by dashes.
fn slug(id: &str) -> bool {
    id.contains('-')
        && id.split('-').all(|word| {
            !word.is_empty()
                && word
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        })
}

/// How a reference names a private item when it is not by the item's own native id — the whole
/// of what a refusal says about it, never what it held.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReferenceForm {
    /// A human key with a team or project prefix: `ABC-123`.
    PrefixedKey,
    /// An address.
    Url,
    /// Words joined by dashes that name no item by its native id.
    Slug,
    /// Something else its source holds no item under as its native id.
    NotNative,
    /// An opaque id whose tracker no source this host configures is, so nothing can confirm it.
    Unconfirmed,
}

impl std::fmt::Display for ReferenceForm {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::PrefixedKey => "a prefixed key",
            Self::Url => "a URL",
            Self::Slug => "a slug",
            Self::NotNative => "an id that is not the item's native id",
            Self::Unconfirmed => "an opaque id that no source this host configures can confirm",
        })
    }
}

/// The boundary domain's published sanitisation response roots.
pub(crate) fn schemas() -> [(&'static str, schemars::Schema); 2] {
    [
        ("Sanitization", schemars::schema_for!(Sanitization)),
        ("SanitizeDryRun", schemars::schema_for!(SanitizeDryRun)),
    ]
}

#[cfg(test)]
mod diagnostic_locators {
    use super::{
        ChangedField, Exposure, Locator, LocatorKind, NativeId, screened_field, screened_locator,
    };

    #[test]
    fn an_identifier_absent_from_the_screening_cannot_be_printed_plain() {
        let exposure = Exposure::default();
        assert!(matches!(
            screened_locator(&exposure, &[], LocatorKind::Item, "site:quietharbor"),
            Locator::Redacted {
                kind: LocatorKind::Item,
                ..
            }
        ));
        for field in [
            ChangedField::Metadata("quietharbor".to_owned()),
            ChangedField::Comment(NativeId("quietharbor".to_owned())),
        ] {
            assert!(matches!(
                screened_field(&field, &exposure, &[]),
                Locator::Redacted { .. }
            ));
        }
    }

    #[test]
    fn a_field_without_an_answer_is_redacted_and_a_screened_field_keeps_its_prefix() {
        let exposure = Exposure::default().locate(LocatorKind::MetadataKey, "ordinary");
        let field = ChangedField::Metadata("ordinary".to_owned());
        assert!(matches!(
            screened_field(&field, &exposure, &[]),
            Locator::Redacted {
                kind: LocatorKind::MetadataKey,
                ..
            }
        ));
        assert_eq!(
            screened_field(&field, &exposure, &[Locator::Plain("ordinary".to_owned())]),
            Locator::Plain("metadata:ordinary".to_owned())
        );
    }
}
