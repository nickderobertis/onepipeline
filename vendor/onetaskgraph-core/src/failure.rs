//! Why a command failed, in a form a caller can branch on.
//!
//! A person reads the stderr line; a program acting on a failure needs one question
//! answered first — would asking again, unchanged, ever get a different answer? A refusal
//! never will, and retrying one on a timer spends a hosted source's allowance for nothing.
//! So every failure this product reports carries a [`FailureClass`], and the class is
//! decided in exactly one place, [`classify`], which both the failure document and each
//! entry of a partial answer's `errors` call rather than restating.

use onetaskgraph_plugin_api::{SourceError, SourceName};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::config::ConfigError;
use crate::engine::{EngineError, TaskAssetsError};

/// Whether repeating a failed request unchanged could change the answer.
///
/// Closed on purpose: a caller acts on this alone, so a third value would be one every
/// caller written before it silently misreads. What the failure *was* is
/// [`Failure`]'s `kind`, which is the open half.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum FailureClass {
    /// The store or a source declined the request; repeating it unchanged cannot alter
    /// the answer.
    Refused,
    /// The request got no ruling a caller could act on, so the same request may succeed
    /// later.
    Transient,
}

/// The class of a failure, from the source error that caused it — or `None` when no
/// source caused it.
///
/// The whole mapping, stated once. A rate limit and a source that could not be reached
/// are the only failures a wait can change; a source that answered with a refusal, a
/// configuration it will not run on, a credential it rejected or data it cannot represent
/// will answer the same way next time, and so will every failure the engine decided on
/// its own — an id that names nothing, a stale origin, a source nothing configures.
#[must_use]
pub fn classify(cause: Option<&SourceError>) -> FailureClass {
    match cause {
        Some(SourceError::RateLimited { .. } | SourceError::Unavailable { .. }) => {
            FailureClass::Transient
        }
        Some(
            SourceError::Refused { .. }
            | SourceError::Config { .. }
            | SourceError::Auth { .. }
            | SourceError::Malformed { .. },
        )
        | None => FailureClass::Refused,
    }
}

/// The document a command writes to standard output when it exits `1` under machine
/// output: one object whose single member is the [`Failure`].
///
/// An object around the failure rather than the failure itself, so a reader can tell
/// this document from every answer document by its one key before reading anything else.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct FailureDocument {
    /// Why the command failed.
    pub failure: Failure,
    /// What a write made with `--expect` that failed came to, part by part — its `result`,
    /// its `fallback` and what it `spent` — absent for every other failure.
    #[serde(flatten)]
    pub expected: crate::cache::ExpectedWrite,
    /// What the destination item a copy refused to write held, as the copy's read of it
    /// before writing found it — present only for a refusal made after that read, such as
    /// `target-task-origin`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub held: Option<crate::cache::WriteResult>,
    /// `true` when the write was refused for something already on its item before it — a
    /// private repository among its `repositories`, or a term sanitising it could not fix —
    /// and absent otherwise.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    #[schemars(!skip_serializing_if)]
    pub predates_write: bool,
    /// Every item a copy refused as `oversize` — its source id, its destination, the size of
    /// the body that write would store and the most that destination holds — and absent for
    /// every other failure.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[schemars(!skip_serializing_if)]
    pub oversize: Vec<crate::Oversized>,
}

/// Why one command failed.
///
/// Every member is always written, `source` and `retry_after_seconds` as `null` when they
/// have nothing to say, so a caller reads a fixed shape.
// Built only from an engine error, a configuration error or `Failure::decided`, so `class`
// always follows `classify` and `message` is always the failure's own rendering. `Deserialize`
// is for reading one back out of a report that carries it — a delivered task a copy could not
// keep in step — and builds nothing a caller could not already have been handed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[schemars(transform = every_member_required)]
pub struct Failure {
    /// Whether repeating the request unchanged could change the answer.
    class: FailureClass,
    /// What failed: the causing source error's own `kind` when a source caused it, and
    /// otherwise this product's kebab-case name for the failure, such as `no-such-item`.
    // llmlint: ignore[invalid_states_unrepresentable] Open by contract: the kind a source error carries is copied verbatim, and a subprocess-hosted plugin speaks the same vocabulary a version later than this binary, so no closed type here could hold every kind a caller is owed. `class` is the closed half a caller acts on.
    kind: String,
    /// The configured source the failure came from, or `null` when none did.
    source: Option<SourceName>,
    /// What the command reported on standard error, without its `onetaskgraph: ` prefix.
    message: String,
    /// How many seconds a rate limit asked the caller to wait, or `null` when it named no
    /// wait.
    retry_after_seconds: Option<u64>,
}

/// Mark every member of an object schema required.
///
/// `#[schemars(required)]` on an `Option` member drops `null` from its type, which would
/// declare the very `null` this document writes invalid; required-and-nullable is what
/// `Failure` actually is, so the list is filled in after the members are described.
fn every_member_required(schema: &mut schemars::Schema) {
    let members: Vec<serde_json::Value> = schema
        .get("properties")
        .and_then(serde_json::Value::as_object)
        .map(|properties| {
            properties
                .keys()
                .cloned()
                .map(serde_json::Value::from)
                .collect()
        })
        .unwrap_or_default();
    schema.insert("required".to_owned(), serde_json::Value::Array(members));
}

// These doc comments are the Rust surface alone: the type's own doc comment is the schema's
// description, so what a linking caller reads is said here rather than there.
/// What a linking caller reads.
///
/// The same members the failure document writes, through [`Failure::class`],
/// [`Failure::kind`], [`Failure::source`], [`Failure::message`] and
/// [`Failure::retry_after_seconds`], so a Rust caller never serialises a failure to branch on
/// it. They read and nothing more: no caller can build or alter a failure whose class
/// disagrees with [`classify`].
impl Failure {
    /// A failure this product decided on its own, with no source behind it.
    #[must_use]
    pub fn decided(kind: &str, message: impl Into<String>) -> Self {
        Self::caused(kind.to_owned(), None, None, message.into())
    }

    /// Whether repeating the request unchanged could change the answer — the one closed
    /// value a caller branches on, as [`classify`] decided it for this failure's cause.
    #[must_use]
    pub fn class(&self) -> FailureClass {
        self.class
    }

    /// What failed: the causing source error's own `kind` when a source caused it, and
    /// otherwise this product's kebab-case name for the failure, such as `no-such-item`.
    #[must_use]
    pub fn kind(&self) -> &str {
        &self.kind
    }

    /// The configured source the failure came from, or `None` when none did.
    #[must_use]
    pub fn source(&self) -> Option<&SourceName> {
        self.source.as_ref()
    }

    /// What a person reads: the stderr line without its prefix.
    #[must_use]
    pub fn message(&self) -> &str {
        &self.message
    }

    /// How many seconds a rate limit asked the caller to wait, or `None` when it named no
    /// wait.
    #[must_use]
    pub fn retry_after_seconds(&self) -> Option<u64> {
        self.retry_after_seconds
    }

    /// One failure, classed by what caused it.
    fn caused(
        kind: String,
        source: Option<SourceName>,
        cause: Option<&SourceError>,
        message: String,
    ) -> Self {
        Self {
            class: classify(cause),
            kind,
            source,
            message,
            retry_after_seconds: match cause {
                Some(SourceError::RateLimited {
                    retry_after_seconds,
                    ..
                }) => *retry_after_seconds,
                _ => None,
            },
        }
    }
}

/// The `kind` a source error is written with on the wire, verbatim.
///
/// Read off its own serialisation rather than matched here, so this cannot spell a kind
/// differently from the `SourceError` a partial answer carries beside it.
fn source_kind(error: &SourceError) -> String {
    serde_json::to_value(error)
        .ok()
        .and_then(|wire| wire.get("kind")?.as_str().map(str::to_owned))
        .expect("a source error is a kind-tagged object")
}

/// The failure an engine error amounts to, walking to the one it wraps.
///
/// A failure that wraps another — a destination that could not be built, a source that
/// refused part of a copy, a copy that could not be undone — takes the class and kind of
/// the failure it wraps, because that is what a caller has to act on.
fn cause(error: &EngineError) -> (String, Option<SourceName>, Option<&SourceError>) {
    // Every name below that is reported as a source was a configured `SourceName` before
    // the engine rendered it into the error, so it parses back; a name that did not would
    // be one no configuration holds, which is exactly what `null` says.
    let configured = |name: &str| SourceName::new(name.to_owned()).ok();
    match error {
        EngineError::UnknownSource { .. } => ("unknown-source".to_owned(), None, None),
        EngineError::IncompleteSource { .. } => ("incomplete-source".to_owned(), None, None),
        EngineError::Token { .. } => ("page-token".to_owned(), None, None),
        EngineError::NoSources => ("no-sources".to_owned(), None, None),
        EngineError::NotWritable { name, .. } => {
            ("not-writable".to_owned(), configured(name), None)
        }
        EngineError::NoDocuments { name, .. } => {
            ("no-documents".to_owned(), configured(name), None)
        }
        EngineError::NotCached { name } => ("not-cached".to_owned(), configured(name), None),
        EngineError::ExpectOneTask => ("expect-one-task".to_owned(), None, None),
        EngineError::IfVersionOneTask => ("if-version-one-task".to_owned(), None, None),
        EngineError::RepeatedPage { .. } => ("repeated-page".to_owned(), None, None),
        EngineError::ItemOutsideSource { .. } => ("item-outside-source".to_owned(), None, None),
        EngineError::BatchDocument { .. } => ("invalid-batch".to_owned(), None, None),
        EngineError::UnnarrowedQuery { .. } => ("unnarrowed-query".to_owned(), None, None),
        EngineError::RelationsIncomplete { .. } => ("relations-incomplete".to_owned(), None, None),
        EngineError::VersionMoved { .. } => ("version-moved".to_owned(), None, None),
        EngineError::PlanMember { error, .. } => cause(error),
        EngineError::NoComments { name, .. } | EngineError::CommentlessDestination { name, .. } => {
            ("no-comments".to_owned(), configured(name), None)
        }
        EngineError::ChangedWhileCopied { .. } => ("changed-while-copied".to_owned(), None, None),
        EngineError::StoredContentDiverged { item, .. } => (
            "stored-content-diverged".to_owned(),
            Some(item.source.clone()),
            None,
        ),
        EngineError::QueryNotTasks => ("query-not-tasks".to_owned(), None, None),
        EngineError::StatusesNotTasks => ("statuses-not-tasks".to_owned(), None, None),
        EngineError::ConditionsNotTasks => ("conditions-not-tasks".to_owned(), None, None),
        EngineError::CreateInProgress { .. } => ("create-in-progress".to_owned(), None, None),
        EngineError::CreateDuplicated { destination, .. } => (
            "create-duplicated".to_owned(),
            Some(destination.clone()),
            None,
        ),
        EngineError::CreationOwner { destination, .. } => {
            ("creation-owner".to_owned(), Some(destination.clone()), None)
        }
        EngineError::OtherwiseWithoutStatus => ("otherwise-without-status".to_owned(), None, None),
        EngineError::StatusConditionPreserved => {
            ("status-condition-preserved".to_owned(), None, None)
        }
        EngineError::DestinationStatus { destination, .. } => (
            "destination-status".to_owned(),
            Some(destination.source.clone()),
            None,
        ),
        EngineError::CommentsNotDocuments => ("comments-not-documents".to_owned(), None, None),
        EngineError::NoPriority { name, .. } => ("no-priority".to_owned(), configured(name), None),
        EngineError::NoAssignees { name, .. } => {
            ("no-assignees".to_owned(), configured(name), None)
        }
        EngineError::SeveralAssignees { .. } => ("several-assignees".to_owned(), None, None),
        EngineError::CommentsNotWritable { name, .. }
        | EngineError::StatusNotWritable { name, .. }
        | EngineError::MetadataNotWritable { name, .. }
        | EngineError::PriorityNotWritable { name, .. }
        | EngineError::ContentNotWritable { name, .. }
        | EngineError::UpdateNotWritable { name, .. } => {
            ("not-writable".to_owned(), configured(name), None)
        }
        EngineError::NoSuchItem { .. }
        | EngineError::NoSuchTask { .. }
        | EngineError::NoSuchProject { .. }
        | EngineError::NoSuchDocument { .. } => ("no-such-item".to_owned(), None, None),
        EngineError::NoSuchComment { .. } => ("no-such-comment".to_owned(), None, None),
        EngineError::GraphGroupNotText { .. } => ("graph-group".to_owned(), None, None),
        EngineError::DependencyCycle { .. } => ("dependency-cycle".to_owned(), None, None),
        EngineError::NotCreatable { name, .. } | EngineError::RenderingNotWritable { name, .. } => {
            ("not-writable".to_owned(), configured(name), None)
        }
        EngineError::AssetNotReferenced { .. } => ("asset-not-referenced".to_owned(), None, None),
        EngineError::AssetNotGiven { .. } => ("asset-not-given".to_owned(), None, None),
        EngineError::AssetGivenTwice { .. } => ("asset-given-twice".to_owned(), None, None),
        EngineError::AssetNotHeld { .. } => ("asset-not-held".to_owned(), None, None),
        EngineError::AssetsUnsupported { name, .. } => {
            ("assets-unsupported".to_owned(), configured(name), None)
        }
        EngineError::MissingAnswers { .. } => ("template-missing-required".to_owned(), None, None),
        EngineError::Template { error } => (error.kind().to_owned(), None, None),
        EngineError::NoStoredAnswers { .. } => ("no-stored-answers".to_owned(), None, None),
        EngineError::NoTemplate { .. } => ("no-template".to_owned(), None, None),
        EngineError::TemplateNotAFile { .. } => ("template-not-a-file".to_owned(), None, None),
        EngineError::MalformedProvenance { .. } => ("malformed-provenance".to_owned(), None, None),
        EngineError::StaleOrigin { .. } => ("stale-origin".to_owned(), None, None),
        EngineError::StaleLink { .. } => ("stale-link".to_owned(), None, None),
        EngineError::TargetTaskWith { .. } => ("target-task-with".to_owned(), None, None),
        EngineError::TargetTaskNeedsOne => ("target-task-needs-one".to_owned(), None, None),
        EngineError::TargetNativeNeedsOne { .. } => {
            ("target-native-needs-one".to_owned(), None, None)
        }
        EngineError::NotATask { .. } => ("not-a-task".to_owned(), None, None),
        EngineError::TargetTaskSource { .. } => ("target-task-source".to_owned(), None, None),
        EngineError::TargetTaskMissing { .. } => ("target-task-missing".to_owned(), None, None),
        EngineError::TargetTaskOrigin { .. } => ("target-task-origin".to_owned(), None, None),
        EngineError::CreateWith { .. } => ("create-with".to_owned(), None, None),
        EngineError::CreateCarried { .. } => ("create-carried".to_owned(), None, None),
        EngineError::NotAMember { .. } => ("not-a-member".to_owned(), None, None),
        EngineError::NoProjectCounterpart { .. } => {
            ("no-project-counterpart".to_owned(), None, None)
        }
        EngineError::UnrecordedMember { .. } => ("unrecorded-member".to_owned(), None, None),
        EngineError::Misrouted { .. } => ("misrouted".to_owned(), None, None),
        EngineError::SplitPrivateHome { .. } => ("split-private-home".to_owned(), None, None),
        EngineError::ProjectionRefused { .. } => ("projection-refused".to_owned(), None, None),
        EngineError::NotPrivateDestination { destination, .. } => (
            "not-private-destination".to_owned(),
            configured(destination),
            None,
        ),
        EngineError::NotPrivateDestinationPredates { .. } => {
            ("not-private-destination".to_owned(), None, None)
        }
        EngineError::BoundaryRefusedPredates { .. } => ("boundary-refused".to_owned(), None, None),
        EngineError::SanitizeDryRun { .. } => ("sanitize-dry-run".to_owned(), None, None),
        EngineError::SanitizeConflict { .. } => ("sanitize-conflict".to_owned(), None, None),
        EngineError::TermsUnlisted { .. } => ("terms-unlisted".to_owned(), None, None),
        EngineError::TermsUndecided { .. } => ("boundary-unavailable".to_owned(), None, None),
        EngineError::DestinationNotPrivate { destination, .. } => (
            "destination-not-private".to_owned(),
            configured(destination),
            None,
        ),
        EngineError::VisibilityUnreadable {
            destination, error, ..
        } => (
            "visibility-unreadable".to_owned(),
            configured(destination),
            Some(error),
        ),
        EngineError::Oversize { items } => (
            "oversize".to_owned(),
            match items.as_slice() {
                [item] => Some(item.destination.clone()),
                _ => None,
            },
            None,
        ),
        EngineError::BoundaryRefused { destination, .. } => {
            ("boundary-refused".to_owned(), configured(destination), None)
        }
        EngineError::BoundaryUnavailable { destination, .. } => (
            "boundary-unavailable".to_owned(),
            configured(destination),
            None,
        ),
        EngineError::PrivateReference { destination, .. } => (
            "private-reference".to_owned(),
            configured(destination),
            None,
        ),
        EngineError::DesignatorNotConfigured { .. } => {
            ("designator-not-configured".to_owned(), None, None)
        }
        EngineError::PrivateReferenceContent { destination, .. } => (
            "private-reference-content".to_owned(),
            configured(destination),
            None,
        ),
        EngineError::PrivateItemReference { destination, .. } => (
            "private-reference".to_owned(),
            configured(destination),
            None,
        ),
        EngineError::ReferenceUnclassified { destination, .. } => (
            "reference-unclassified".to_owned(),
            configured(destination),
            None,
        ),
        EngineError::ProjectUnclassified { .. } => ("project-unclassified".to_owned(), None, None),
        EngineError::PrivateMemberOfPublicProject { .. } => {
            ("private-member".to_owned(), None, None)
        }
        EngineError::ProjectionReference { .. } => ("projection-reference".to_owned(), None, None),
        EngineError::ProjectionsMalformed { .. } => {
            ("projections-malformed".to_owned(), None, None)
        }
        EngineError::ProjectionNotMirrored { .. } => {
            ("projection-not-mirrored".to_owned(), None, None)
        }
        EngineError::DestinationUnavailable { name, error }
        | EngineError::SourceRefused { name, error }
        | EngineError::SourceUnavailable { name, error }
        | EngineError::SourceFailed { name, error } => {
            (source_kind(error), configured(name), Some(error))
        }
        EngineError::CopyNotUndone { error, .. }
        | EngineError::CopyDestinationUncertain { error, .. } => cause(error),
    }
}

impl From<&EngineError> for Failure {
    fn from(error: &EngineError) -> Self {
        let (kind, source, caused_by) = cause(error);
        Self::caused(kind, source, caused_by, error.to_string())
    }
}

impl From<&TaskAssetsError> for Failure {
    /// Classed by what stopped the read, and written as the read's own message, which names
    /// the task as well as that cause.
    fn from(error: &TaskAssetsError) -> Self {
        let (kind, source, caused_by) = cause(&error.error);
        Self::caused(kind, source, caused_by, error.to_string())
    }
}

impl From<&ConfigError> for Failure {
    /// A configuration this product will not run on. No source caused it — a plugin
    /// refusing its own block is refused here, at load, before any source is built.
    fn from(error: &ConfigError) -> Self {
        let kind = match error {
            ConfigError::Read { .. } => "config-read",
            ConfigError::Syntax { .. } => "config-syntax",
            ConfigError::Setting { .. } => "config-setting",
        };
        Self::decided(kind, error.to_string())
    }
}
