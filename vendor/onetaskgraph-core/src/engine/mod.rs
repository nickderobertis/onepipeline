//! The query engine: fan-out, capability compensation, and the plan it reports.
//!
//! Given the sources a configuration resolved to and a query, the engine addresses one,
//! several or every one of them **at once**, and decides per source and per predicate
//! whether to push the predicate down or to apply it itself. A predicate a source
//! declares [`Native`](Support::Native) is passed in the query and never re-applied. A
//! predicate it declares [`Unsupported`](Support::Unsupported) is **removed** from what
//! that source sees — the source would ignore it anyway, and leaving it in would invite
//! a source to half-apply it — and the engine narrows the wider result set in memory.
//!
//! What makes that worth the machinery is that the two are not the same plan. A source
//! with real server-side search keeps it, and a folder of Markdown beside it is
//! compensated for, and the caller can see which of the two it got: every response
//! carries a [`QueryPlan`], `--explain` renders it, and `--json` publishes it.
//!
//! Nothing here writes anything down. See [`fetch`] for the walk that makes that true.

mod all_pages;
mod assets;
pub(crate) mod assignees;
mod attestation;
pub(crate) mod batch;
pub mod boundary;
mod cached;
mod closure;
mod comment;
mod comments_since;
mod copy;
mod delivery;
mod dependents;
mod designators;
pub use dependents::Completeness;
mod fetch;
mod graph;
mod held_off;
mod held_version;
mod join;
mod local;
mod metadata;
mod narrow;
mod plan_graph;
mod projection;
pub(crate) mod relations;
mod rendered;
mod resume;
mod stored;
mod sweep;
pub(crate) use sweep::schemas as sweep_schemas;
mod update;

use std::collections::BTreeMap;
use std::num::NonZeroU32;
use std::sync::atomic::{AtomicU32, Ordering};

use chrono::{DateTime, Utc};
use onetaskgraph_plugin_api::{
    Capabilities, Cursor, DependencyEdge, Direction, Document, DocumentQuery, Label, LabelFilter,
    MetadataMatch, MetadataRecord, NativeId, Page, PageRequest, Priority, Project, ProjectFilter,
    ProjectQuery, Repository, SecretResolver, SharedClock, SourceError, SourceName, StatusCategory,
    Task, TaskQuery, TextFields, TextQuery, system_clock,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::GlobalId;
use crate::config::{Config, Placement, Routes};
use crate::plan::{
    PageToken, Predicate, QueryPlan, QueryResponse, SourceFailure, SourcePlan, Unanswered,
};
use crate::resolve::{ResolvedSource, UnavailableSource, resolve_available_with_clock};

pub use sweep::{
    FieldOutcome, ItemOutcome, SweepCoverage, SweepField, SweepItem, SweepKind, SweepReport,
    SweepRequests, Uncovered,
};

use fetch::{Fetched, Stream, fits, merge, unrepeated, walk};
use join::join_all;
use local::{LocalDocuments, LocalProjects, LocalTasks};
pub(crate) use resume::{Owed, Resumption, StreamState};
use resume::{Resume, StreamKind};

pub use assets::{HeldAsset, TaskAssetNames, TaskAssetsError};
pub use assignees::{AssigneeCondition, TaskAssigneeSet};
pub use attestation::AttestRequest;
pub use batch::{
    AddedComment, BATCH_SCHEMA_VERSION, BatchOutcome, CommentAddResult, CommentBatch,
    CommentBatchItem, CommentsAdded, FindAnswer, FindIn, FindQuery, FindRequest, FoundTasks,
    TasksUpdated, UpdateBatch, UpdateBatchItem, UpdateResult,
};
pub use closure::{
    CompleteAction, IssueCompleted, IssuesCompleted, ReconcileOutcome, StatusReconciled,
    StatusesReconciled,
};
pub use comment::{
    CommentList, CommentWritten, DeletedComment, DocumentDetail, TaskDetail, TaskDetails,
};
pub use comments_since::{
    COMMENTS_SINCE_SCHEMA_VERSION, CommentActivity, CommentsSince, CommentsSinceRequest,
};
pub use copy::MirrorTarget;
pub(crate) use copy::spent_between;
pub use copy::{
    BudgetSpent, CommentCounts, CopyAction, CopyConditions, CopyItems, CopyLink, CopyLookup,
    CopyOutcome, CopyReport, CopyRequest, CopyScope, CopyVia, CreatesIn, LocateLookup,
    LocateReport, LocateRequest, Located, LocatedCounterpart, MatchBy, NoCounterpart, NotATask,
    Otherwise, Oversized, PriorityCondition, PublicProjection, PublicProjections, Spent,
    StatusCondition, StatusPolicy,
};
pub(crate) use copy::{malformed_links, malformed_locations};
pub use delivery::{Delivered, DeliveryOutcome, ProjectStatusSet, TaskStatusSet, settled};
pub use graph::{
    GraphDirection, GraphEdge, GraphNode, GroupKey, PROJECT_GRAPH_SCHEMA_VERSION, ProjectGraph,
    ProjectGraphRequest, label as graph_label,
};
pub use local::ProjectSelector;
pub use metadata::MetadataSet;
pub use narrow::{GuardOutcome, PriorityGuard, TaskContentSet, TaskPrioritySet};
pub use plan_graph::{PlanCreated, PlanMember, PlannedMember, PlannedProject};
pub use relations::{
    LinkBatch, LinkItem, LinkOp, LinkOutcome, LinkResult, LinksWritten, OtherEnd, ParentBatch,
    ParentEdge, ParentOutcome, ParentResult, ParentsSet, RELATIONS_SCHEMA_VERSION, ReadLink,
    RelationsItem, TaskRelations,
};
pub use rendered::{
    Body, DocumentCreate, ProjectCreate, Regenerated, Regeneration, RenderRequest, RenderTemplate,
    RenderedRecord, TaskCreate, TaskCreated, TemplateAnswers, UnusedAnswers,
};
pub use update::TaskUpdated;

/// One item, under the qualified id the engine addresses it by.
///
/// A plugin only ever deals in its own [`NativeId`]; qualifying one is the engine's job,
/// so this type is the engine's and a plugin never constructs one.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Qualified<T> {
    /// `<source>:<native>`, the form a user types back at the command line.
    pub id: GlobalId,
    /// The item as its source reported it, unchanged.
    pub item: T,
}

/// One dependency edge with both ends qualified.
///
/// An end may belong to another source. The near plugin owns that qualified far id and the
/// engine reports it without resolving or fetching the far source, so it holds no index.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct QualifiedEdge {
    /// The item the edge starts at, and the one that **depends on** the other.
    ///
    /// The direction a caller asked in says which end they named, not which end the edge
    /// starts at: a forward read and the matching reverse read report the same edge.
    pub from: QualifiedEndpoint,
    /// The item the edge points at, and the one that must finish first.
    pub to: QualifiedEndpoint,
    /// What the edge means.
    pub kind: onetaskgraph_plugin_api::DependencyKind,
}

/// One typed, qualified endpoint in an engine dependency response.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct QualifiedEndpoint {
    /// `<source>:<native>`, preserved when a plugin reports another source.
    pub id: GlobalId,
    /// Whether this endpoint names a task or project.
    pub kind: onetaskgraph_plugin_api::ItemKind,
}

impl std::fmt::Display for QualifiedEndpoint {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.id.fmt(formatter)
    }
}

/// One hit of a search that may cross entities.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum SearchHit {
    /// A task matched.
    Task(Qualified<Task>),
    /// A project matched.
    Project(Qualified<Project>),
}

/// Which entities a search covers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum SearchKind {
    /// Tasks only.
    Tasks,
    /// Projects only.
    Projects,
    /// Both, interleaved.
    #[default]
    Both,
}

/// What one configured source is, as `sources list` reports it.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct SourceListing {
    /// The name the configuration gave it.
    pub source: SourceName,
    /// The plugin kind behind it.
    ///
    /// A `String` because the vocabulary is open, not because it was not thought about:
    /// this is the kind a source reports, and a subprocess-hosted plugin reports one
    /// arriving over the wire from a binary this workspace never compiled. No
    /// compile-time enumeration can hold that, and a newtype over the same string would
    /// only move where an unrelated value is accepted.
    // llmlint: ignore[invalid_states_unrepresentable] the reason above, and the one
    // recorded for the same field of `SourcePlan` in plan.rs: `kind` is an open
    // vocabulary a subprocess plugin extends at run time, and `SourcePlan.kind: String`
    // is approved contract text this field is rendered beside.
    pub kind: String,
    /// Whether it built, and what it can do if it did.
    #[serde(flatten)]
    pub state: SourceState,
}

/// Whether a configured source is answering.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
#[serde(tag = "state", rename_all = "kebab-case")]
pub enum SourceState {
    /// The source built, and declares this.
    Available {
        /// What it applies itself.
        capabilities: Capabilities,
    },
    /// The source could not be built at all.
    Unavailable {
        /// Why not.
        error: SourceError,
    },
}

/// Which page of a result set the caller wants.
#[derive(Debug, Clone, PartialEq)]
pub struct Paging {
    /// The most items to return.
    pub limit: NonZeroU32,
    /// Where to resume, or `None` to start at the beginning.
    pub token: Option<PageToken>,
}

/// The filters every list verb shares.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Filters {
    /// Free-text search, when the caller asked for one.
    pub text: Option<TextQuery>,
    /// Label membership, by name.
    pub labels: LabelFilter,
    /// Status categories to keep. Empty means unfiltered.
    pub statuses: Vec<StatusCategory>,
}

/// A request for a page of tasks.
#[derive(Debug, Clone, PartialEq)]
pub struct TaskRequest {
    /// Which sources to address. Empty means the configuration's own selection.
    pub sources: Vec<SourceName>,
    /// What to keep.
    pub filters: Filters,
    /// Which project the tasks belong to.
    pub project: ProjectSelector,
    /// Priorities to keep: a task matches when its priority is any one of these. Empty means
    /// unfiltered.
    ///
    /// Here rather than in [`Filters`], because a project has no priority: the filters a
    /// project list shares with a task list are the ones both kinds of item carry.
    pub priorities: Vec<Priority>,
    /// Comment activity to keep: a task matches when one of its comments was created, or last
    /// edited, at or after this instant. `None` means unfiltered.
    ///
    /// Here rather than in [`Filters`] for the reason [`priorities`](Self::priorities) is: a
    /// project has no comments. A source that does not apply it natively is narrowed by a read
    /// of its comments for every task its other predicates kept.
    pub commented_since: Option<DateTime<Utc>>,
    /// Caller-defined metadata values to keep: a task matches when it holds every one.
    /// Empty means unfiltered.
    ///
    /// Here rather than in [`Filters`] for the reason [`priorities`](Self::priorities) is:
    /// the predicate is a task query's alone.
    pub metadata: Vec<MetadataMatch>,
    /// The copy origin to keep: a task matches when it was copied from this item. `None`
    /// means unfiltered.
    pub origin: Option<GlobalId>,
    /// With a [`ProjectSelector::Qualified`] project, whether the tasks of every member
    /// project that project names at `onetaskgraph.members` are read beside its own, each
    /// under its own source. Ignored for any other selector.
    ///
    /// The members are learned by reading the home on every request — never kept — so a
    /// member added between two pages is a different query, and its token is refused.
    // llmlint: ignore[invalid_states_unrepresentable] The field's name, type and default are the contract this task states (R5: "`TaskRequest` gains `include_members: bool` (default `false`)"), which the downstream engine reads plans with; the combination with any other selector is refused at the command line (`--members` requires a qualified `--project`) and documented here as ignored.
    pub include_members: bool,
    /// Which page.
    pub paging: Paging,
}

/// A request for a page of projects.
#[derive(Debug, Clone)]
pub struct ProjectRequest {
    /// Which sources to address. Empty means the configuration's own selection.
    pub sources: Vec<SourceName>,
    /// What to keep.
    pub filters: Filters,
    /// Which page.
    pub paging: Paging,
}

/// The filters a document list carries.
///
/// Its own type rather than [`Filters`], and the difference is the whole of it: there are
/// no statuses here. A document is not work and carries no status, so a status filter has
/// nothing to compare against — and a shared type with a `statuses` field would let a
/// caller write one down and leave every reader to decide what it meant.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DocumentFilters {
    /// Free-text search, when the caller asked for one.
    pub text: Option<TextQuery>,
    /// Label membership, by name.
    pub labels: LabelFilter,
}

/// A request for a page of documents.
#[derive(Debug, Clone)]
pub struct DocumentRequest {
    /// Which sources to address. Empty means the configuration's own selection.
    pub sources: Vec<SourceName>,
    /// What to keep.
    pub filters: DocumentFilters,
    /// Which project the documents live in.
    pub project: ProjectSelector,
    /// Which page.
    pub paging: Paging,
}

/// A request for a page of labels.
#[derive(Debug, Clone)]
pub struct LabelRequest {
    /// Which sources to address. Empty means the configuration's own selection.
    pub sources: Vec<SourceName>,
    /// Which page.
    pub paging: Paging,
}

/// A request for a page of search hits.
#[derive(Debug, Clone)]
pub struct SearchRequest {
    /// Which sources to address. Empty means the configuration's own selection.
    pub sources: Vec<SourceName>,
    /// What to look for, and where.
    pub text: TextQuery,
    /// Which entities to cover.
    pub kind: SearchKind,
    /// Which page.
    pub paging: Paging,
}

/// A request for a page of one item's dependency edges.
#[derive(Debug, Clone)]
pub struct DependencyRequest {
    /// The qualified item to walk from.
    pub id: GlobalId,
    /// Which way to walk.
    pub direction: Direction,
    /// What a task's read owes when a source declared private could not be asked what it
    /// records of the task: every other edge, with that source in
    /// [`QueryResponse::unanswered`], or that source in [`QueryResponse::errors`].
    pub completeness: Completeness,
    /// Which page.
    pub paging: Paging,
}

/// What a copy found another attempt doing with the intent to create an item's counterpart.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CreateHolder {
    /// Holding it under a live lease, which ends at this instant.
    Lease {
        /// When the other attempt's lease ends.
        until: chrono::DateTime<chrono::Utc>,
    },
    /// Having created the counterpart and linked it, since this copy planned to create one.
    Linked {
        /// The counterpart the other attempt linked.
        counterpart: GlobalId,
    },
    /// Having taken it over and then settled or released it, so it no longer names this copy.
    Released,
}

impl std::fmt::Display for CreateHolder {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Lease { until } => {
                let until = until.to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
                write!(
                    formatter,
                    "is being created at its destination by another copy, whose lease runs \
                     until {until}\nnext: let that copy finish, or copy it again once {until} \
                     has passed."
                )
            }
            Self::Linked { counterpart } => write!(
                formatter,
                "was created at its destination by another copy, which linked {counterpart}\n\
                 next: copy it again, which updates {counterpart}."
            ),
            Self::Released => write!(
                formatter,
                "is no longer being created by this copy: another copy took its create over\n\
                 next: copy it again, which finds whatever that copy made."
            ),
        }
    }
}

/// `ids` joined for a message, in their order.
fn joined(ids: &[GlobalId]) -> String {
    ids.iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(", ")
}

/// What the engine refuses before it asks any source.
///
/// Distinct from a [`SourceFailure`], which is one source failing while the others
/// answer: everything here means the request itself cannot be run at all.
///
/// `Eq` is deliberately absent: three variants carry the [`SourceError`] the source
/// itself gave, which is what a user has to act on, and that type is `PartialEq` alone.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum EngineError {
    /// A `--source` named something no configuration configures.
    #[error(
        "no source named {name:?} is configured\n\
         next: name one of the configured sources ({configured}), or add {name:?} under \
         `sources` — `onetaskgraph sources list` shows what this configuration has."
    )]
    UnknownSource {
        /// The name that was asked for.
        name: String,
        /// The names that exist, for the message.
        configured: String,
    },

    /// A command named a source that some configuration layer refines — its `routes`, a
    /// `config` field — but that no layer read here declares a `plugin` for, so it is not
    /// configured: by `--source`, a qualified id, `default_sources` or a route it follows.
    #[error(
        "source \"{name}\" is not configured here: no configuration layer declares its plugin\n\
         next: declare `sources.{name}.plugin` in a layer this directory reads, or name \
         another source — `onetaskgraph config show` lists the layer each of \"{name}\"'s \
         settings came from."
    )]
    IncompleteSource {
        /// The name that was asked for.
        name: SourceName,
    },

    /// A `--page` token that decodes but does not belong to this query.
    #[error(
        "{message}\n\
         next: page with a token exactly as the previous page reported it, and against \
         the same configuration — or drop `--page` to start the walk again."
    )]
    Token {
        /// What the token claims that this configuration cannot honour.
        message: String,
    },

    /// Nothing at all is configured, so there is nothing to ask.
    #[error(
        "no sources are configured\n\
         next: add one under `sources` in onetaskgraph.yaml — `onetaskgraph schema` \
         prints what each plugin accepts."
    )]
    NoSources,

    /// A private item was to be written to a source not declared private.
    #[error(
        "{item} is private, and source {destination} is not declared private (it is \
         declared {declared}); a private item is written only to a source declared private \
         whose backend agrees\n\
         next: write it to a source declared `visibility: private`, give {destination} a \
         `{{classification: private, to: <source>}}` route to one, or declare {destination} \
         private if its backend is."
    )]
    NotPrivateDestination {
        /// How the write named the item.
        item: String,
        /// The configured name of the destination.
        destination: String,
        /// What its configuration declares.
        declared: crate::config::SourceVisibility,
    },

    /// A narrow write to an item a source not declared private already holds, whose stored
    /// record was already private — its `repositories` already naming a private repository —
    /// before this write, which changes none of them.
    #[error(
        "{item} is private, and source {destination} is not declared private (it is \
         declared {declared}); a private item is written only to a source declared private \
         whose backend agrees. The item's `repositories` already named a private repository \
         before this write, and replacement cannot fix `repositories`\n\
         next: remove that repository from the item's `repositories`, or move the item to a \
         source declared private."
    )]
    NotPrivateDestinationPredates {
        /// How the write named the item, as screened.
        item: String,
        /// The configured name of the destination, as screened.
        destination: String,
        /// What its configuration declares.
        declared: crate::config::SourceVisibility,
    },

    /// A source declared private answered, at the write, that it is not.
    #[error(
        "source {destination} is declared private, but its backend reads {reality} for \
         {item}, so nothing was written\n\
         next: make that backend private — for a GitHub board, both the project and the \
         repository its issues are created in — or correct the source's `visibility`."
    )]
    DestinationNotPrivate {
        /// How the write named the item.
        item: String,
        /// The configured name of the destination.
        destination: String,
        /// What its backend answered.
        reality: onetaskgraph_plugin_api::Visibility,
    },

    /// A source declared private could not say, at the write, who can read it.
    #[error(
        "the visibility of source {destination} could not be read for {item}, and a \
         visibility is never guessed, so nothing was written: {error}\n\
         next: give the source's own credential what reading its visibility needs, then \
         write again."
    )]
    VisibilityUnreadable {
        /// How the write named the item.
        item: String,
        /// The configured name of the destination.
        destination: String,
        /// Why the read failed.
        error: SourceError,
    },

    /// A copy would write an item whose body is larger than its destination holds, so it
    /// wrote nothing: every item of it was sized before the first write.
    #[error(
        "nothing was copied: {}\n\
         next: shorten {} — or split what it says across more items — and copy again; \
         `--dry-run` reports every item's body size beside its destination's limit.",
        oversized(items),
        if items.len() == 1 { "that item" } else { "those items" }
    )]
    Oversize {
        /// Every item over its destination's limit, in the order the copy planned them.
        items: Vec<copy::Oversized>,
    },

    /// The caller's check refused a write to a destination not verified private.
    #[error(
        "{item} was not written to source {destination}, which is not verified private: \
         {reason}\n\
         next: take what names a private repository out of it, or write it to a source \
         declared private."
    )]
    BoundaryRefused {
        /// How the write named the item.
        item: String,
        /// The configured name of the destination.
        destination: String,
        /// The check's neutral reason.
        reason: String,
    },

    /// A write to an item a source not declared private already holds was refused for a term
    /// that was already on the item before this write, and that sanitising the item could not
    /// fix.
    #[error(
        "{item} was not written to source {destination}, which is not verified private: \
         {reason}. The term was already on the item before this write, and replacement cannot \
         fix it in {unfixable}\n\
         next: take it out of the item by hand — or declare a replacement for it and write \
         again — then repeat this write."
    )]
    BoundaryRefusedPredates {
        /// How the write named the item, as screened.
        item: String,
        /// The configured name of the destination, as screened.
        destination: String,
        /// The policy's neutral reason.
        reason: String,
        /// The kinds of field replacement cannot fix.
        unfixable: boundary::UnfixableKinds,
    },

    /// `--sanitize-dry-run`: a write that would have sanitised its item first wrote nothing.
    #[error(
        "nothing was written: sanitising {item} first would change what the sanitization lists, \
         and the write would then {}\n\
         next: run it again without --sanitize-dry-run to sanitise the item and write.",
        if *would_pass { "pass" } else { "still be refused" }
    )]
    SanitizeDryRun {
        /// How the write named the item, as screened.
        item: String,
        /// Whether the write would pass once the item was sanitised.
        would_pass: bool,
    },

    /// `sources sanitize` was asked of a store whose policy cannot list its terms.
    #[error(
        "source {name} cannot be swept: the policy cannot list the private terms to search \
         for ({why})\n\
         next: configure `write_policy.terms_command` — for example [onevcs, boundary, terms, \
         --input, \"-\"] — and sweep again."
    )]
    TermsUnlisted {
        /// The source asked about.
        name: String,
        /// Why, in neutral words.
        why: String,
    },

    /// `sources sanitize` was asked of a store whose terms command could not decide.
    #[error(
        "source {name} was not swept: the policy could not decide the private terms to search \
         for ({why})\n\
         next: repair what `write_policy.terms_command` reports, then sweep again."
    )]
    TermsUndecided {
        /// The source asked about.
        name: String,
        /// Why, in neutral words.
        why: String,
    },

    /// A sanitising write found its item moved twice since it was read, and wrote nothing.
    #[error(
        "{item} changed twice while it was being sanitised, so nothing was overwritten\n\
         next: read it again and repeat the write."
    )]
    SanitizeConflict {
        /// How the write named the item, as screened.
        item: String,
    },

    /// The caller's check could not decide about a write to a destination not verified
    /// private, which is never a pass.
    #[error(
        "the public boundary check is unavailable, so {item} was not written to source \
         {destination}: {reason}\n\
         next: configure `write_policy.check_command`, or repair what it reports, then write \
         again."
    )]
    BoundaryUnavailable {
        /// How the write named the item.
        item: String,
        /// The configured name of the destination.
        destination: String,
        /// The check's neutral reason.
        reason: String,
    },

    /// A write to a source not declared private would name an item of one that is.
    #[error(
        "{item} names an item of source {named}, which is declared private, and source \
         {destination} is not; a reference to a private source is never written where it \
         would name it, so nothing was written\n\
         next: copy what it names along with it, write it to a source declared private, or \
         take the dependency or the delivery that names it off first."
    )]
    PrivateReference {
        /// How the write named the item.
        item: String,
        /// The configured name of the destination.
        destination: String,
        /// The private source a reference names.
        named: String,
    },

    /// A write to a source not declared private would name a private item by something other
    /// than its native id: a prefixed human key, a URL or a slug, or an endpoint object
    /// carrying a key beyond `designator`, `id` and `kind`. Only a native id can be written
    /// where a private item must not be named, in the opaque form, because only it conveys
    /// nothing; this refusal names the form, never what it held.
    #[error(
        "{item} cannot be written to source {destination}: it names a private item by {form}, \
         which would carry what that item is called where it must not be named, so nothing was \
         written\n\
         next: name the item by its qualified native id, `<source>:<native id>`, which is \
         written as an opaque designator and id, or take the dependency off."
    )]
    PrivateReferenceContent {
        /// How the write named the item being written.
        item: String,
        /// The configured name of the destination.
        destination: String,
        /// The form of the reference — never its value.
        form: boundary::ReferenceForm,
    },

    /// A dependency read asked for every edge, and one is recorded under a designator no source
    /// this host configures is.
    #[error(
        "{id} depends on an item recorded under designator {designator}, and no source this host \
         configures is that tracker, so the answer would lack it; the read was refused as \
         --complete asks\n\
         next: configure the tracker that designator names — its plugin and, for a folder, the \
         same public_alias — or read without --complete to have it named in `unanswered`."
    )]
    DesignatorNotConfigured {
        /// The item read.
        id: String,
        /// The designator no configured source is.
        designator: onetaskgraph_plugin_api::Designator,
    },

    /// A write to a source not declared private would name an item classified private, though
    /// its source is not declared private.
    #[error(
        "{item} names {reference}, which is private, and source {destination} is not declared \
         private; a reference to a private item is never written where it would name it, so \
         nothing was written\n\
         next: write it to a source declared private, or take the dependency or the delivery \
         that names it off first."
    )]
    PrivateItemReference {
        /// How the write named the item.
        item: String,
        /// The configured name of the destination.
        destination: String,
        /// The private item a reference names, qualified.
        reference: String,
    },

    /// A write to a source not declared private would name an item whose classification
    /// cannot be established.
    #[error(
        "{item} names {reference}, whose classification could not be established ({why}), and \
         source {destination} is not declared private; an id that might name a private item is \
         never written where it would name it, so nothing was written\n\
         next: configure that item's source and make it readable, copy it along, take the \
         reference off, or write to a source declared private."
    )]
    ReferenceUnclassified {
        /// How the write named the item.
        item: String,
        /// The reference, qualified.
        reference: String,
        /// The configured name of the destination.
        destination: String,
        /// Why its classification could not be established.
        why: String,
    },

    /// While the boundary is active, an item was filed under a project whose classification
    /// cannot be established, so it could not be written anywhere not declared private.
    #[error(
        "project {project} could not be classified ({why}), and while the boundary is active \
         an item filed under it is never read as public, so nothing was written\n\
         next: make that project readable at its source, file the item under a project its \
         source holds, or write to a source declared private."
    )]
    ProjectUnclassified {
        /// The project, qualified.
        project: String,
        /// Why its classification could not be established.
        why: String,
    },

    /// A public projection's content links to a local path, or carries raw HTML that could
    /// name one, which the view could not carry.
    #[error(
        "{task} is public, and its content carries {reference:?}, a local link or raw HTML its \
         public projection cannot carry; a projection never carries a path of the private \
         store, nor HTML whose destination it cannot read, so nothing was written\n\
         next: link to a URL in Markdown instead, attach the file as an image asset referenced \
         as `./<name>`, or take it out of the task."
    )]
    ProjectionReference {
        /// The canonical task, qualified.
        task: crate::GlobalId,
        /// The link's destination, as the content spells it.
        reference: String,
    },

    /// A private project's `onetaskgraph.public_projections` map is not the shape this engine
    /// reads.
    #[error(
        "the public projection map of {project} cannot be read ({why}), so nothing was \
         written\n\
         next: put that project's `onetaskgraph.public_projections` back as a copy wrote it, \
         or remove it to let the next copy record it afresh."
    )]
    ProjectionsMalformed {
        /// The private project, qualified.
        project: crate::GlobalId,
        /// What is wrong with it.
        why: String,
    },

    /// A canonical task's status was set, and its public projection's could not be.
    #[error(
        "the status of {task} was set, but its public projection {projection} could not be \
         given it ({why})\n\
         next: set the status of {task} again once {projection}'s source answers, or copy its \
         project again, which brings every projection in step."
    )]
    ProjectionNotMirrored {
        /// The canonical task, qualified.
        task: crate::GlobalId,
        /// The view it has, or the map naming it when that could not be read — boxed, so every
        /// other error is not as large as this one.
        projection: Box<copy::MirrorTarget>,
        /// Why its write failed.
        why: String,
    },

    /// A private item was to be filed under a project held public.
    #[error(
        "{item} is private, and project {project} it would be filed under is public; a \
         public project never holds a private member, so nothing was written\n\
         next: classify the project private first — `onetaskgraph project create --id \
         <id> --classification private` over it — or file the item under a private project."
    )]
    PrivateMemberOfPublicProject {
        /// How the write named the item.
        item: String,
        /// The project, qualified.
        project: String,
    },

    /// A copy named a destination whose plugin has no write side.
    #[error(
        "source {name} cannot be written: its plugin is {kind}, which has no write \
         side\n\
         next: copy into a source whose plugin can be written — `onetaskgraph sources \
         list` reports each one's plugin."
    )]
    NotWritable {
        /// The configured name of the destination.
        name: String,
        /// The plugin behind it.
        kind: String,
    },

    /// A document read or write named a source whose plugin has no documents.
    ///
    /// The same shape [`NotWritable`](Self::NotWritable) has, and for the same reason: the
    /// declaration is read once at the handshake, so a source that says it has no
    /// documents is refused before anything is read rather than asked and then apologised
    /// for.
    #[error(
        "source {name} has no documents: its plugin is {kind}, which holds none\n\
         next: name a source whose plugin has documents — `onetaskgraph sources list` \
         reports each one's plugin and what it declares."
    )]
    NoDocuments {
        /// The configured name of the source.
        name: String,
        /// The plugin behind it.
        kind: String,
    },

    /// `task copy --expect` named more than one task, a query, or something other than a task.
    #[error(
        "--expect names the version of one destination task, so it is taken by a copy of one \
         task alone\n\
         next: copy that one task by its id with --expect, and the others without it"
    )]
    ExpectOneTask,

    /// A batch document — a `task find` request, a comment batch or an update batch — this
    /// release cannot read, refused whole before anything is read or written.
    #[error(
        "{what} cannot be read: {why}, so nothing was read or written\n\
         next: fix it and run the command again; `onetaskgraph schema` describes every batch \
         document"
    )]
    BatchDocument {
        /// Which document, or which part of it.
        what: String,
        /// What is wrong with it.
        why: String,
    },

    /// A `task find` query narrowed by none of `text`, `metadata` and `origin`, which would
    /// answer every task of its source.
    #[error(
        "query {key:?} is narrowed by none of text, metadata and origin, so it would answer \
         every task of the source; nothing was read\n\
         next: give it a text, a metadata value or an origin, or list the source with \
         `onetaskgraph task list --all`"
    )]
    UnnarrowedQuery {
        /// The query's key.
        key: String,
    },

    /// `task relations --complete` read an item whose children were not read whole.
    #[error(
        "the relations of {items} were not read whole, so --complete refuses the answer\n\
         next: drop --complete to take what was read, with each item's `children_complete` \
         saying which are partial"
    )]
    RelationsIncomplete {
        /// The items read in part.
        items: String,
    },

    /// `task comments --item` named a task of a source other than the one listed.
    #[error(
        "{item} is not a task of source {source_name}, whose comments this lists\n\
         next: name only tasks of {source_name} with --item, or list the other source's comments \
         on their own"
    )]
    ItemOutsideSource {
        /// The qualified id named.
        item: String,
        /// The source listed.
        source_name: String,
    },

    /// `task list --all` was answered a page token it had already followed: a source repeating
    /// a cursor, which a walk to exhaustion would otherwise ask for forever.
    #[error(
        "a page answered the token {token}, which this listing had already followed, so walking \
         every page would never end — a source repeated its cursor\n\
         next: list without --all, a page at a time, and report the repeated cursor to whoever \
         runs that source"
    )]
    RepeatedPage {
        /// The page token answered a second time.
        token: String,
    },

    /// `task copy --if-version` named more than one task, a query, or something other than a
    /// task.
    #[error(
        "--if-version names the version of one destination task, so it is taken by a copy of \
         one task alone\n\
         next: copy that one task by its id with --if-version, and the others without it"
    )]
    IfVersionOneTask,

    /// A write held to a version — a task write, a document's metadata, or an attestation — was
    /// refused because a read of the item answered another: a change made at the source since
    /// the caller's read.
    #[error(
        "{id} is at version {found}, not the version {expected} this write expects — it changed \
         since that version was read — so nothing was written\n\
         next: read it again (`onetaskgraph <task|project|document> show`), decide whether the \
         write still applies, and pass the version that read reports"
    )]
    VersionMoved {
        /// The qualified id of the item.
        id: String,
        /// The version the write expected.
        expected: String,
        /// The version the read answered.
        found: String,
        /// What the item was found holding: its `version` the one found.
        held: Box<crate::cache::WriteResult>,
    },

    /// One member of a plan graph could not be created after the project, and every member
    /// before it, had landed: nothing is taken back, so what landed is named.
    #[error(
        "member {member} of the plan could not be created: {error}\n\
         already created: {}\n\
         next: fix what the member's failure names, then create the members not yet created \
         with `task create`, or remove what landed and create the plan again",
        landed.join(", ")
    )]
    PlanMember {
        /// The member's declared id.
        member: String,
        /// The qualified ids already created — the project first — in the order they landed.
        landed: Vec<String>,
        /// Why the member could not be created.
        error: Box<EngineError>,
    },

    /// `--expect` named a task of a source that keeps no local cache, which is where what an
    /// expected write takes instead of reading lives.
    #[error(
        "source {name} keeps no cache, so a write cannot take what it needs from one\n\
         next: set sources.{name}.cache.enabled to cache it, or write without --expect"
    )]
    NotCached {
        /// The configured name of the source.
        name: String,
    },

    /// A comment verb named a task of a source whose plugin has no comments.
    ///
    /// The shape [`NoDocuments`](Self::NoDocuments) has, for its reason: the declaration is
    /// read once at the handshake, so the source is refused before anything is read.
    #[error(
        "source {name} has no comments: its plugin is {kind}, whose tasks hold none\n\
         next: name a task of a source whose plugin has comments — `onetaskgraph sources \
         list` reports each one's plugin."
    )]
    NoComments {
        /// The configured name of the source.
        name: String,
        /// The plugin behind it.
        kind: String,
    },

    /// A comment verb that writes named a source whose comments can be read but not written.
    #[error(
        "source {name} cannot be written: its plugin is {kind}, whose comments can be read \
         but not added to, edited or removed\n\
         next: list them with `onetaskgraph task comment list`, or comment on a task of a \
         source whose plugin can be written — `onetaskgraph sources list` reports each \
         one's plugin."
    )]
    CommentsNotWritable {
        /// The configured name of the source.
        name: String,
        /// The plugin behind it.
        kind: String,
    },

    /// `task status set` named a source whose plugin has no write side.
    #[error(
        "source {name} cannot write a status: its plugin is {kind}, which has no write side\n\
         next: set the status in that source itself, or name a task of a source whose plugin \
         can be written — `onetaskgraph sources list` reports each one's plugin."
    )]
    StatusNotWritable {
        /// The configured name of the source.
        name: String,
        /// The plugin behind it.
        kind: String,
    },

    /// A metadata verb named a record of a source whose plugin has no write side.
    #[error(
        "source {name} cannot write a {record}'s metadata: its plugin is {kind}, which has no \
         write side\n\
         next: set the key in that source itself, or name a {record} of a source whose plugin \
         can be written — `onetaskgraph sources list` reports each one's plugin."
    )]
    MetadataNotWritable {
        /// The configured name of the source.
        name: String,
        /// The plugin behind it.
        kind: String,
        /// Which kind of record was named.
        record: MetadataRecord,
    },

    /// `task priority set` named a source whose plugin has no write side.
    #[error(
        "source {name} cannot write a priority: its plugin is {kind}, which has no write side\n\
         next: set the priority in that source itself, or name a task of a source whose plugin \
         can be written — `onetaskgraph sources list` reports each one's plugin."
    )]
    PriorityNotWritable {
        /// The configured name of the source.
        name: String,
        /// The plugin behind it.
        kind: String,
    },

    /// `task content set` named a source whose plugin has no write side.
    #[error(
        "source {name} cannot write a task's content: its plugin is {kind}, which has no write \
         side\n\
         next: edit the content in that source itself, or name a task of a source whose plugin \
         can be written — `onetaskgraph sources list` reports each one's plugin."
    )]
    ContentNotWritable {
        /// The configured name of the source.
        name: String,
        /// The plugin behind it.
        kind: String,
    },

    /// `task update` named a source whose plugin has no write side.
    #[error(
        "source {name} cannot update a task: its plugin is {kind}, which has no write side\n\
         next: change the task in that source itself, or name a task of a source whose plugin \
         can be written — `onetaskgraph sources list` reports each one's plugin."
    )]
    UpdateNotWritable {
        /// The configured name of the source.
        name: String,
        /// The plugin behind it.
        kind: String,
    },

    /// A create or a render was given an asset its content does not reference.
    #[error(
        "the {record} was given the asset {asset}, which its content does not reference\n\
         next: reference it in the content as `![<alt>](./{asset})`, or drop that --asset."
    )]
    AssetNotReferenced {
        /// The record being written.
        record: String,
        /// The asset given.
        asset: String,
    },

    /// A create or a render's content references an asset the record would not hold.
    #[error(
        "the {record}'s content references the asset ./{asset}, which the {record} would not \
         hold\n\
         next: give it with --asset <PATH> naming a file called {asset}, or remove the \
         reference."
    )]
    AssetNotGiven {
        /// The record being written.
        record: String,
        /// The asset its content references.
        asset: String,
    },

    /// A create or a render was given two assets under one name.
    #[error(
        "the {record} was given two assets named {asset}\n\
         next: give each asset once — an asset is stored under its file's base name, so \
         rename one of the two files."
    )]
    AssetGivenTwice {
        /// The record being written.
        record: String,
        /// The name given twice.
        asset: String,
    },

    /// A record carrying an image asset was to be written to a source whose plugin stores
    /// none.
    #[error(
        "source {name} cannot store the asset {asset} of {record}: its plugin is {kind}, which \
         declares image assets unsupported\n\
         next: write the record to a source whose plugin stores assets — such as local-md — \
         or remove the reference to ./{asset} from its content."
    )]
    AssetsUnsupported {
        /// The configured name of the destination.
        name: String,
        /// The plugin behind it.
        kind: String,
        /// The record that carries the asset.
        record: String,
        /// The asset.
        asset: String,
    },

    /// A record whose content references an asset it does not hold was to be copied.
    #[error(
        "{record}'s content references the asset ./{asset}, which {record} does not hold\n\
         next: store the asset with the record — `onetaskgraph task render` or `document \
         render` with --asset <PATH> — or remove the reference, then copy again."
    )]
    AssetNotHeld {
        /// The record being copied, by its qualified id.
        record: String,
        /// The asset its content references.
        asset: String,
    },

    /// A human task projection its durable intent, its routing or its tracker evidence does not
    /// authorize — nothing further was sent for it.
    #[error(
        "human task projection {item_id:?} was refused: {message}\n\
         next: {next}"
    )]
    ProjectionRefused {
        /// The request's external idempotency identity.
        item_id: String,
        /// Why it was refused.
        message: String,
        /// What resolves it.
        next: String,
    },

    /// `task create` or `document create` named a source whose plugin has no write side.
    #[error(
        "source {name} cannot create a {record}: its plugin is {kind}, which has no write side\n\
         next: create it in a source whose plugin can be written — `onetaskgraph sources list` \
         reports each one's plugin."
    )]
    NotCreatable {
        /// The configured name of the source.
        name: String,
        /// The plugin behind it.
        kind: String,
        /// Which kind of record was to be created.
        record: MetadataRecord,
    },

    /// `task render`, `project render` or `document render` named a source whose plugin has no
    /// write side.
    #[error(
        "source {name} cannot write a {record}'s rendering: its plugin is {kind}, which has no \
         write side\n\
         next: render it with --dry-run to read the result, or regenerate a {record} of a source \
         whose plugin can be written."
    )]
    RenderingNotWritable {
        /// The configured name of the source.
        name: String,
        /// The plugin behind it.
        kind: String,
        /// A task, a project or a document.
        record: RenderedRecord,
    },

    /// A regenerate left required variables unanswered, because the answers stored beside the
    /// item could not be used or did not cover them.
    #[error(
        "supply every required answer to regenerate {id}: {} unanswered, and {reason}\n\
         next: answer {} with --var NAME=VALUE or an answers file (--answers FILE), or run \
         interactively to be asked.",
        names.join(", "),
        if names.len() == 1 { "it" } else { "each" }
    )]
    MissingAnswers {
        /// The item regenerated.
        id: String,
        /// Every required variable left unanswered, in declaration order.
        names: Vec<String>,
        /// What became of the stored answers, as a clause: why they were not the base, or
        /// that they were and did not answer these.
        reason: String,
    },

    /// A template could not be loaded or rendered, or refused the answers it was given.
    #[error("{error}")]
    Template {
        /// What the template refused.
        error: crate::template::TemplateError,
    },

    /// An answers read named an item with no answers stored beside it.
    #[error(
        "{record} {id} has no stored template answers: {reason}\n\
         next: regenerate it with every required answer (`onetaskgraph {record} render {id} \
         --var NAME=VALUE`), or read its provenance with `onetaskgraph {record} show {id}`."
    )]
    NoStoredAnswers {
        /// A task, a project or a document.
        record: RenderedRecord,
        /// The item.
        id: String,
        /// Why none are there.
        reason: String,
    },

    /// A regenerate named no template, and the item records none.
    #[error(
        "{record} {id} records no template it was rendered from, and none was given\n\
         next: name one with --template FILE or --template-loader FILE."
    )]
    NoTemplate {
        /// A task, a project or a document.
        record: RenderedRecord,
        /// The item.
        id: String,
    },

    /// A regenerate named no template, and the item's `onetaskgraph.template` entry is not one
    /// this product writes, so it names nothing to render.
    #[error(
        "{record} {id} records a template entry this product did not write — {problem}\n\
         next: regenerate it with --template FILE or --template-loader FILE and every required \
         answer, which records a fresh entry."
    )]
    MalformedProvenance {
        /// A task, a project or a document.
        record: RenderedRecord,
        /// The item.
        id: String,
        /// What is wrong with the entry.
        problem: String,
    },

    /// A regenerate named no template, and the one the item records is not a readable file.
    #[error(
        "{record} {id} was rendered from {reference:?}, which is not a readable file, so it \
         cannot be re-read; a recorded reference is never turned into a location\n\
         next: supply the template with --template-loader FILE (a loader document naming what \
         to render), or name a template file with --template FILE."
    )]
    TemplateNotAFile {
        /// A task, a project or a document.
        record: RenderedRecord,
        /// The item.
        id: String,
        /// The provenance `template` it records.
        reference: String,
    },

    /// An assignee was to be written — by `task create --assignee`, `task assignee set`, an
    /// update-many item or a guarded copy — to a source that has not opted in to assignees.
    ///
    /// Refused before that source is asked anything.
    #[error(
        "source {name} does not hold assignees, so {task}'s assignee cannot be written to it: \
         its {kind} configuration does not set assignees\n\
         next: set `assignees: true` under `sources.{name}.config` to opt that source in, or \
         leave the assignee out of the write."
    )]
    NoAssignees {
        /// The configured name of the source that has not opted in.
        name: String,
        /// The plugin behind it.
        kind: String,
        /// The qualified id of the task the write names.
        task: String,
    },

    /// A write would assign an item to more than one person, which no write does: a task copied
    /// while its source holds several assignees, or a library caller's update naming several.
    #[error(
        "{item} holds {count} assignees, and a write assigns one person or none\n\
         next: leave {item} assigned to one person, or copy it over an existing item without \
         --when-destination-assignees so that item keeps its own."
    )]
    SeveralAssignees {
        /// The item whose assignees could not be written.
        item: String,
        /// How many people it holds.
        count: usize,
    },

    /// A priority other than `none` was to be written — by `task priority set` or by a copy —
    /// to a source whose plugin declares it holds no priority.
    ///
    /// Refused before that source is asked anything, so a copy it stops has written nothing.
    #[error(
        "source {name} cannot hold the field priority, so {task}'s priority {priority} cannot \
         be written to it: its plugin is {kind}, which declares priority unsupported\n\
         next: write to a source whose plugin holds a priority — `onetaskgraph sources list` \
         reports what each declares — or set the task's priority to none first; a \
         github-projects source holds one once its configuration sets priority_mapping."
    )]
    NoPriority {
        /// The configured name of the source that cannot hold it.
        name: String,
        /// The plugin behind it.
        kind: String,
        /// The qualified id of the task whose priority it is.
        task: String,
        /// The priority that could not be written.
        priority: onetaskgraph_plugin_api::Priority,
    },

    /// A metadata verb named a project its source does not hold.
    #[error(
        "no project with the id {id}\n\
         next: check the id, or list what is there — `onetaskgraph project list` reports every \
         project the configured sources hold."
    )]
    NoSuchProject {
        /// The qualified id that named nothing.
        id: String,
    },

    /// A metadata verb named a document its source does not hold.
    #[error(
        "no document with the id {id}\n\
         next: check the id, or list what is there — `onetaskgraph document list` reports every \
         document the configured sources hold."
    )]
    NoSuchDocument {
        /// The qualified id that named nothing.
        id: String,
    },

    /// `project graph --group-by` met a task whose value under the key is neither a string
    /// nor `null`, so it names no group a picture could draw.
    #[error(
        "task {task} holds {value} under {key}, which is not a group: a group is a non-empty \
         string, and a task with none holds nothing, null or \"\" there\n\
         next: write a string under {key} on {task} — `onetaskgraph task metadata set {task} \
         {key} '\"<group>\"'` — or group by another key."
    )]
    GraphGroupNotText {
        /// The qualified id of the task holding it.
        task: GlobalId,
        /// The `--group-by` key.
        key: graph::GroupKey,
        /// The value it holds, boxed so this one variant does not widen every error.
        value: Box<serde_json::Value>,
    },

    /// `project graph` met tasks of the project that depend on each other in a cycle, which
    /// no order can put each after what it depends on.
    #[error(
        "the tasks {} depend on each other in a cycle, so they have no order to draw\n\
         next: read each one's edges with `onetaskgraph task deps <ID>` and remove the \
         dependency that closes the cycle.",
        .tasks.iter().map(ToString::to_string).collect::<Vec<_>>().join(", ")
    )]
    DependencyCycle {
        /// The qualified ids of every task the cycle leaves unordered, in order.
        tasks: Vec<GlobalId>,
    },

    /// A comment verb named a task its source does not hold.
    #[error(
        "no task with the id {id}\n\
         next: check the id, or list what is there — `onetaskgraph task list` reports every \
         task the configured sources hold."
    )]
    NoSuchTask {
        /// The qualified id that named nothing.
        id: String,
    },

    /// A comment verb named a comment the task does not have.
    #[error(
        "task {task} has no comment with the id {comment}\n\
         next: list its comments — `onetaskgraph task comment list {task}` reports each \
         one's id."
    )]
    NoSuchComment {
        /// The qualified id of the task.
        task: String,
        /// The comment id that named nothing on it.
        comment: String,
    },

    /// A source the verb reads or writes one item of is configured but could not be built.
    ///
    /// Distinct from [`DestinationUnavailable`](Self::DestinationUnavailable), whose next
    /// action is about a copy.
    #[error(
        "source {name} could not be built: {error}\n\
         next: fix that source — `onetaskgraph sources list` reports its state — then run the \
         command again."
    )]
    SourceUnavailable {
        /// The configured name of the source.
        name: String,
        /// Why it did not build.
        error: SourceError,
    },

    /// A source refused, or failed, the one call a verb made of it.
    ///
    /// Distinct from [`SourceRefused`](Self::SourceRefused), whose next action is about a
    /// copy, and from a [`SourceFailure`], which leaves other sources' results standing: a
    /// comment is one call to one source, so there is nothing else to report beside it.
    #[error(
        "source {name} could not do it: {error}\n\
         next: fix what the source named above, then run the command again."
    )]
    SourceFailed {
        /// The source that failed.
        name: String,
        /// What it said.
        error: SourceError,
    },

    /// A copy named a destination that is configured but could not be built.
    #[error(
        "the destination source {name} could not be built: {error}\n\
         next: fix that source — `onetaskgraph sources list` reports its state — then \
         copy again."
    )]
    DestinationUnavailable {
        /// The configured name of the destination.
        name: String,
        /// Why it did not build.
        error: SourceError,
    },

    /// A copy named an id its own source does not hold.
    #[error(
        "no item with the id {id}\n\
         next: check the id, or list what is there — `onetaskgraph task list` and \
         `onetaskgraph project list` report what the configured sources hold."
    )]
    NoSuchItem {
        /// The qualified id that named nothing.
        id: String,
    },

    /// An item's recorded origin names an item the destination no longer holds.
    ///
    /// Refused rather than created: the destination item was deleted or moved on purpose,
    /// and creating a second one there would duplicate work somebody removed.
    #[error(
        "{item} was copied from {origin}, which that destination no longer holds\n\
         next: re-run with --recreate to create a new item there instead, or restore \
         {origin}."
    )]
    StaleOrigin {
        /// The item being copied.
        item: String,
        /// The origin it records.
        origin: String,
    },

    /// An item's recorded copy link names an item the destination no longer holds.
    ///
    /// Refused rather than created, for the reason [`Self::StaleOrigin`] is: the item the
    /// last copy landed on was deleted at the destination, and creating another would
    /// duplicate work somebody removed.
    #[error(
        "{item} was last copied to {link}, which that destination no longer holds\n\
         next: re-run with --recreate to create a new item there instead, or restore \
         {link}."
    )]
    StaleLink {
        /// The item being copied.
        item: String,
        /// The destination item its `onetaskgraph.copies` entry names.
        link: String,
    },

    /// An explicit target cannot be combined with another target selection mode.
    #[error(
        "--target-task cannot be given with {flag}\nnext: choose one way to select the destination task."
    )]
    TargetTaskWith {
        /// The incompatible option.
        flag: CopyLookup,
    },
    /// An explicit target binds exactly one named source task.
    #[error(
        "--target-task requires exactly one source task id\nnext: name one task, without query selection."
    )]
    TargetTaskNeedsOne,
    /// `task locate --target-native` names one destination item, so it locates one task.
    #[error(
        "--target-native names the counterpart of exactly one task, and {count} were named\n\
         next: name one task beside --target-native, or drop it to look each counterpart up."
    )]
    TargetNativeNeedsOne {
        /// How many tasks were named.
        count: usize,
    },
    /// `task locate` was named an item that is not a task.
    #[error(
        "{id} is a {kind}, not a task, and task locate places tasks alone\n\
         next: name a task's qualified id — `onetaskgraph task list` reports them."
    )]
    NotATask {
        /// The id named.
        id: GlobalId,
        /// What it names instead.
        kind: NotATask,
    },
    /// The target must belong to the configured destination.
    #[error(
        "target task {target} is not in destination {destination}\nnext: name a task in the destination source."
    )]
    TargetTaskSource {
        /// The named task.
        target: GlobalId,
        /// The requested destination.
        destination: SourceName,
    },
    /// The destination no longer holds the named task.
    #[error(
        "target task {target} does not exist\nnext: name an existing destination task whose origin names the source task."
    )]
    TargetTaskMissing {
        /// The missing task.
        target: GlobalId,
    },
    /// The named task does not record the source task as its origin.
    #[error(
        "target task {target} has no origin naming {item}\nnext: choose a destination task whose origin names {item}."
    )]
    TargetTaskOrigin {
        /// The named destination task.
        target: GlobalId,
        /// The source task being copied.
        item: GlobalId,
        /// What the named task held, as the copy's read of it before writing found it.
        held: Option<Box<crate::cache::WriteResult>>,
    },

    /// `--create` was given beside a flag that says how to look for a counterpart, when
    /// `--create` says there is none to look for.
    #[error(
        "--create cannot be given with {flag}: --create asserts the destination holds no \
         counterpart, so there is nothing for {flag} to look for\n\
         next: drop {flag} to create each item without looking, or drop --create to look."
    )]
    CreateWith {
        /// The flag given beside `--create`.
        flag: CopyLookup,
    },

    /// A `--comments` copy into a source whose plugin has no comments, refused before
    /// anything is read: it has nowhere to put the comments it was asked to carry.
    #[error(
        "--comments cannot carry comments into {name}: its plugin is {kind}, whose tasks hold \
         none\n\
         next: copy without --comments, or copy into a source whose plugin has comments — \
         `onetaskgraph sources list` reports each one's plugin."
    )]
    CommentlessDestination {
        /// The configured name of the destination.
        name: String,
        /// The plugin behind it.
        kind: String,
    },

    /// A copy named its items by a query and was asked to copy projects or documents: a query
    /// names tasks, so only a task copy names its items that way.
    #[error(
        "a copy names its items by a query only when it copies tasks\n\
         next: name the projects or documents to copy by their qualified ids."
    )]
    QueryNotTasks,

    /// A copy of projects or documents was asked to keep only the tasks at some status
    /// categories: `--status` narrows the tasks a task copy names, so only a task copy takes it.
    #[error(
        "a copy keeps the tasks at a status only when it copies tasks\n\
         next: copy the projects or documents without --status, or copy the tasks with it."
    )]
    StatusesNotTasks,

    /// A project or a document copy was given a condition on the counterpart it updates:
    /// `--when-destination-status`, `--otherwise` and `--when-destination-priority` condition
    /// a task copy alone.
    #[error(
        "a copy conditions the counterpart it updates only when it copies tasks\n\
         next: copy the projects or documents without --when-destination-status, --otherwise \
         or --when-destination-priority, or copy the tasks with them."
    )]
    ConditionsNotTasks,

    /// Another copy attempt holds, or has settled, the intent to create this item's
    /// counterpart, so this copy wrote nothing more and sent nothing more to the destination.
    #[error("{item} {by}")]
    CreateInProgress {
        /// The source item being copied.
        item: GlobalId,
        /// What the other attempt was found doing.
        by: CreateHolder,
    },

    /// More than one item at the destination carries the key of the create copying this item,
    /// so none of them was linked: a duplicate, reported rather than adopted.
    #[error(
        "{item} has more than one counterpart at {destination} carrying its create key: \
         {}\n\
         next: delete all but one of them at {destination}, then copy {item} again to link \
         the one left.",
        joined(.found)
    )]
    CreateDuplicated {
        /// The source item being copied.
        item: GlobalId,
        /// The destination source.
        destination: SourceName,
        /// Every item carrying the key, qualified, in the order the destination listed them.
        found: Vec<GlobalId>,
    },

    /// A create would file its issue somewhere the destination's own policy on where it
    /// creates refuses — a GitHub Projects source's `creation_owners` — so nothing of it was
    /// written.
    #[error(
        "{item} was not copied: {reason}\n\
         next: name a repository of an allowed owner on {item}, set the source's repository to \
         one, or add the owner to creation_owners on source {destination}."
    )]
    CreationOwner {
        /// The source item whose create was refused.
        item: GlobalId,
        /// The destination source.
        destination: SourceName,
        /// The destination's own reason, naming the repository and the allowed owners.
        reason: String,
    },

    /// `--otherwise` was given without `--when-destination-status`, which is what it is the
    /// other branch of.
    #[error(
        "--otherwise says what a copy does with an item whose destination status is not one \
         --when-destination-status names, and none was named\n\
         next: name the categories with --when-destination-status, or drop --otherwise."
    )]
    OtherwiseWithoutStatus,

    /// `--when-destination-status` was given beside `--status-policy preserve-destination`,
    /// which makes no status write for it to guard.
    #[error(
        "--when-destination-status guards the status a copy writes, and --status-policy \
         preserve-destination writes none over an existing item\n\
         next: drop --when-destination-status, or copy with --status-policy source."
    )]
    StatusConditionPreserved,

    /// A task copy's counterpart was at a status category `--when-destination-status` does not
    /// name, under `--otherwise refuse`: nothing was written for it.
    #[error(
        "{item} was not copied: its counterpart {destination} is at {spelled}, which is not one \
         of {named}\n\
         next: copy it with --otherwise preserve-destination to keep that status, or name \
         {spelled} with --when-destination-status if the copy may write over it.",
        spelled = copy::spelled_category(*.held),
        named = .allowed
            .iter()
            .map(|category| copy::spelled_category(*category))
            .collect::<Vec<_>>()
            .join(", ")
    )]
    DestinationStatus {
        /// The source item being copied.
        item: GlobalId,
        /// The counterpart it would have updated.
        destination: GlobalId,
        /// The counterpart's status category.
        held: StatusCategory,
        /// The categories the condition names, in the order the caller named them.
        allowed: Vec<StatusCategory>,
    },

    /// A copy of documents was asked to carry comments: a document holds none, so only a task
    /// or a project copy carries them.
    #[error(
        "a document holds no comments, so a document copy carries none\n\
         next: copy the documents without --comments, or copy tasks or projects with it."
    )]
    CommentsNotDocuments,

    /// A copy read a task whose source moved to another revision of it between two of the
    /// requests that read it, and again when the copy read it a second time — or, for a task
    /// giving no change token whose comments run past one page, whose full passes over its
    /// comments never agreed two in a row.
    ///
    /// Reported against that task alone, as a failed outcome of the copy, rather than failing
    /// the copy: nothing was written for it, and every other item landed.
    #[error(
        "{item} changed while it was being copied, twice, so nothing was written for it rather \
         than a mix of two of its revisions\n\
         next: copy it again once it has stopped changing."
    )]
    ChangedWhileCopied {
        /// The task.
        item: GlobalId,
    },

    /// A destination did not hold its own stored content stable: the follow-up write that sent
    /// back exactly the content an earlier write's own answer said it stores — see
    /// [`TaskSource::stored_content`](onetaskgraph_plugin_api::TaskSource::stored_content) — was
    /// answered with something else again.
    ///
    /// A destination may rewrite the Markdown it is written, and the engine then takes what it
    /// reported storing as the item's record and writes it back once, beside the fingerprints
    /// it keeps over it. Only a destination that rewrites its own stored content is refused, so
    /// the `body_digest` and the copy record the item carries are never vouched for over
    /// content a later read would not hand back.
    #[error(
        "{item} was written, but source {} did not keep the content it said it stores: the \
         follow-up write sent that content back and its answer stores it otherwise, first at \
         line {line}:\n\
         sent:   {}\n\
         stored: {}\n\
         so the fingerprints the item records cannot be vouched for, and no rendering check will \
         accept it\n\
         next: read the item back from source {} and compare it with what the line shows; a \
         backend that rewrites its own stored content on every write is a defect of that \
         backend or its plugin, so report it there, then write the item again.",
        item.source,
        diverged_line(sent.as_deref(), *line),
        diverged_line(stored.as_deref(), *line),
        item.source
    )]
    StoredContentDiverged {
        /// The item as the destination holds it, qualified by the destination source.
        item: GlobalId,
        /// The first line, counted from 1, at which the two differ.
        line: usize,
        /// That line as the follow-up write sent it, `None` when the sent content ended before
        /// it — told apart from an empty line, because a line Linear inserts and a line it leaves
        /// out read alike otherwise.
        sent: Option<String>,
        /// That line as the destination stored it, `None` when the stored content ended before it.
        stored: Option<String>,
    },

    /// `--create` named an item that itself records a counterpart at the destination.
    ///
    /// Refused rather than created: the item's own `onetaskgraph.copies` link — or its
    /// origin — names an item there, so the caller's assertion that the destination holds
    /// none is wrong for it, and creating another would duplicate it.
    #[error(
        "{item} already records a counterpart at the destination, {carrier}, and --create \
         asserts it has none\n\
         next: copy it without --create, which updates {carrier}."
    )]
    CreateCarried {
        /// The item being copied.
        item: GlobalId,
        /// The destination item its link or its origin names.
        carrier: GlobalId,
    },

    /// A member copy named a task that is not a member of the project being copied.
    ///
    /// Refused before anything is written, because a copy that names members names the
    /// part of one project it carries: a task of some other project, or of no project, is
    /// not a narrower version of that copy but a different one.
    #[error(
        "{id} is not a task of {project}, so it cannot be copied as one of its members\n\
         next: name a task `onetaskgraph task list --project {project}` reports, or copy \
         {id} on its own with `onetaskgraph task copy`.",
        project = .projects.iter().map(ToString::to_string).collect::<Vec<_>>().join(", ")
    )]
    NotAMember {
        /// The id that was named as a member.
        id: GlobalId,
        /// The projects the copy carries, none of which it is a member of.
        projects: Vec<GlobalId>,
    },

    /// A members-only copy named a project with no counterpart where its members would land.
    ///
    /// Refused before anything is written, because a members-only copy leaves the project
    /// alone: it is the whole project copy this names that creates one.
    #[error(
        "{project} has no counterpart in {missing} to copy its members into, and a \
         members-only copy never creates or writes a project\n\
         next: create it with `onetaskgraph project copy {project} --to {destination}`, then \
         copy members into it with --members-only."
    )]
    NoProjectCounterpart {
        /// The project the copy named.
        project: GlobalId,
        /// The destination the copy was asked to write.
        destination: SourceName,
        /// The source holding no counterpart: the destination, or one its routes send a
        /// member to.
        missing: SourceName,
    },

    /// A member copy's item depends on a member it was not told to carry, and that member
    /// records no origin naming the destination.
    ///
    /// Refused before anything is written. The destination id of a member the copy does
    /// not carry comes from that member's own recorded origin and from nowhere else — not
    /// from a walk of the destination, which is the read a member copy exists to avoid —
    /// and writing the edge without it would either drop it silently or point it at the
    /// id the member has at its source, which the destination has never heard of.
    #[error(
        "{item} depends on {member}, which this copy was not told to carry and which records \
         no origin in {destination}\n\
         next: name {member} with --member as well, record its {destination} id at \
         onetaskgraph.origin, or copy the whole project without --member."
    )]
    UnrecordedMember {
        /// The item whose edge could not be resolved.
        item: GlobalId,
        /// The member that edge points at.
        member: GlobalId,
        /// The destination it records no origin in.
        destination: SourceName,
    },

    /// A routed copy found an item's existing counterpart in a source other than the one its
    /// repositories route it to now.
    ///
    /// Refused before anything is written. A repository changed after a copy is a person's
    /// decision about where the work lives, and moving it silently would leave the
    /// counterpart where it was, unlinked, beside a second one.
    #[error(
        "{item} was copied to {counterpart}, but its repositories now route it to {route}\n\
         next: move or remove {counterpart} by hand and copy again, or restore {item}'s \
         repositories so it routes to {source} again.",
        source = .counterpart.source
    )]
    Misrouted {
        /// The item being copied.
        item: GlobalId,
        /// The counterpart it already has.
        counterpart: GlobalId,
        /// The source it routes to now.
        route: SourceName,
    },

    /// A routed copy of a plan holding private work could not choose one home for it: the
    /// items whose repositories decide where its home goes, among those classified private,
    /// route to more than one source declared private.
    ///
    /// Refused before anything is written. A plan is one home, and choosing one of the two
    /// private sources would file the other's work under a member the first one names.
    #[error(
        "{project}'s private work routes to more than one private source, so it has no one \
         home: {split}\n\
         next: route the plan's private repositories to one source declared private, or add a \
         `{{classification: private, to: <source>}}` route to the source it was copied to.",
        split = .destinations
            .iter()
            .map(|(destination, items)| format!(
                "{destination} for {}",
                items.iter().map(ToString::to_string).collect::<Vec<_>>().join(", ")
            ))
            .collect::<Vec<_>>()
            .join("; ")
    )]
    SplitPrivateHome {
        /// The project being copied.
        project: GlobalId,
        /// Each private source its private work routes to, in the order first met, with the
        /// items that route there.
        destinations: Vec<(SourceName, Vec<GlobalId>)>,
    },

    /// A source refused something the copy asked of it.
    ///
    /// Distinct from a [`SourceFailure`], which leaves the other sources' results
    /// standing: a copy is one write into one destination, and half of one is not an
    /// answer.
    #[error(
        "source {name} could not do it: {error}\n\
         next: fix what the source named above, then copy again."
    )]
    SourceRefused {
        /// The source that refused.
        name: String,
        /// What it said.
        error: SourceError,
    },

    /// A copy failed, and the destination could not be put back the way it was found.
    ///
    /// A copy is either complete or it never happened, and the engine undoes its own
    /// writes to make that true. When the destination will not take one of them back, the
    /// failure has to say so and name what is still there: a user told only that the copy
    /// failed would copy again over a destination nobody has described to them, which is
    /// the retry that trips a hosted destination's rate limiter.
    #[error(
        "the copy failed and could not be undone.\n\
         it failed because: {error}\n\
         it could not be undone because: {refusal}\n\
         so these still hold what it wrote: {left_behind}\n\
         next: remove or put back those items, then copy again."
    )]
    CopyNotUndone {
        /// Why the copy failed in the first place.
        error: Box<EngineError>,
        /// The items the copy created or overwrote and could not take back.
        ///
        /// The ids themselves rather than a sentence about them: a caller acting on this —
        /// removing them, or reporting them — needs the ids, and the joined form only the
        /// message needs is [`LeftBehind`]'s own `Display`.
        left_behind: LeftBehind,
        /// What the destination said when the copy tried to take them back.
        refusal: SourceError,
    },

    /// A copy failed on a create whose answer was malformed, so the destination may hold an
    /// item the copy never learned the id of.
    ///
    /// A create is journalled by the id its answer carries, and a plugin that stored the
    /// item before answering in a shape the engine refuses has kept it under an id nobody
    /// was told. Every write the copy could identify is undone before this is returned, and
    /// one the destination will not take back wraps this in [`EngineError::CopyNotUndone`];
    /// the one it cannot identify is said rather than passed over, because a user told only
    /// that the answer was malformed would copy again and land a second item beside the first.
    #[error(
        "the copy failed, and {destination} may hold an item it could not take back.\n\
         it failed because: {}\n\
         {destination}'s answer to {operation} was malformed, so the copy never learned the \
         id of what that write may have stored and cannot take it back\n\
         next: inspect {destination} for an item this copy may have created and remove it \
         before copying again.",
        reason(error)
    )]
    CopyDestinationUncertain {
        /// The malformed answer, as the destination's refusal of the write.
        error: Box<EngineError>,
        // llmlint: ignore-block[invalid_states_unrepresentable] Both fields are `String` because this variant's shape is a stated contract other work builds against, and it matches its neighbours: `SourceRefused`, `NotWritable` and the rest name a source as a `String`. Both are only ever built in `engine/copy.rs`, from a configured source's own name and from one of four method literals.
        /// The destination source the create was sent to.
        destination: String,
        /// The plugin method whose answer was malformed: `write_task`, `write_tasks`,
        /// `write_document` or `write_project`.
        operation: String,
        // llmlint: ignore-end[invalid_states_unrepresentable]
    },
}

impl EngineError {
    /// Whether this refusal is of something already on the item before the write — what the
    /// failure document says as `"predates_write": true`.
    #[must_use]
    pub fn predates_write(&self) -> bool {
        matches!(
            self,
            Self::NotPrivateDestinationPredates { .. } | Self::BoundaryRefusedPredates { .. }
        )
    }

    /// Every item a copy refused as [`EngineError::Oversize`], and none for every other refusal.
    #[must_use]
    pub fn oversize(&self) -> &[copy::Oversized] {
        match self {
            Self::Oversize { items } => items,
            _ => &[],
        }
    }

    /// What the destination item a copy refused to write held, as the copy's read of it before
    /// writing found it — for a refusal made after that read, and `None` for every other.
    #[must_use]
    pub fn held(&self) -> Option<&crate::cache::WriteResult> {
        match self {
            Self::TargetTaskOrigin { held, .. } => held.as_deref(),
            Self::VersionMoved { held, .. } => Some(held),
            _ => None,
        }
    }
}

/// Every item of an [`EngineError::Oversize`], as its message names them.
fn oversized(items: &[copy::Oversized]) -> String {
    items
        .iter()
        .map(|item| {
            format!(
                "{} would write a body of {} characters to source {}, which holds at most {}",
                item.source, item.body_size, item.destination, item.limit
            )
        })
        .collect::<Vec<_>>()
        .join("; ")
}

/// One side's `line` of a [`EngineError::StoredContentDiverged`] as its message shows it: the
/// line quoted, or that side's content having ended before it.
fn diverged_line(text: Option<&str>, line: usize) -> String {
    match text {
        Some(text) => format!("{text:?}"),
        None => format!("no line {line}: the content ends at line {}", line - 1),
    }
}

/// Why a wrapped failure happened, without the next action it would give on its own: a
/// wrapper names the next action for the whole failure, and an inner one telling the user to
/// copy again is exactly what [`EngineError::CopyDestinationUncertain`] exists to replace.
fn reason(error: &EngineError) -> String {
    match error {
        EngineError::SourceRefused { name, error } => {
            format!("source {name} could not do it: {error}")
        }
        other => other.to_string(),
    }
}

/// The items a failed copy left at the destination — one of them at least, always.
///
/// A plain `Vec` here would let [`EngineError::CopyNotUndone`] be built naming nothing
/// still there, and naming what is still there is the whole reason that failure is
/// distinct from the one it wraps: a user told only that the copy could not be undone,
/// and then handed an empty list, has been told about a destination nobody described to
/// them, which is exactly the blind retry this mechanism exists to remove. The first id
/// is a field of its own, so the empty case cannot be written down.
#[derive(Debug, Clone, PartialEq)]
pub struct LeftBehind {
    /// The first item the destination would not take back.
    first: GlobalId,
    /// The ones it would not take back after that, in the order it refused them.
    rest: Vec<GlobalId>,
}

impl LeftBehind {
    /// The list holding the one item every such refusal has to name.
    #[must_use]
    pub fn new(first: GlobalId) -> Self {
        Self {
            first,
            rest: Vec::new(),
        }
    }

    /// Record another item the destination would not take back.
    pub fn push(&mut self, id: GlobalId) {
        self.rest.push(id);
    }

    /// Every item still at the destination, in the order the destination refused them.
    pub fn iter(&self) -> impl Iterator<Item = &GlobalId> {
        std::iter::once(&self.first).chain(self.rest.iter())
    }
}

impl std::fmt::Display for LeftBehind {
    /// The qualified ids, comma-separated — the form the failure message reads in.
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}", self.first)?;
        for id in &self.rest {
            write!(formatter, ", {id}")?;
        }
        Ok(())
    }
}

/// One configured source, in exactly one of the two states a configured source has.
///
/// A sum rather than two lists side by side: with a `ready` vector and an `unavailable`
/// one, a source appearing in both is a shape the type permits and every reader has to
/// decide about — and they would not all decide the same way, since one of them fans a
/// query out and another names failures.
pub enum ConfiguredSource {
    /// It built, and answers queries.
    Ready(ResolvedSource),
    /// It did not, and every response says so instead.
    Unavailable(UnavailableSource),
}

impl ConfiguredSource {
    /// The name the configuration gave it, whichever state it is in.
    #[must_use]
    pub fn name(&self) -> &SourceName {
        match self {
            Self::Ready(source) => source.name(),
            Self::Unavailable(source) => source.name(),
        }
    }
}

/// The sources a configuration resolved to, and the queries they answer.
pub struct Engine {
    /// Every configured source, in configured-name order, each in one state.
    sources: Vec<ConfiguredSource>,
    /// Which sources answer when a request names none.
    selection: Vec<SourceName>,
    /// Sources a configuration layer refines but none declares a `plugin` for: not
    /// configured, and refused by name rather than as unknown.
    incomplete: std::collections::BTreeSet<SourceName>,
    /// Where an item written to a source goes instead. Read from configuration and never
    /// from a source, so it holds nothing of anybody's work.
    routes: Routes,
    /// How the public boundary stands: active or not, the policy it asks, the term scope.
    boundary: boundary::Boundary,
    /// The local cache of each source configured with one, by name.
    caches: BTreeMap<SourceName, std::sync::Arc<crate::cache::SourceCache>>,
    /// The destination version a running `task copy --expect` names, while it runs.
    copy_expect: std::sync::Mutex<Option<String>>,
    /// Which tracker each source reads, as its configuration declares and its plugin answers.
    designators: designators::Designators,
    /// Each source's human task projection authority, as its configuration declares it.
    projections: BTreeMap<SourceName, Result<crate::task_projection::ProjectionSource, String>>,
    /// The clock a copy waits on while an earlier attempt's create may still be listed.
    clock: SharedClock,
    /// Each source's settings for a create safe to retry, by name; a source not named keeps
    /// the defaults.
    creates: BTreeMap<SourceName, crate::config::CreateSettings>,
    /// This engine's own name for the copy attempts it makes, recorded on every intent to
    /// create it takes, so an intent another process holds is told from its own.
    attempt: copy::intent::Attempt,
}

/// One operation's hold on every source's visibility reads, released when dropped — on every
/// path out of the operation, a refusal's included.
pub(crate) struct VisibilityHold<'a> {
    engine: &'a Engine,
}

impl Drop for VisibilityHold<'_> {
    fn drop(&mut self) {
        for source in self.engine.ready() {
            source.source().release_visibility();
        }
    }
}

/// A read task with same-record section links restored to authored slugs.
fn authored_task(source: &ResolvedSource, mut task: Task) -> Task {
    if let (Some(form), Some(content)) = (
        crate::section_links::SectionForm::for_kind(source.kind()),
        task.content.as_deref(),
    ) {
        let own = task.location.as_ref().map(|location| match location {
            onetaskgraph_plugin_api::Location::Url(url) => url.as_str(),
            onetaskgraph_plugin_api::Location::Path(path) => path.as_str(),
        });
        let stored = source
            .source()
            .stored_headings(MetadataRecord::Task, &task.id);
        let authored = form.read_same_record(content, own, stored.as_deref());
        if authored != content
            && let Some(provenance) = stored::restamped(&task.metadata, &[content], &authored)
        {
            task.metadata.insert(
                crate::template::TemplateProvenance::KEY.to_owned(),
                provenance.to_value(),
            );
        }
        task.content = Some(authored);
    }
    task
}

/// `source`, handed the markup its kind's section form and expandable form need its bodies to
/// carry, so a source built anywhere gets it on the way in: see
/// [`SectionForm::markup`](crate::section_links::SectionForm::markup) and
/// [`ExpandableForm::markup`](crate::expandable_blocks::ExpandableForm::markup).
fn with_body_markup(source: ResolvedSource) -> ResolvedSource {
    // No kind needs both: `expandable_blocks`' tests hold the two entries to that.
    if let Some(markup) = crate::expandable_blocks::ExpandableForm::for_kind(source.kind())
        .and_then(crate::expandable_blocks::ExpandableForm::markup)
        .or_else(|| {
            crate::section_links::SectionForm::for_kind(source.kind())
                .and_then(|form| form.markup())
        })
    {
        source.source().attach_body_markup(markup);
    }
    source
}

/// `source`, sent nothing between a rate limit it answers and the reset that limit names, on
/// `clock`.
fn held_off(source: ResolvedSource, clock: &SharedClock) -> ResolvedSource {
    let name = source.name().clone();
    source.wrapped(|inner| Box::new(held_off::HeldOff::new(name, inner, clock.clone())))
}

impl Engine {
    /// Build every source a configuration names.
    ///
    /// A source whose plugin refuses to build — a credential that is not there, a
    /// plugin whose implementation has not landed — is **not** fatal: it becomes an
    /// entry in every response's `errors`, exactly as a source that fails mid-query
    /// does, and the other sources still answer. A user with three sources and one
    /// expired token gets the other two rather than nothing.
    #[must_use]
    pub fn build(config: &Config, secrets: &dyn SecretResolver) -> Self {
        Self::build_with_clock(config, secrets, &system_clock())
    }

    /// [`build`](Self::build), handing every in-process source `clock` to pace and back off
    /// on — the process's one clock, which the binary takes from
    /// [`process_clock`](crate::process_clock).
    #[must_use]
    pub fn build_with_clock(
        config: &Config,
        secrets: &dyn SecretResolver,
        clock: &SharedClock,
    ) -> Self {
        let (ready, unavailable) = resolve_available_with_clock(config, secrets, clock);
        let mut caches = BTreeMap::new();
        let located: BTreeMap<SourceName, crate::cache::CacheLocation> =
            crate::cache::locations(config).into_iter().collect();
        let ready: Vec<ResolvedSource> = ready
            .into_iter()
            .map(|source| match located.get(source.name()) {
                Some(location) => {
                    let name = source.name().clone();
                    source.wrapped(|inner| {
                        let (cached, cache) =
                            crate::cache::CachedSource::new(inner, location.clone());
                        caches.insert(name, cache);
                        Box::new(cached) as Box<dyn onetaskgraph_plugin_api::TaskSource>
                    })
                }
                None => source,
            })
            .collect();
        let mut engine = Self::assembled(
            ready
                .into_iter()
                .map(ConfiguredSource::Ready)
                .chain(unavailable.into_iter().map(ConfiguredSource::Unavailable))
                .collect(),
            config.selected_sources(),
            clock,
        )
        .with_routes(config.routes())
        .with_incomplete(config.incomplete().clone())
        .with_boundary(boundary::Boundary::configured(
            config.write_policy(),
            &config.visibilities(),
        ));
        engine.caches = caches;
        engine.designators = designators::Designators::configured(config);
        engine.projections = crate::task_projection::configured(config);
        engine.creates = config
            .sources()
            .iter()
            .map(|(name, source)| (name.clone(), source.create()))
            .collect();
        engine
    }

    /// Drive sources built elsewhere — the engine's own tests, and any caller holding a
    /// source it did not resolve from a configuration document.
    ///
    /// Each ready source is put behind the engine's rate-limit memory on the system clock, as
    /// [`build_with_clock`](Self::build_with_clock) puts each one behind it on the clock it is
    /// handed: see `held_off`.
    #[must_use]
    pub fn new(sources: Vec<ConfiguredSource>, selection: Vec<SourceName>) -> Self {
        Self::assembled(sources, selection, &system_clock())
    }

    /// An engine over `sources`, each ready one behind the rate-limit memory on `clock` — the
    /// one place every source is put there, so every call to it passes through.
    fn assembled(
        sources: Vec<ConfiguredSource>,
        selection: Vec<SourceName>,
        clock: &SharedClock,
    ) -> Self {
        let sources = sources
            .into_iter()
            .map(|source| match source {
                ConfiguredSource::Ready(source) => {
                    ConfiguredSource::Ready(held_off(with_body_markup(source), clock))
                }
                unavailable @ ConfiguredSource::Unavailable(_) => unavailable,
            })
            .collect();
        Self {
            sources,
            selection,
            incomplete: std::collections::BTreeSet::new(),
            routes: Routes::default(),
            boundary: boundary::Boundary::default(),
            caches: BTreeMap::new(),
            copy_expect: std::sync::Mutex::new(None),
            designators: designators::Designators::default(),
            projections: BTreeMap::new(),
            clock: clock.clone(),
            creates: BTreeMap::new(),
            attempt: copy::intent::attempt(),
        }
    }

    fn with_incomplete(mut self, incomplete: std::collections::BTreeSet<SourceName>) -> Self {
        self.incomplete = incomplete;
        self
    }

    fn with_boundary(mut self, boundary: boundary::Boundary) -> Self {
        self.boundary = boundary;
        self
    }

    /// The same engine, placing what is written to each source by `routes`.
    ///
    /// [`build`](Self::build) takes them from the configuration; a caller holding sources
    /// it built itself states them here, checked by [`Routes::new`].
    ///
    /// Each source is told the visibility the routes declare for it
    /// ([`TaskSource::declare_visibility`](onetaskgraph_plugin_api::TaskSource::declare_visibility)),
    /// which is where a source that places a new item per item learns it is declared private.
    #[must_use]
    pub fn with_routes(mut self, routes: Routes) -> Self {
        self.routes = routes;
        for source in self.ready() {
            source
                .source()
                .declare_visibility(match self.routes.visibility(source.name()) {
                    crate::config::SourceVisibility::Public => {
                        onetaskgraph_plugin_api::Visibility::Public
                    }
                    crate::config::SourceVisibility::Private => {
                        onetaskgraph_plugin_api::Visibility::Private
                    }
                    crate::config::SourceVisibility::Unknown => {
                        onetaskgraph_plugin_api::Visibility::Unknown
                    }
                });
        }
        self
    }

    /// Hold every source's visibility reads for one operation, until the guard is dropped: a
    /// destination is read once for the whole operation rather than once per write
    /// ([`TaskSource::hold_visibility`](onetaskgraph_plugin_api::TaskSource::hold_visibility)).
    pub(crate) fn hold_visibility(&self) -> VisibilityHold<'_> {
        for source in self.ready() {
            source.source().hold_visibility();
        }
        VisibilityHold { engine: self }
    }

    /// Where an item with `repositories`, written to `source`, lands.
    #[must_use]
    pub fn place(&self, source: &SourceName, repositories: &[Repository]) -> Placement {
        self.routes.place(source, repositories)
    }

    /// Every source that built, in configured-name order.
    fn ready(&self) -> impl Iterator<Item = &ResolvedSource> {
        self.sources.iter().filter_map(|source| match source {
            ConfiguredSource::Ready(ready) => Some(ready),
            ConfiguredSource::Unavailable(_) => None,
        })
    }

    /// The source called `name`, when it built.
    pub(crate) fn ready_source(
        &self,
        name: &SourceName,
    ) -> Option<&dyn onetaskgraph_plugin_api::TaskSource> {
        self.ready()
            .find(|source| source.name() == name)
            .map(ResolvedSource::source)
    }

    /// Every source that did not, in the same order.
    fn unavailable(&self) -> impl Iterator<Item = &UnavailableSource> {
        self.sources.iter().filter_map(|source| match source {
            ConfiguredSource::Unavailable(unavailable) => Some(unavailable),
            ConfiguredSource::Ready(_) => None,
        })
    }

    /// Every configured source, whether or not it built, in name order.
    #[must_use]
    pub fn listing(&self) -> Vec<SourceListing> {
        let mut listings: Vec<SourceListing> = self
            .ready()
            .map(|source| SourceListing {
                source: source.name().clone(),
                kind: source.kind().to_owned(),
                state: SourceState::Available {
                    capabilities: source.source().capabilities(),
                },
            })
            .chain(self.unavailable().map(|source| SourceListing {
                source: source.name().clone(),
                kind: source.kind().to_owned(),
                state: SourceState::Unavailable {
                    error: source.error().clone(),
                },
            }))
            .collect();
        listings.sort_by(|left, right| left.source.cmp(&right.source));
        listings
    }

    /// Whether this configuration has a source called `name`, built or not.
    ///
    /// A caller reading a `--project` argument needs this: `urn:project:1` is a qualified
    /// id only if `urn` is a source here, and a native id full of colons otherwise. That
    /// rule cannot be applied without knowing what is configured.
    #[must_use]
    pub fn has(&self, name: &SourceName) -> bool {
        self.sources.iter().any(|source| source.name() == name)
    }

    /// End one command on every source this engine holds, so the next command reads afresh.
    ///
    /// A caller holding one engine for many units of work — a worker writing settlement after
    /// settlement back over hours — calls this between them. A source may reuse what it read
    /// within one command, so without this call a person's edit made between two units can be
    /// overwritten by the next write. After it, no source answers a read or bases a write on
    /// item content, statuses, board contents or search results it held before. What a source
    /// keeps is its own decision, made under [`TaskSource::end_command`]'s rule; nothing here
    /// names a plugin.
    ///
    /// The binary never calls it: one command line is one process, which drops everything at
    /// exit.
    ///
    /// # Errors
    ///
    /// Returns [`EngineError::SourceFailed`] for the first source that could not end its
    /// command. Every source is asked whether or not one before it failed.
    ///
    /// [`TaskSource::end_command`]: onetaskgraph_plugin_api::TaskSource::end_command
    pub async fn end_command(&self) -> Result<(), EngineError> {
        // What the write policy answered was this command's: the next one asks again.
        self.boundary.forget();
        let mut first = None;
        for source in self.ready() {
            if let Err(error) = source.source().end_command().await
                && first.is_none()
            {
                first = Some(EngineError::SourceFailed {
                    name: source.name().to_string(),
                    error,
                });
            }
        }
        first.map_or(Ok(()), Err)
    }

    /// One page of tasks.
    ///
    /// # Errors
    ///
    /// Returns [`EngineError`] when the request names a source nothing configures, or
    /// carries a page token this engine did not issue. One source failing is not an
    /// error: it lands in the response's `errors`.
    pub async fn tasks(
        &self,
        request: &TaskRequest,
    ) -> Result<QueryResponse<Qualified<Task>>, EngineError> {
        let mut names = self.resolve_selection(&request.sources)?;
        // A qualified project id names one project of one source, so no other source can
        // hold a task in it. Narrowing here means the plan reports the source that was
        // actually asked rather than a row of empty entries for sources that could not
        // have answered.
        let mut members: Vec<GlobalId> = Vec::new();
        let mut unread: Vec<SourceFailure> = Vec::new();
        if let ProjectSelector::Qualified(id) = &request.project {
            self.known(&id.source)?;
            names.retain(|name| name == &id.source);
            // Its members join it, each asked for in its own source: a home has at most one
            // member per source, so one source still answers for one project.
            if request.include_members {
                (members, unread) = self.members(id).await;
                names.extend(members.iter().map(|member| member.source.clone()));
            }
        }
        let selector = |name: &SourceName| -> ProjectSelector {
            members
                .iter()
                .find(|member| &member.source == name)
                .map_or_else(
                    || request.project.clone(),
                    |member| ProjectSelector::Qualified(member.clone()),
                )
        };
        let query = shape(
            "task-list",
            &names,
            &(
                &request.filters,
                &request.project,
                &request.priorities,
                &request.commented_since,
                &request.metadata,
                &request.origin,
                &members,
            ),
        );
        let states = resumption(
            self,
            request.paging.token.as_ref(),
            &[StreamKind::Items],
            &query,
        )?;
        let budget = request.paging.limit.get();

        let mut answer = Answer::new();
        answer.errors.extend(unread);
        let (ready, starts) = walking(answer.split(self, &names), &states, StreamKind::Items);

        let shapes: Vec<TaskShape> = ready
            .iter()
            .map(|source| {
                shape_tasks(
                    &source.source().capabilities(),
                    &request.filters,
                    &project_filter(&selector(source.name())),
                    &request.priorities,
                    request.commented_since,
                    &request.metadata,
                    request.origin.as_ref(),
                )
            })
            .collect();
        let counters: Vec<AtomicU32> = ready.iter().map(|_| AtomicU32::new(0)).collect();
        let outcomes: Vec<Outcomes> = shapes.iter().map(|shape| shape.outcomes.clone()).collect();

        let walks = ready
            .iter()
            .enumerate()
            .map(|(index, source)| {
                fetch_tasks(
                    source,
                    &shapes[index],
                    &starts[index],
                    budget,
                    &counters[index],
                )
            })
            .collect();

        let streams = answer.collect(&ready, join_all(walks).await, &counters, outcomes);
        answer.finish(
            streams,
            budget,
            owed(&states),
            &query,
            |name, task: Task| {
                delivery::qualified_task(GlobalId::new(name.clone(), task.id.clone()), task)
            },
        )
    }

    /// The member projects `home` names at `onetaskgraph.members` that can be read, and a
    /// failure for each that cannot.
    ///
    /// Read from the home on every call and kept nowhere. A member in a source nothing
    /// configures, one its source does not hold, and one whose source fails the read are
    /// each reported rather than left out, because a plan missing a member without saying
    /// so reads exactly like a plan that has none. So is a home its source fails to read or
    /// does not hold: its members cannot be learned, and a page of its own tasks alone must
    /// not pass for the whole plan. A home whose source did not build is reported by the
    /// walk, as any unavailable source is.
    async fn members(&self, home: &GlobalId) -> (Vec<GlobalId>, Vec<SourceFailure>) {
        let mut members = Vec::new();
        let mut unread = Vec::new();
        let Some(source) = self.ready().find(|source| source.name() == &home.source) else {
            return (members, unread);
        };
        let held = match source.source().get_project(&home.native).await {
            Ok(Some(held)) => held,
            Ok(None) => {
                unread.push(SourceFailure {
                    source: home.source.clone(),
                    error: SourceError::Refused {
                        message: format!(
                            "{home} names no project {} holds, so its member projects cannot \
                             be read",
                            home.source
                        ),
                    },
                });
                return (members, unread);
            }
            Err(error) => {
                unread.push(SourceFailure {
                    source: home.source.clone(),
                    error,
                });
                return (members, unread);
            }
        };
        let named = match copy::members_of(home, &held.metadata) {
            Ok(named) => named,
            // A list nobody can read is a home whose plan cannot be read whole, which is a
            // failure of the home's source to report rather than a plan with no members.
            Err(message) => {
                unread.push(SourceFailure {
                    source: home.source.clone(),
                    error: SourceError::Malformed { message },
                });
                return (members, unread);
            }
        };
        for member in named {
            if !self.has(&member.source) {
                unread.push(SourceFailure {
                    source: member.source.clone(),
                    error: SourceError::Config {
                        message: format!(
                            "{home} names {member} as a member project, but no source named \
                             {} is configured",
                            member.source
                        ),
                    },
                });
                continue;
            }
            // A member whose source did not build is reported by the walk, as any
            // unavailable source is.
            let Some(there) = self.ready().find(|source| source.name() == &member.source) else {
                members.push(member);
                continue;
            };
            match there.source().get_project(&member.native).await {
                Ok(Some(_)) => members.push(member),
                Ok(None) => unread.push(SourceFailure {
                    source: member.source.clone(),
                    error: SourceError::Refused {
                        message: format!(
                            "{home} names {member} as a member project, and {} holds no such \
                             project",
                            member.source
                        ),
                    },
                }),
                Err(error) => unread.push(SourceFailure {
                    source: member.source.clone(),
                    error,
                }),
            }
        }
        (members, unread)
    }

    /// One page of projects.
    ///
    /// # Errors
    ///
    /// As [`tasks`](Self::tasks).
    pub async fn projects(
        &self,
        request: &ProjectRequest,
    ) -> Result<QueryResponse<Qualified<Project>>, EngineError> {
        self.projects_copied_from(request, None).await
    }

    /// One page of the projects copied from `origin` — those whose recorded copy origin is
    /// exactly that qualified id — in the shape [`projects`](Self::projects) answers, with
    /// every other filter of `request` applied as it applies them.
    ///
    /// Each selected source is asked [`TaskSource::query_projects_by_origin`], so a source
    /// that can find one origin's copies answers without listing every project it holds, and
    /// the plan reports the origin pushed down to it. Zero, one or several projects may come
    /// back: which of several a caller wants is the caller's question.
    ///
    /// # Errors
    ///
    /// As [`tasks`](Self::tasks).
    pub async fn projects_by_origin(
        &self,
        request: &ProjectRequest,
        origin: &GlobalId,
    ) -> Result<QueryResponse<Qualified<Project>>, EngineError> {
        self.projects_copied_from(request, Some(origin)).await
    }

    /// [`projects`](Self::projects), narrowed to one copy origin when `origin` names one.
    async fn projects_copied_from(
        &self,
        request: &ProjectRequest,
        origin: Option<&GlobalId>,
    ) -> Result<QueryResponse<Qualified<Project>>, EngineError> {
        let names = self.resolve_selection(&request.sources)?;
        // An origin is part of what a page token resumes, so a token one origin's listing
        // handed out is refused by another's, as a token of other filters is.
        let query = match origin {
            None => shape("project-list", &names, &request.filters),
            Some(origin) => shape(
                "project-list",
                &names,
                &(&request.filters, origin.to_string()),
            ),
        };
        let states = resumption(
            self,
            request.paging.token.as_ref(),
            &[StreamKind::Items],
            &query,
        )?;
        let budget = request.paging.limit.get();

        let mut answer = Answer::new();
        // A source declaring `projects: unsupported` has no project table at all, so
        // there is nothing to compensate for and nothing to ask: the predicate is
        // reported unavailable and that source contributes no rows. This is the one
        // outcome the engine cannot narrow its way out of, which is what `unavailable`
        // in the plan is for.
        let mut with_projects = Vec::new();
        for source in answer.split(self, &names) {
            if source.source().capabilities().projects.is_native() {
                with_projects.push(source);
            } else {
                answer.unreachable_predicate(source, Predicate::Project);
            }
        }
        let (ready, starts) = walking(with_projects, &states, StreamKind::Items);

        let shapes: Vec<ProjectShape> = ready
            .iter()
            .map(|source| shape_projects(&source.source().capabilities(), &request.filters, origin))
            .collect();
        let counters: Vec<AtomicU32> = ready.iter().map(|_| AtomicU32::new(0)).collect();
        let outcomes: Vec<Outcomes> = shapes.iter().map(|shape| shape.outcomes.clone()).collect();

        let walks = ready
            .iter()
            .enumerate()
            .map(|(index, source)| {
                fetch_projects(
                    source,
                    &shapes[index],
                    &starts[index],
                    budget,
                    &counters[index],
                )
            })
            .collect();

        let streams = answer.collect(&ready, join_all(walks).await, &counters, outcomes);
        answer.finish(
            streams,
            budget,
            owed(&states),
            &query,
            |name, project: Project| Qualified {
                id: GlobalId::new(name.clone(), project.id.clone()),
                item: project,
            },
        )
    }

    /// One page of documents.
    ///
    /// A source declaring it has no documents is **not asked**: the declaration is read
    /// once here, that source contributes no rows, and the plan reports
    /// [`Predicate::Document`] unavailable for it. So a document list spanning a mixed set
    /// of sources answers with what the document-bearing ones hold and says nothing
    /// alarming about the others — the same shape a source with no project table takes.
    ///
    /// # Errors
    ///
    /// As [`tasks`](Self::tasks).
    pub async fn documents(
        &self,
        request: &DocumentRequest,
    ) -> Result<QueryResponse<Qualified<Document>>, EngineError> {
        let mut names = self.resolve_selection(&request.sources)?;
        // A qualified project id names one project of one source, exactly as it does for
        // tasks, so no other source can hold a document in it.
        if let ProjectSelector::Qualified(id) = &request.project {
            self.known(&id.source)?;
            names.retain(|name| name == &id.source);
        }
        let query = shape(
            "document-list",
            &names,
            &(&request.filters, &request.project),
        );
        let states = resumption(
            self,
            request.paging.token.as_ref(),
            &[StreamKind::Items],
            &query,
        )?;
        let budget = request.paging.limit.get();

        let mut answer = Answer::new();
        let mut with_documents = Vec::new();
        for source in answer.split(self, &names) {
            if source.source().capabilities().documents.is_native() {
                with_documents.push(source);
            } else {
                answer.unreachable_predicate(source, Predicate::Document);
            }
        }
        let (ready, starts) = walking(with_documents, &states, StreamKind::Items);

        let shapes: Vec<DocumentShape> = ready
            .iter()
            .map(|source| {
                shape_documents(
                    &source.source().capabilities(),
                    &request.filters,
                    &project_filter(&request.project),
                )
            })
            .collect();
        let counters: Vec<AtomicU32> = ready.iter().map(|_| AtomicU32::new(0)).collect();
        let outcomes: Vec<Outcomes> = shapes.iter().map(|shape| shape.outcomes.clone()).collect();

        let walks = ready
            .iter()
            .enumerate()
            .map(|(index, source)| {
                fetch_documents(
                    source,
                    &shapes[index],
                    &starts[index],
                    budget,
                    &counters[index],
                )
            })
            .collect();

        let streams = answer.collect(&ready, join_all(walks).await, &counters, outcomes);
        answer.finish(
            streams,
            budget,
            owed(&states),
            &query,
            |name, document: Document| Qualified {
                id: GlobalId::new(name.clone(), document.id.clone()),
                item: document,
            },
        )
    }

    /// One page of labels.
    ///
    /// # Errors
    ///
    /// As [`tasks`](Self::tasks).
    pub async fn labels(
        &self,
        request: &LabelRequest,
    ) -> Result<QueryResponse<Qualified<Label>>, EngineError> {
        let names = self.resolve_selection(&request.sources)?;
        let query = shape("label-list", &names, &());
        let states = resumption(
            self,
            request.paging.token.as_ref(),
            &[StreamKind::Items],
            &query,
        )?;
        let budget = request.paging.limit.get();

        let mut answer = Answer::new();
        let (ready, starts) = walking(answer.split(self, &names), &states, StreamKind::Items);
        let counters: Vec<AtomicU32> = ready.iter().map(|_| AtomicU32::new(0)).collect();
        let outcomes: Vec<Outcomes> = ready.iter().map(|_| Outcomes::default()).collect();

        let walks = ready
            .iter()
            .enumerate()
            .map(|(index, source)| fetch_labels(source, &starts[index], budget, &counters[index]))
            .collect();

        let streams = answer.collect(&ready, join_all(walks).await, &counters, outcomes);
        answer.finish(
            streams,
            budget,
            owed(&states),
            &query,
            |name, label: Label| Qualified {
                id: GlobalId::new(name.clone(), label.id.clone()),
                item: label,
            },
        )
    }

    /// One page of search hits, over tasks, projects, or both.
    ///
    /// # Errors
    ///
    /// As [`tasks`](Self::tasks).
    pub async fn search(
        &self,
        request: &SearchRequest,
    ) -> Result<QueryResponse<SearchHit>, EngineError> {
        let names = self.resolve_selection(&request.sources)?;
        // The streams this search reads, which is what a token resuming it may name. A
        // `--kind both` walk that has exhausted one half carries only the other, so this
        // is what a token may name rather than what it must.
        let reads: &[StreamKind] = match request.kind {
            SearchKind::Tasks => &[StreamKind::Tasks],
            SearchKind::Projects => &[StreamKind::Projects],
            SearchKind::Both => &[StreamKind::Tasks, StreamKind::Projects],
        };
        // The scope is deliberately not in the fingerprint: which streams a search covers
        // is exactly what `reads` checks below, name by name and with a message that says
        // which half a token names. Folding it in here would refuse the same mistake one
        // layer earlier and less clearly, and leave that check unreachable.
        let query = shape("search", &names, &request.text);
        let states = resumption(self, request.paging.token.as_ref(), reads, &query)?;
        let budget = request.paging.limit.get();
        let filters = Filters {
            text: Some(request.text.clone()),
            ..Filters::default()
        };

        let mut answer = Answer::new();

        // One stream per (source, entity), because a search over both entities reads two
        // result sets from each source and each has its own place to resume.
        let mut ready = Vec::new();
        let mut kinds = Vec::new();
        let mut starts = Vec::new();
        for source in answer.split(self, &names) {
            let mut streams = Vec::new();
            if matches!(request.kind, SearchKind::Tasks | SearchKind::Both) {
                streams.push(StreamKind::Tasks);
            }
            if matches!(request.kind, SearchKind::Projects | SearchKind::Both) {
                if source.source().capabilities().projects.is_native() {
                    streams.push(StreamKind::Projects);
                } else {
                    answer.unreachable_predicate(source, Predicate::Project);
                }
            }
            for stream in streams {
                if let Some(resume) = resume_at(&states, source.name(), stream) {
                    ready.push(source);
                    kinds.push(stream);
                    starts.push(resume);
                }
            }
        }

        let shapes: Vec<HitShape> = ready
            .iter()
            .zip(kinds.iter())
            .map(|(source, kind)| shape_hits(&source.source().capabilities(), &filters, *kind))
            .collect();
        let counters: Vec<AtomicU32> = ready.iter().map(|_| AtomicU32::new(0)).collect();
        let outcomes: Vec<Outcomes> = shapes.iter().map(|shape| shape.outcomes.clone()).collect();

        let walks = ready
            .iter()
            .enumerate()
            .map(|(index, source)| {
                fetch_hits(
                    source,
                    &shapes[index],
                    &starts[index],
                    budget,
                    &counters[index],
                )
            })
            .collect();

        let streams =
            answer.collect_streams(&ready, &kinds, join_all(walks).await, &counters, outcomes);
        answer.finish(
            streams,
            budget,
            owed(&states),
            &query,
            |name, found: Found| match found {
                Found::Task(task) => SearchHit::Task(delivery::qualified_task(
                    GlobalId::new(name.clone(), task.id.clone()),
                    task,
                )),
                Found::Project(project) => SearchHit::Project(Qualified {
                    id: GlobalId::new(name.clone(), project.id.clone()),
                    item: project,
                }),
            },
        )
    }

    /// One task by its qualified id, or an empty page when there is no such task.
    ///
    /// # Errors
    ///
    /// Returns [`EngineError::UnknownSource`] when the id names a source nothing
    /// configures.
    pub async fn task(&self, id: &GlobalId) -> Result<QueryResponse<Qualified<Task>>, EngineError> {
        let name = self.known(&id.source)?;
        let mut answer = Answer::new();
        let selected = answer.split(self, std::slice::from_ref(&name));
        let Some(source) = selected.first() else {
            return answer.nothing();
        };
        let found = source
            .source()
            .get_task(&id.native)
            .await
            .map(|found| found.map(|task| authored_task(source, task)));
        let qualified = GlobalId::new(source.name().clone(), id.native.clone());
        answer.one(source, found, |task| {
            delivery::qualified_task(qualified, task)
        })
    }

    /// One project by its qualified id, or an empty page when there is no such project.
    ///
    /// # Errors
    ///
    /// As [`task`](Self::task).
    pub async fn project(
        &self,
        id: &GlobalId,
    ) -> Result<QueryResponse<Qualified<Project>>, EngineError> {
        let name = self.known(&id.source)?;
        let mut answer = Answer::new();
        let selected = answer.split(self, std::slice::from_ref(&name));
        let Some(source) = selected.first() else {
            return answer.nothing();
        };
        let found = source.source().get_project(&id.native).await;
        let qualified = GlobalId::new(source.name().clone(), id.native.clone());
        answer.one(source, found, |project| Qualified {
            id: qualified,
            item: project,
        })
    }

    /// One document by its qualified id, or an empty page when there is no such document.
    ///
    /// A source declaring it has no documents holds none, so it is not asked and the
    /// answer is the empty page with [`Predicate::Document`] reported unavailable — the
    /// same answer the list verb gives, rather than a failure.
    ///
    /// # Errors
    ///
    /// As [`task`](Self::task).
    pub async fn document(
        &self,
        id: &GlobalId,
    ) -> Result<QueryResponse<Qualified<Document>>, EngineError> {
        let name = self.known(&id.source)?;
        let mut answer = Answer::new();
        let selected = answer.split(self, std::slice::from_ref(&name));
        let Some(source) = selected.first() else {
            return answer.nothing();
        };
        if !source.source().capabilities().documents.is_native() {
            answer.unreachable_predicate(source, Predicate::Document);
            return answer.nothing();
        }
        let found = source.source().get_document(&id.native).await;
        let qualified = GlobalId::new(source.name().clone(), id.native.clone());
        answer.one(source, found, |document| Qualified {
            id: qualified,
            item: document,
        })
    }

    /// One page of a task's dependency edges.
    ///
    /// # Errors
    ///
    /// As [`task`](Self::task), plus [`EngineError::Token`] for a page token this engine
    /// did not issue.
    pub async fn task_dependencies(
        &self,
        request: &DependencyRequest,
    ) -> Result<QueryResponse<QualifiedEdge>, EngineError> {
        self.dependencies(request, Entity::Task).await
    }

    /// One page of a project's dependency edges.
    ///
    /// # Errors
    ///
    /// As [`task_dependencies`](Self::task_dependencies).
    pub async fn project_dependencies(
        &self,
        request: &DependencyRequest,
    ) -> Result<QueryResponse<QualifiedEdge>, EngineError> {
        self.dependencies(request, Entity::Project).await
    }

    /// Both dependency verbs, which differ only in which of a source's two edge sets
    /// they read and which of its two declarations governs the reverse direction.
    ///
    /// A task's edges are also what the private side records of them (see
    /// [`dependents`]): a forward read of a task in a source not declared private reads, as
    /// one more stream each, the tasks of every source declared private that record it as
    /// depending on them, and a reverse read of a task in a source declared private reads the
    /// dependents it records on itself beside its source's own answer. A recording source
    /// that could not be asked is reported in [`QueryResponse::unanswered`] rather than as a
    /// failure, unless the request asks for [`Completeness::Required`].
    async fn dependencies(
        &self,
        request: &DependencyRequest,
        entity: Entity,
    ) -> Result<QueryResponse<QualifiedEdge>, EngineError> {
        let name = self.known(&request.id.source)?;
        let recording = match (entity, request.direction) {
            (Entity::Task, Direction::DependsOn) => {
                self.recording_sources_for(&name, &request.id.native).await
            }
            _ => Vec::new(),
        };
        let records_its_own = entity == Entity::Task
            && request.direction == Direction::DependedOnBy
            && self.declared(&name) == crate::config::SourceVisibility::Private;
        let mut names = vec![name.clone()];
        names.extend(recording.iter().cloned());
        let query = shape(
            "dependencies",
            &names,
            &(entity, &request.id.native, request.direction),
        );
        let reads: &[StreamKind] = if records_its_own {
            &[StreamKind::Items, StreamKind::Recorded]
        } else {
            &[StreamKind::Items]
        };
        let states = resumption(self, request.paging.token.as_ref(), reads, &query)?;
        let budget = request.paging.limit.get();

        let mut answer = Answer::new();
        let selected = answer.split(self, &names);
        let Some(source) = selected
            .iter()
            .copied()
            .find(|source| source.name() == &name)
        else {
            return answer.nothing();
        };

        let capabilities = source.source().capabilities();
        let support = match entity {
            Entity::Task => capabilities.task_dependencies,
            Entity::Project => capabilities.project_dependencies,
        };
        // `DependencySupport` has no unsupported variant on purpose: a dependency read is
        // answered natively or emulated by the scan below, never abandoned and never
        // silently empty.
        let emulating = request.direction == Direction::DependedOnBy && !support.answers_reverse();
        let mut own = Outcomes::default();
        if request.direction == Direction::DependedOnBy {
            if emulating {
                own.record(Predicate::ReverseDependencies, Outcome::Emulated);
            } else {
                own.record(Predicate::ReverseDependencies, Outcome::PushedDown);
            }
        }

        // Every stream still walking, in one order: the source's own, the dependents the task
        // records on itself, then each recording source's.
        let mut streams: Vec<(&ResolvedSource, StreamKind, Resume)> = Vec::new();
        if let Some(start) = resume_at(&states, &name, StreamKind::Items) {
            streams.push((source, StreamKind::Items, start));
        }
        if records_its_own && let Some(start) = resume_at(&states, &name, StreamKind::Recorded) {
            streams.push((source, StreamKind::Recorded, start));
        }
        for recorder in selected
            .iter()
            .copied()
            .filter(|other| other.name() != &name)
        {
            if let Some(start) = resume_at(&states, recorder.name(), StreamKind::Items) {
                streams.push((recorder, StreamKind::Items, start));
            }
        }

        let counters: Vec<AtomicU32> = streams.iter().map(|_| AtomicU32::new(0)).collect();
        let mut walked = Vec::new();
        let mut outcomes = Vec::new();
        for ((stream, kind, start), calls) in streams.iter().zip(&counters) {
            let near = GlobalId::new(name.clone(), request.id.native.clone());
            walked.push(match kind {
                StreamKind::Recorded => {
                    recorded_on(stream, &request.id.native, start, budget, calls).await
                }
                _ if stream.name() == &name => {
                    fetch_edges(
                        stream,
                        &request.id.native,
                        request.direction,
                        entity,
                        emulating,
                        start,
                        budget,
                        calls,
                    )
                    .await
                }
                _ => recorded_for(stream, &near, start, budget, calls).await,
            });
            outcomes.push(if stream.name() == &name && *kind == StreamKind::Items {
                own.clone()
            } else {
                Outcomes::default()
            });
        }
        // An opaque far end is answered as the source its designator is, and one no configured
        // source is leaves the page and is named instead.
        let mut resolving = designators::Resolving::default();
        for fetched in walked.iter_mut().flatten() {
            let mut kept = Vec::with_capacity(fetched.rows.len());
            for mut row in std::mem::take(&mut fetched.rows) {
                if let Some(to) = self
                    .resolve_endpoint(row.item.to.clone(), &mut resolving)
                    .await
                {
                    row.item.to = to;
                    kept.push(row);
                }
            }
            fetched.rows = kept;
        }
        let ready: Vec<&ResolvedSource> = streams.iter().map(|(stream, _, _)| *stream).collect();
        let kinds: Vec<StreamKind> = streams.iter().map(|(_, kind, _)| *kind).collect();

        let streams = answer.collect_streams(&ready, &kinds, walked, &counters, outcomes);
        let mut response = answer.finish(
            streams,
            budget,
            owed(&states),
            &query,
            |name, edge: DependencyEdge| QualifiedEdge {
                from: qualify_endpoint(name, edge.from),
                to: qualify_endpoint(name, edge.to),
                kind: edge.kind,
            },
        )?;
        // A source declared private that could not be asked what it records of a task held
        // elsewhere did not fail this read, unless the caller asked for every source's part.
        if entity == Entity::Task && request.completeness == Completeness::NamingUnasked {
            let (unasked, failed) = std::mem::take(&mut response.errors)
                .into_iter()
                .partition(|failure| self.records_unasked(&name, failure));
            response.errors = failed;
            response.unanswered = unasked.into_iter().map(Unanswered::Source).collect();
        }
        if request.completeness == Completeness::Required {
            resolving.required(&request.id.to_string(), &mut response.errors)?;
        } else {
            response.errors.extend(resolving.failed);
            response.unanswered.extend(resolving.unanswered);
        }
        Ok(response)
    }

    /// The names a request addresses: the ones it gave, or the configuration's own.
    fn resolve_selection(&self, asked: &[SourceName]) -> Result<Vec<SourceName>, EngineError> {
        if asked.is_empty() {
            if self.selection.is_empty() {
                return Err(EngineError::NoSources);
            }
            return self.selection.iter().map(|name| self.known(name)).collect();
        }
        asked.iter().map(|name| self.known(name)).collect()
    }

    /// `name` when this configuration has a source called that.
    fn known(&self, name: &SourceName) -> Result<SourceName, EngineError> {
        if self.has(name) {
            return Ok(name.clone());
        }
        if self.incomplete.contains(name) {
            return Err(EngineError::IncompleteSource { name: name.clone() });
        }
        if self.sources.is_empty() {
            return Err(EngineError::NoSources);
        }
        Err(EngineError::UnknownSource {
            name: name.to_string(),
            configured: self
                .listing()
                .iter()
                .map(|listing| listing.source.to_string())
                .collect::<Vec<_>>()
                .join(", "),
        })
    }
}

fn qualify_endpoint(
    source: &SourceName,
    endpoint: onetaskgraph_plugin_api::DependencyEndpoint,
) -> QualifiedEndpoint {
    let kind = endpoint.kind;
    let is_qualified = endpoint.is_qualified();
    let endpoint_id = endpoint.into_id();
    QualifiedEndpoint {
        id: if is_qualified {
            endpoint_id
                .parse()
                .expect("plugin-api validates qualified dependency endpoints")
        } else {
            GlobalId::new(source.clone(), NativeId(endpoint_id))
        },
        kind,
    }
}

/// Which of a source's two dependency graphs a request walks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Entity {
    /// Task dependencies.
    Task,
    /// Project dependencies.
    Project,
}

/// A search hit before it is qualified.
enum Found {
    /// A task matched.
    Task(Task),
    /// A project matched.
    Project(Project),
}

/// What happened to one predicate against one source.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Outcome {
    /// Applied by the source itself.
    PushedDown,
    /// Applied by the engine over a wider result set.
    AppliedLocally,
    /// Answered by a bounded scan of the source.
    Emulated,
    /// Neither side could answer it, so this source contributed nothing for it.
    Unavailable,
}

/// What happened to each predicate against one source.
///
/// Keyed by predicate, one outcome each, because those are the only states there are: a
/// predicate the source applied was not also applied here, and one nobody could answer
/// was not also pushed down. The four lists [`SourcePlan`] carries are this map fanned
/// out at the boundary — held *as* four lists a predicate could sit in all four at once,
/// and four contradictory claims about one predicate is the one thing the part of the
/// answer whose whole job is to say which of them is true must not be able to say.
///
/// A `BTreeMap` rather than a `HashMap` so the lists come out in one order and two runs
/// of a query render the same plan.
#[derive(Debug, Clone, Default, PartialEq)]
struct Outcomes(BTreeMap<Predicate, Outcome>);

impl Outcomes {
    /// Record what happened to one predicate, replacing whatever was recorded before.
    ///
    /// Replacing rather than refusing: shaping a query decides each predicate once, and a
    /// second decision about the same one is the later one — there is no case here where
    /// both were meant to stand.
    fn record(&mut self, predicate: Predicate, outcome: Outcome) {
        self.0.insert(predicate, outcome);
    }

    /// Record the same outcome for several predicates, as a text search does for the two
    /// fields it covers.
    fn record_all(&mut self, predicates: impl IntoIterator<Item = Predicate>, outcome: Outcome) {
        for predicate in predicates {
            self.record(predicate, outcome);
        }
    }

    /// The predicates this outcome befell, in the map's own stable order.
    fn with(&self, outcome: Outcome) -> Vec<Predicate> {
        self.0
            .iter()
            .filter(|(_, recorded)| **recorded == outcome)
            .map(|(predicate, _)| *predicate)
            .collect()
    }
}

/// The query one source sees, and the predicates left to the engine.
struct TaskShape {
    /// What the source is asked.
    pushed: TaskQuery,
    /// What the engine narrows afterwards.
    local: LocalTasks,
    /// What to report.
    outcomes: Outcomes,
}

/// As [`TaskShape`], for projects.
struct ProjectShape {
    /// What the source is asked.
    pushed: ProjectAsk,
    /// What the engine narrows afterwards.
    local: LocalProjects,
    /// What to report.
    outcomes: Outcomes,
}

/// Which of its two project listings a source is asked: one query, or every project copied
/// from one origin — never both, because the second carries nothing of the first.
enum ProjectAsk {
    /// [`TaskSource::query_projects`] with this query.
    Query(ProjectQuery),
    /// [`TaskSource::query_projects_by_origin`] for this origin.
    Origin(GlobalId),
}

/// As [`TaskShape`], for documents.
struct DocumentShape {
    /// What the source is asked.
    pushed: DocumentQuery,
    /// What the engine narrows afterwards.
    local: LocalDocuments,
    /// What to report.
    outcomes: Outcomes,
}

/// As [`TaskShape`], for one entity's half of a search.
struct HitShape {
    /// Which entity this stream reads.
    stream: StreamKind,
    /// What a task stream asks.
    tasks: TaskQuery,
    /// What a project stream asks.
    projects: ProjectQuery,
    /// What the engine narrows afterwards, for tasks.
    local_tasks: LocalTasks,
    /// What the engine narrows afterwards, for projects.
    local_projects: LocalProjects,
    /// What to report.
    outcomes: Outcomes,
}

/// The plan and the failures a response carries, accumulated as the verb runs.
///
/// One type rather than three parallel vectors threaded through every verb, because the
/// rule they enforce together is one rule: a source that fails contributes an error and
/// still leaves every other source's results standing.
struct Answer {
    /// One entry per source the engine addressed, merged by source at the end.
    plans: Vec<SourcePlan>,
    /// Every source that could not answer.
    errors: Vec<SourceFailure>,
}

impl Answer {
    fn new() -> Self {
        Self {
            plans: Vec::new(),
            errors: Vec::new(),
        }
    }

    /// The selected sources that built, recording the ones that did not as failures.
    ///
    /// A source that never built is reported and skipped rather than fatal: that is the
    /// same rule as a source failing mid-query, applied one step earlier.
    fn split<'a>(&mut self, engine: &'a Engine, names: &[SourceName]) -> Vec<&'a ResolvedSource> {
        let mut selected = Vec::new();
        for name in names {
            match engine.sources.iter().find(|source| source.name() == name) {
                Some(ConfiguredSource::Ready(source)) => selected.push(source),
                Some(ConfiguredSource::Unavailable(source)) => {
                    self.errors.push(source.failure());
                }
                None => {}
            }
        }
        selected
    }

    /// Record that a source could answer nothing for `predicate`.
    fn unreachable_predicate(&mut self, source: &ResolvedSource, predicate: Predicate) {
        let mut outcomes = Outcomes::default();
        outcomes.record(predicate, Outcome::Unavailable);
        self.plans.push(plan_for(source, outcomes, 0));
    }

    /// Turn each source's walk into a stream, keeping a failed source's failure.
    fn collect<T>(
        &mut self,
        ready: &[&ResolvedSource],
        walked: Vec<Result<Fetched<T>, SourceError>>,
        counters: &[AtomicU32],
        outcomes: Vec<Outcomes>,
    ) -> Vec<Stream<T>> {
        let kinds = vec![StreamKind::Items; ready.len()];
        self.collect_streams(ready, &kinds, walked, counters, outcomes)
    }

    /// As [`collect`](Self::collect), where a source may contribute more than one stream.
    fn collect_streams<T>(
        &mut self,
        ready: &[&ResolvedSource],
        kinds: &[StreamKind],
        walked: Vec<Result<Fetched<T>, SourceError>>,
        counters: &[AtomicU32],
        outcomes: Vec<Outcomes>,
    ) -> Vec<Stream<T>> {
        let mut streams = Vec::new();
        for (index, result) in walked.into_iter().enumerate() {
            let source = ready[index];
            let pages = counters[index].load(Ordering::Relaxed);
            self.plans
                .push(plan_for(source, outcomes[index].clone(), pages));
            match result {
                Ok(fetched) => streams.push(Stream {
                    source: source.name().clone(),
                    kind: kinds[index],
                    fetched,
                }),
                // A stream that failed leaves the token, so a walk always terminates: a
                // source failing on every page would otherwise page forever.
                Err(error) => self.errors.push(SourceFailure {
                    source: source.name().clone(),
                    error,
                }),
            }
        }
        streams
    }

    /// The response for a verb that reads exactly one item from exactly one source.
    fn one<T, U>(
        self,
        source: &ResolvedSource,
        found: Result<Option<T>, SourceError>,
        qualify: impl FnOnce(T) -> U,
    ) -> Result<QueryResponse<U>, EngineError> {
        Ok(self.one_response(source, found, qualify))
    }

    /// [`one`](Self::one), for a caller with no refusal to thread through.
    fn one_response<T, U>(
        mut self,
        source: &ResolvedSource,
        found: Result<Option<T>, SourceError>,
        qualify: impl FnOnce(T) -> U,
    ) -> QueryResponse<U> {
        self.plans.push(plan_for(source, Outcomes::default(), 1));
        let items = match found {
            Ok(Some(item)) => vec![qualify(item)],
            Ok(None) => Vec::new(),
            Err(error) => {
                self.errors.push(SourceFailure {
                    source: source.name().clone(),
                    error,
                });
                Vec::new()
            }
        };
        QueryResponse {
            items,
            next: None,
            plan: QueryPlan {
                per_source: merge_plans(self.plans),
            },
            errors: self.errors,
            unanswered: Vec::new(),
        }
    }

    /// The response for a verb with nothing left to ask.
    fn nothing<U>(self) -> Result<QueryResponse<U>, EngineError> {
        Ok(QueryResponse {
            items: Vec::new(),
            next: None,
            plan: QueryPlan {
                per_source: merge_plans(self.plans),
            },
            errors: self.errors,
            unanswered: Vec::new(),
        })
    }

    /// Merge the streams into the caller's page and mint the token that resumes it.
    ///
    /// `first` is the stream the token being resumed says is owed the next row, so the
    /// round-robin picks up where the previous page stopped rather than restarting.
    fn finish<T, U>(
        self,
        streams: Vec<Stream<T>>,
        budget: u32,
        first: Option<&Owed>,
        query: &str,
        qualify: impl Fn(&SourceName, T) -> U,
    ) -> Result<QueryResponse<U>, EngineError> {
        let (rows, states, owed) = merge(streams, budget, first);
        let next = (!states.is_empty()).then(|| PageToken::encode(query, owed, &states));
        Ok(QueryResponse {
            items: rows
                .into_iter()
                .map(|(name, item)| qualify(&name, item))
                .collect(),
            next,
            plan: QueryPlan {
                per_source: merge_plans(self.plans),
            },
            errors: self.errors,
            unanswered: Vec::new(),
        })
    }
}

/// One source's plan entry: the outcomes fanned out into the four lists the contract's
/// [`SourcePlan`] carries, each in one order so two runs read the same.
fn plan_for(source: &ResolvedSource, outcomes: Outcomes, pages: u32) -> SourcePlan {
    SourcePlan {
        source: source.name().clone(),
        kind: source.kind().to_owned(),
        pushed_down: outcomes.with(Outcome::PushedDown),
        applied_locally: outcomes.with(Outcome::AppliedLocally),
        emulated: outcomes.with(Outcome::Emulated),
        unavailable: outcomes.with(Outcome::Unavailable),
        pages_fetched: pages,
    }
}

/// One entry per source, however many streams that source contributed.
///
/// `search --kind both` reads two streams from each source, and a plan is per source:
/// two entries for one name would say the engine addressed it twice.
fn merge_plans(plans: Vec<SourcePlan>) -> Vec<SourcePlan> {
    let mut merged: Vec<SourcePlan> = Vec::new();
    for plan in plans {
        if let Some(existing) = merged
            .iter_mut()
            .find(|existing| existing.source == plan.source)
        {
            existing.pushed_down.extend(plan.pushed_down);
            existing.applied_locally.extend(plan.applied_locally);
            existing.emulated.extend(plan.emulated);
            existing.unavailable.extend(plan.unavailable);
            existing.pages_fetched = existing.pages_fetched.saturating_add(plan.pages_fetched);
            for list in [
                &mut existing.pushed_down,
                &mut existing.applied_locally,
                &mut existing.emulated,
                &mut existing.unavailable,
            ] {
                list.sort_unstable();
                list.dedup();
            }
        } else {
            merged.push(plan);
        }
    }
    merged
}

/// The sources still walking, with where each picks up.
///
/// A token names every stream that has more to give, so a source **absent** from one has
/// been exhausted and is not asked again. Without that, the second page of a walk would
/// restart every finished source from its first row.
fn walking<'a>(
    selected: Vec<&'a ResolvedSource>,
    states: &Option<Resumption>,
    kind: StreamKind,
) -> (Vec<&'a ResolvedSource>, Vec<Resume>) {
    let mut ready = Vec::new();
    let mut starts = Vec::new();
    for source in selected {
        if let Some(resume) = resume_at(states, source.name(), kind) {
            ready.push(source);
            starts.push(resume);
        }
    }
    (ready, starts)
}

/// A fingerprint of everything about a query that decides which rows it returns, and in
/// what order — the verb, the sources it addresses, and every filter it carries.
///
/// Written from the request's own [`Debug`] rather than field by field, and that is the
/// point: a filter added to `Filters` next year joins the fingerprint by existing. A
/// hand-written canonical form would keep compiling with the new field missing, and the
/// tokens it minted would silently stop distinguishing the queries that differ by it —
/// which is the whole failure this exists to prevent, reintroduced quietly.
///
/// Hashed rather than carried whole so a token stays a thing a person can paste. This is
/// not a signature and there is nothing secret in a token — see [`PageToken`]. It detects
/// a caller resuming the wrong walk, which is a mistake rather than an attack, so FNV-1a
/// is enough and needs no dependency the supply-chain gate would then have to weigh.
///
/// [`Debug`] output is not promised to be stable across compiler releases, and that is
/// survivable here: a token outstanding across a rebuild is refused with the message
/// above rather than honoured wrongly, which is the safe direction to fail in.
fn shape(verb: &str, sources: &[SourceName], filters: &impl std::fmt::Debug) -> String {
    let names: Vec<&str> = sources.iter().map(SourceName::as_str).collect();
    fingerprint(&format!("{verb}|{names:?}|{filters:?}"))
}

/// FNV-1a over `text`, as sixteen hex digits.
fn fingerprint(text: &str) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in text.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{hash:016x}")
}

/// The stream a token says is owed the next row, or `None` when it says none is.
///
/// A fresh query has no token and every token whose last round ended evenly carries no
/// such stream, so `None` is the common case and means "begin at the first stream".
fn owed(document: &Option<Resumption>) -> Option<&Owed> {
    document.as_ref()?.owed.as_ref()
}

/// Where one stream picks up, or `None` when the token says it is finished.
fn resume_at(states: &Option<Resumption>, source: &SourceName, kind: StreamKind) -> Option<Resume> {
    match states {
        None => Some(Resume::default()),
        Some(document) => document
            .streams
            .iter()
            .find(|state| &state.source == source && state.stream == kind)
            .map(|state| state.resume.clone()),
    }
}

/// Read a caller's page token against the sources this configuration has.
///
/// [`PageToken::parse`] has already established that the string is this engine's own
/// resume document — that is structural and happens where the caller's string enters.
/// What it cannot establish is that the document belongs *here*, because only the engine
/// knows which sources are configured and what page ceiling each one declares. So the
/// three things a token this engine wrote is always true of are checked here:
///
/// 1. every stream it names belongs to a configured source, so a token carried over from
///    another configuration is refused rather than quietly resuming half a walk;
/// 2. every stream it names is one **this verb reads**. A token minted by
///    `search --kind both` carries task and project streams, and `task list` reads
///    neither; without this it would find nothing to resume, drop every source, and
///    answer with an empty page and a zero exit — a wrong answer that looks like an
///    exhausted walk, which is the worst shape this failure could take;
/// 3. no stream appears twice, because a walk has one place to pick up per stream;
/// 4. no `skip` reaches a source's declared page ceiling, because the engine's `skip` is
///    an index among the surviving rows of one source page and can never reach it.
///
/// 5. the stream owed the next row, when the document names one, is a stream the
///    document also resumes.
///
/// A sixth thing needs no check here: at most one stream is owed the next row, because
/// [`Resumption`] holds that as one optional stream rather than as a flag on each of
/// them, so a document naming two has no spelling.
///
/// None of this is a security boundary and a page token is not a credential: nothing in
/// one is secret, and a forged cursor is handed straight back to the source that would
/// have issued it and refused there. What it buys is that a stale or hand-edited token
/// fails saying so, instead of silently returning a page from somewhere else in the walk.
fn resumption(
    engine: &Engine,
    token: Option<&PageToken>,
    reads: &[StreamKind],
    query: &str,
) -> Result<Option<Resumption>, EngineError> {
    let Some(document) = token.map(PageToken::decode) else {
        return Ok(None);
    };

    // Every cursor below is an offset into the result set *one* query produced. Handed to
    // a different one — the same verb with another `--label`, another `--search`, another
    // `--direction` — each source picks up at a position that meant something in a walk
    // the caller is no longer doing, and the rows that come back are real rows at exit
    // zero. Nothing about that answer says it is arbitrary, which is what makes it worth
    // refusing rather than serving.
    if document.query != query {
        return Err(EngineError::Token {
            message: "this page token was written by a different query — resume the walk it \
                      came from, or drop --page to start this one from the beginning"
                .to_owned(),
        });
    }
    let states = &document.streams;

    let mut seen: Vec<(&SourceName, StreamKind)> = Vec::new();
    for state in states {
        if !reads.contains(&state.stream) {
            return Err(EngineError::Token {
                message: format!(
                    "this page token resumes {}, which this command does not read — it \
                     was written by a different query",
                    state.stream.describe()
                ),
            });
        }
        let ceiling = engine
            .ready()
            .find(|source| source.name() == &state.source)
            .map(ceiling);
        if ceiling.is_none() && !engine.has(&state.source) {
            return Err(EngineError::Token {
                message: format!(
                    "this page token resumes a source called {:?}, which this \
                     configuration does not have",
                    state.source.as_str()
                ),
            });
        }
        if let Some(ceiling) = ceiling
            && state.resume.skip >= ceiling
        {
            return Err(EngineError::Token {
                message: format!(
                    "this page token resumes {} rows into a page of source {:?}, which \
                     serves at most {ceiling}",
                    state.resume.skip,
                    state.source.as_str()
                ),
            });
        }
        if seen.contains(&(&state.source, state.stream)) {
            return Err(EngineError::Token {
                message: format!(
                    "this page token gives source {:?} two places to resume from",
                    state.source.as_str()
                ),
            });
        }
        seen.push((&state.source, state.stream));
    }

    // The stream owed the next row has to be one of the streams this document resumes.
    // Ignoring a stray one would be harmless in its effect — the merge would start at the
    // first stream instead — but it would be a value from outside accepted without a
    // reading, and the next thing to depend on it would inherit that.
    if let Some(owed) = &document.owed
        && !document
            .streams
            .iter()
            .any(|state| state.source == owed.source && state.stream == owed.stream)
    {
        return Err(EngineError::Token {
            message: format!(
                "this page token owes the next row to a stream it does not resume, \
                 {:?}'s {}",
                owed.source.as_str(),
                owed.stream.describe()
            ),
        });
    }

    Ok(Some(document))
}

/// Which project a task must belong to, as a source sees it.
///
/// A qualified id becomes a plain native one because by the time this runs the selection
/// holds only that id's own source — so there is no "some other source" case to get
/// wrong, and none to leave untested.
fn project_filter(selector: &ProjectSelector) -> ProjectFilter {
    match selector {
        ProjectSelector::Any => ProjectFilter::Any,
        ProjectSelector::Orphans => ProjectFilter::Orphans,
        ProjectSelector::Native(id) => ProjectFilter::Is(id.clone()),
        ProjectSelector::Qualified(id) => ProjectFilter::Is(id.native.clone()),
    }
}

/// The predicates one text query is made of.
fn text_predicates(fields: TextFields) -> Vec<Predicate> {
    match fields {
        TextFields::Title => vec![Predicate::SearchTitle],
        TextFields::Content => vec![Predicate::SearchContent],
        TextFields::TitleOrContent => vec![Predicate::SearchTitle, Predicate::SearchContent],
    }
}

/// Whether a source searches **every** field this query names.
///
/// Every, not any: a `title-or-content` search pushed to a source that searches only
/// titles would come back missing every row that matches in the body alone — a narrower
/// result than the truth, which is the one thing compensation cannot repair. So a
/// half-capable source is not asked at all and the engine searches both fields itself.
fn searches_natively(capabilities: &Capabilities, fields: TextFields) -> bool {
    match fields {
        TextFields::Title => capabilities.search_title.is_native(),
        TextFields::Content => capabilities.search_content.is_native(),
        TextFields::TitleOrContent => {
            capabilities.search_title.is_native() && capabilities.search_content.is_native()
        }
    }
}

/// Split a task query between the source and the engine.
fn shape_tasks(
    capabilities: &Capabilities,
    filters: &Filters,
    project: &ProjectFilter,
    priorities: &[Priority],
    commented_since: Option<DateTime<Utc>>,
    metadata: &[MetadataMatch],
    origin: Option<&GlobalId>,
) -> TaskShape {
    let mut pushed = TaskQuery::default();
    let mut local = LocalTasks::default();
    let mut outcomes = Outcomes::default();

    if !filters.labels.is_empty() {
        if capabilities.filter_by_label.is_native() {
            pushed.labels = filters.labels.clone();
            outcomes.record(Predicate::Label, Outcome::PushedDown);
        } else {
            local.labels = Some(filters.labels.clone());
            outcomes.record(Predicate::Label, Outcome::AppliedLocally);
        }
    }
    if !filters.statuses.is_empty() {
        if capabilities.filter_by_status.is_native() {
            pushed.statuses.clone_from(&filters.statuses);
            outcomes.record(Predicate::Status, Outcome::PushedDown);
        } else {
            local.statuses.clone_from(&filters.statuses);
            outcomes.record(Predicate::Status, Outcome::AppliedLocally);
        }
    }
    if !priorities.is_empty() {
        if capabilities.filter_by_priority.is_native() {
            pushed.priorities = priorities.to_vec();
            outcomes.record(Predicate::Priority, Outcome::PushedDown);
        } else {
            local.priorities = priorities.to_vec();
            outcomes.record(Predicate::Priority, Outcome::AppliedLocally);
        }
    }
    if let Some(since) = commented_since {
        if capabilities.filter_by_comment_activity.is_native() {
            pushed.commented_since = Some(since);
            outcomes.record(Predicate::CommentedSince, Outcome::PushedDown);
        } else {
            local.commented_since = Some(since);
            outcomes.record(Predicate::CommentedSince, Outcome::AppliedLocally);
        }
    }
    if !metadata.is_empty() {
        if capabilities.filter_by_metadata.is_native() {
            pushed.metadata = metadata.to_vec();
            outcomes.record(Predicate::Metadata, Outcome::PushedDown);
        } else {
            local.metadata = metadata.to_vec();
            outcomes.record(Predicate::Metadata, Outcome::AppliedLocally);
        }
    }
    if let Some(origin) = origin {
        if capabilities.filter_by_origin.is_native() {
            // The qualified id as a copy stores it: a source compares the string and never
            // parses it.
            pushed.origin = Some(origin.to_string());
            outcomes.record(Predicate::Origin, Outcome::PushedDown);
        } else {
            local.origin = Some(origin.clone());
            outcomes.record(Predicate::Origin, Outcome::AppliedLocally);
        }
    }
    if let Some(text) = &filters.text {
        let predicates = text_predicates(text.fields);
        if searches_natively(capabilities, text.fields) {
            pushed.text = Some(text.clone());
            outcomes.record_all(predicates, Outcome::PushedDown);
        } else {
            local.text = Some(text.clone());
            outcomes.record_all(predicates, Outcome::AppliedLocally);
        }
    }
    match project {
        ProjectFilter::Any => {}
        ProjectFilter::Orphans => {
            if capabilities.orphan_tasks.is_native() {
                pushed.project = ProjectFilter::Orphans;
                outcomes.record(Predicate::Project, Outcome::PushedDown);
            } else {
                local.project = Some(ProjectFilter::Orphans);
                outcomes.record(Predicate::Project, Outcome::AppliedLocally);
            }
        }
        ProjectFilter::Is(id) => {
            if capabilities.projects.is_native() {
                pushed.project = ProjectFilter::Is(id.clone());
                outcomes.record(Predicate::Project, Outcome::PushedDown);
            } else {
                local.project = Some(ProjectFilter::Is(id.clone()));
                outcomes.record(Predicate::Project, Outcome::AppliedLocally);
            }
        }
    }

    TaskShape {
        pushed,
        local,
        outcomes,
    }
}

/// Split a project query between the source and the engine.
///
/// A listing by copy origin asks the source [`TaskSource::query_projects_by_origin`], which
/// carries the origin and nothing else, so every other predicate of it is the engine's to
/// apply — and the origin itself is checked again here, so a source answering wider than
/// the method states cannot widen the answer.
fn shape_projects(
    capabilities: &Capabilities,
    filters: &Filters,
    origin: Option<&GlobalId>,
) -> ProjectShape {
    if let Some(origin) = origin {
        let mut local = LocalProjects::default();
        let mut outcomes = Outcomes::default();
        local.labels = (!filters.labels.is_empty()).then(|| filters.labels.clone());
        local.statuses.clone_from(&filters.statuses);
        local.text.clone_from(&filters.text);
        local.origin = Some(origin.clone());
        if !filters.labels.is_empty() {
            outcomes.record(Predicate::Label, Outcome::AppliedLocally);
        }
        if !filters.statuses.is_empty() {
            outcomes.record(Predicate::Status, Outcome::AppliedLocally);
        }
        if let Some(text) = &filters.text {
            outcomes.record_all(text_predicates(text.fields), Outcome::AppliedLocally);
        }
        outcomes.record(Predicate::Origin, Outcome::PushedDown);
        return ProjectShape {
            pushed: ProjectAsk::Origin(origin.clone()),
            local,
            outcomes,
        };
    }
    let (pushed, local, outcomes) = shape_project_query(capabilities, filters);
    ProjectShape {
        pushed: ProjectAsk::Query(pushed),
        local,
        outcomes,
    }
}

/// Split a project query that names no origin between the source and the engine.
fn shape_project_query(
    capabilities: &Capabilities,
    filters: &Filters,
) -> (ProjectQuery, LocalProjects, Outcomes) {
    let mut pushed = ProjectQuery::default();
    let mut local = LocalProjects::default();
    let mut outcomes = Outcomes::default();
    if !filters.labels.is_empty() {
        if capabilities.filter_by_label.is_native() {
            pushed.labels = filters.labels.clone();
            outcomes.record(Predicate::Label, Outcome::PushedDown);
        } else {
            local.labels = Some(filters.labels.clone());
            outcomes.record(Predicate::Label, Outcome::AppliedLocally);
        }
    }
    if !filters.statuses.is_empty() {
        if capabilities.filter_by_status.is_native() {
            pushed.statuses.clone_from(&filters.statuses);
            outcomes.record(Predicate::Status, Outcome::PushedDown);
        } else {
            local.statuses.clone_from(&filters.statuses);
            outcomes.record(Predicate::Status, Outcome::AppliedLocally);
        }
    }
    if let Some(text) = &filters.text {
        let predicates = text_predicates(text.fields);
        if searches_natively(capabilities, text.fields) {
            pushed.text = Some(text.clone());
            outcomes.record_all(predicates, Outcome::PushedDown);
        } else {
            local.text = Some(text.clone());
            outcomes.record_all(predicates, Outcome::AppliedLocally);
        }
    }
    (pushed, local, outcomes)
}

/// Split a document query between the source and the engine.
///
/// The same three predicates a task query is split by, minus the status filter a document
/// has nothing to compare against.
fn shape_documents(
    capabilities: &Capabilities,
    filters: &DocumentFilters,
    project: &ProjectFilter,
) -> DocumentShape {
    let mut pushed = DocumentQuery::default();
    let mut local = LocalDocuments::default();
    let mut outcomes = Outcomes::default();

    if !filters.labels.is_empty() {
        if capabilities.filter_by_label.is_native() {
            pushed.labels = filters.labels.clone();
            outcomes.record(Predicate::Label, Outcome::PushedDown);
        } else {
            local.labels = Some(filters.labels.clone());
            outcomes.record(Predicate::Label, Outcome::AppliedLocally);
        }
    }
    if let Some(text) = &filters.text {
        let predicates = text_predicates(text.fields);
        if searches_natively(capabilities, text.fields) {
            pushed.text = Some(text.clone());
            outcomes.record_all(predicates, Outcome::PushedDown);
        } else {
            local.text = Some(text.clone());
            outcomes.record_all(predicates, Outcome::AppliedLocally);
        }
    }
    match project {
        ProjectFilter::Any => {}
        ProjectFilter::Orphans => {
            if capabilities.orphan_tasks.is_native() {
                pushed.project = ProjectFilter::Orphans;
                outcomes.record(Predicate::Project, Outcome::PushedDown);
            } else {
                local.project = Some(ProjectFilter::Orphans);
                outcomes.record(Predicate::Project, Outcome::AppliedLocally);
            }
        }
        ProjectFilter::Is(id) => {
            if capabilities.projects.is_native() {
                pushed.project = ProjectFilter::Is(id.clone());
                outcomes.record(Predicate::Project, Outcome::PushedDown);
            } else {
                local.project = Some(ProjectFilter::Is(id.clone()));
                outcomes.record(Predicate::Project, Outcome::AppliedLocally);
            }
        }
    }

    DocumentShape {
        pushed,
        local,
        outcomes,
    }
}

/// Split one entity's half of a search between the source and the engine.
fn shape_hits(capabilities: &Capabilities, filters: &Filters, stream: StreamKind) -> HitShape {
    match stream {
        StreamKind::Projects => {
            let (projects, local_projects, outcomes) = shape_project_query(capabilities, filters);
            HitShape {
                stream,
                tasks: TaskQuery::default(),
                projects,
                local_tasks: LocalTasks::default(),
                local_projects,
                outcomes,
            }
        }
        StreamKind::Items | StreamKind::Tasks | StreamKind::Recorded => {
            let shaped = shape_tasks(
                capabilities,
                filters,
                &ProjectFilter::Any,
                &[],
                None,
                &[],
                None,
            );
            HitShape {
                stream,
                tasks: shaped.pushed,
                projects: ProjectQuery::default(),
                local_tasks: shaped.local,
                local_projects: LocalProjects::default(),
                outcomes: shaped.outcomes,
            }
        }
    }
}

/// How large a page to ask a source for.
///
/// Exactly what is needed when every predicate went down, and the source's own ceiling
/// when the engine is narrowing — because a compensating walk cannot know how many rows
/// of a page will survive, and asking for the caller's limit would turn one filtered
/// page into a page per surviving row.
fn page_size(compensating: bool, budget: u32, ceiling: u32) -> u32 {
    if compensating {
        ceiling
    } else {
        budget.min(ceiling)
    }
}

/// The largest page this source will serve, never zero.
fn ceiling(source: &ResolvedSource) -> u32 {
    source.source().capabilities().max_page_size.max(1)
}

/// Walk one source's tasks, narrowing whatever it did not apply itself.
async fn fetch_tasks(
    source: &ResolvedSource,
    shape: &TaskShape,
    start: &Resume,
    budget: u32,
    calls: &AtomicU32,
) -> Result<Fetched<Task>, SourceError> {
    let compensating = shape.local != LocalTasks::default();
    walk(
        start,
        budget,
        page_size(compensating, budget, ceiling(source)),
        |task| shape.local.keeps(task),
        |cursor, limit| async move {
            calls.fetch_add(1, Ordering::Relaxed);
            let request = PageRequest { cursor, limit };
            let page = source.source().query_tasks(&shape.pushed, &request).await?;
            match shape.local.commented_since {
                None => Ok(page),
                Some(since) => comment::commented_since(source, &shape.local, page, since).await,
            }
        },
    )
    .await
}

/// Walk one source's projects, narrowing whatever it did not apply itself.
async fn fetch_projects(
    source: &ResolvedSource,
    shape: &ProjectShape,
    start: &Resume,
    budget: u32,
    calls: &AtomicU32,
) -> Result<Fetched<Project>, SourceError> {
    let compensating = shape.local != LocalProjects::default();
    walk(
        start,
        budget,
        page_size(compensating, budget, ceiling(source)),
        |project| shape.local.keeps(project),
        |cursor, limit| async move {
            calls.fetch_add(1, Ordering::Relaxed);
            let request = PageRequest { cursor, limit };
            match &shape.pushed {
                ProjectAsk::Origin(origin) => {
                    source
                        .source()
                        .query_projects_by_origin(&origin.to_string(), &request)
                        .await
                }
                ProjectAsk::Query(query) => source.source().query_projects(query, &request).await,
            }
        },
    )
    .await
}

/// Walk one source's documents, narrowing whatever it did not apply itself.
async fn fetch_documents(
    source: &ResolvedSource,
    shape: &DocumentShape,
    start: &Resume,
    budget: u32,
    calls: &AtomicU32,
) -> Result<Fetched<Document>, SourceError> {
    let compensating = shape.local != LocalDocuments::default();
    walk(
        start,
        budget,
        page_size(compensating, budget, ceiling(source)),
        |document| shape.local.keeps(document),
        |cursor, limit| async move {
            calls.fetch_add(1, Ordering::Relaxed);
            let request = PageRequest { cursor, limit };
            source
                .source()
                .query_documents(&shape.pushed, &request)
                .await
        },
    )
    .await
}

/// Walk one source's labels. There is no predicate to compensate for.
async fn fetch_labels(
    source: &ResolvedSource,
    start: &Resume,
    budget: u32,
    calls: &AtomicU32,
) -> Result<Fetched<Label>, SourceError> {
    walk(
        start,
        budget,
        page_size(false, budget, ceiling(source)),
        |_| true,
        |cursor, limit| async move {
            calls.fetch_add(1, Ordering::Relaxed);
            let request = PageRequest { cursor, limit };
            source.source().labels(&request).await
        },
    )
    .await
}

/// Walk one entity's half of a search.
async fn fetch_hits(
    source: &ResolvedSource,
    shape: &HitShape,
    start: &Resume,
    budget: u32,
    calls: &AtomicU32,
) -> Result<Fetched<Found>, SourceError> {
    let ceiling = ceiling(source);
    match shape.stream {
        StreamKind::Projects => {
            let compensating = shape.local_projects != LocalProjects::default();
            walk(
                start,
                budget,
                page_size(compensating, budget, ceiling),
                |found| match found {
                    Found::Project(project) => shape.local_projects.keeps(project),
                    Found::Task(_) => true,
                },
                |cursor, limit| async move {
                    calls.fetch_add(1, Ordering::Relaxed);
                    let request = PageRequest { cursor, limit };
                    let page = source
                        .source()
                        .query_projects(&shape.projects, &request)
                        .await?;
                    Ok(Page {
                        items: page.items.into_iter().map(Found::Project).collect(),
                        next: page.next,
                    })
                },
            )
            .await
        }
        StreamKind::Items | StreamKind::Tasks | StreamKind::Recorded => {
            let compensating = shape.local_tasks != LocalTasks::default();
            walk(
                start,
                budget,
                page_size(compensating, budget, ceiling),
                |found| match found {
                    Found::Task(task) => shape.local_tasks.keeps(task),
                    Found::Project(_) => true,
                },
                |cursor, limit| async move {
                    calls.fetch_add(1, Ordering::Relaxed);
                    let request = PageRequest { cursor, limit };
                    let page = source.source().query_tasks(&shape.tasks, &request).await?;
                    Ok(Page {
                        items: page.items.into_iter().map(Found::Task).collect(),
                        next: page.next,
                    })
                },
            )
            .await
        }
    }
}

/// One page of an item's forward edges.
async fn forward_edges(
    source: &ResolvedSource,
    entity: Entity,
    id: &NativeId,
    request: &PageRequest,
) -> Result<Page<DependencyEdge>, SourceError> {
    match entity {
        Entity::Task => {
            source
                .source()
                .task_dependencies(id, Direction::DependsOn, request)
                .await
        }
        Entity::Project => {
            source
                .source()
                .project_dependencies(id, Direction::DependsOn, request)
                .await
        }
    }
}

/// Walk the tasks of `recorder`, a source declared private, that record `near` as depending
/// on them, each as the edge `near` → that task.
///
/// The match goes down when the source applies metadata matches itself, and is applied here
/// either way, so a source that ignores it still answers only what matches.
///
/// # Errors
///
/// What the source returned, and [`SourceError::Malformed`] for a matched task whose record
/// does not name `near` as a task.
async fn recorded_for(
    recorder: &ResolvedSource,
    near: &GlobalId,
    start: &Resume,
    budget: u32,
    calls: &AtomicU32,
) -> Result<Fetched<DependencyEdge>, SourceError> {
    let wanted = dependents::naming(near);
    let pushed = recorder
        .source()
        .capabilities()
        .filter_by_metadata
        .is_native();
    let query = TaskQuery {
        metadata: if pushed {
            vec![wanted.clone()]
        } else {
            Vec::new()
        },
        ..TaskQuery::default()
    };
    let fetched = walk(
        start,
        budget,
        page_size(!pushed, budget, ceiling(recorder)),
        |task: &Task| wanted.holds(&task.metadata),
        |cursor, limit| {
            calls.fetch_add(1, Ordering::Relaxed);
            let query = &query;
            async move {
                recorder
                    .source()
                    .query_tasks(query, &PageRequest { cursor, limit })
                    .await
            }
        },
    )
    .await?;
    // A record the match found is held to naming `near` as a task before it is an edge, so
    // an entry holding the id under another kind is refused rather than drawn.
    let rows = fetched
        .rows
        .into_iter()
        .map(|row| {
            dependents::names(&row.item, near)?;
            Ok(fetch::Row {
                item: dependents::edge(near, row.item.id),
                resume: row.resume,
            })
        })
        .collect::<Result<_, SourceError>>()?;
    Ok(Fetched {
        rows,
        after: fetched.after,
    })
}

/// Walk the dependents task `native` of `source`, a source declared private, records on
/// itself, each as the edge that dependent → `native`.
///
/// One read of the task per page, paged by position: the record is one value of the task's
/// own, so a page boundary is an offset into it.
async fn recorded_on(
    source: &ResolvedSource,
    native: &NativeId,
    start: &Resume,
    budget: u32,
    calls: &AtomicU32,
) -> Result<Fetched<DependencyEdge>, SourceError> {
    walk(
        start,
        budget,
        budget.min(ceiling(source)),
        |_| true,
        |cursor: Option<Cursor>, limit| async move {
            calls.fetch_add(1, Ordering::Relaxed);
            let Some(task) = source.source().get_task(native).await? else {
                return Ok(Page {
                    items: Vec::new(),
                    next: None,
                });
            };
            let edges = dependents::recorded_edges(&task)?;
            let at = cursor
                .as_ref()
                .map_or(Ok(0), |cursor| cursor.0.parse::<usize>())
                .map_err(|_| SourceError::Malformed {
                    message: "a page of recorded dependents resumed at no position".to_owned(),
                })?;
            let until = at.saturating_add(limit as usize).min(edges.len());
            Ok(Page {
                items: edges.get(at..until).unwrap_or_default().to_vec(),
                next: (until < edges.len()).then(|| Cursor(until.to_string())),
            })
        },
    )
    .await
}

/// Walk one item's dependency edges, emulating the reverse direction when the source
/// only reports forward ones.
///
/// The emulation is the bounded page-by-page scan the contract describes: a page of the
/// source's items, each asked for its own forward edges, keeping the ones that point at
/// `native`. It is indexless by construction — nothing is retained between pages beyond
/// the caller's own page — which is why a source that cannot walk backwards costs a scan
/// rather than a stored reverse index.
#[expect(
    clippy::too_many_arguments,
    reason = "every argument is one axis of one walk — the source, the item, the \
              direction, which of its two graphs, whether the reverse is emulated, where \
              to resume, how many rows to return and where to count calls. Grouping them \
              into a struct would name the same eight values one indirection further from \
              the loop that reads them."
)]
async fn fetch_edges(
    source: &ResolvedSource,
    native: &NativeId,
    direction: Direction,
    entity: Entity,
    emulating: bool,
    start: &Resume,
    budget: u32,
    calls: &AtomicU32,
) -> Result<Fetched<DependencyEdge>, SourceError> {
    let ceiling = ceiling(source);
    if !emulating {
        return walk(
            start,
            budget,
            page_size(false, budget, ceiling),
            |_| true,
            |cursor, limit| async move {
                calls.fetch_add(1, Ordering::Relaxed);
                let request = PageRequest { cursor, limit };
                match entity {
                    Entity::Task => {
                        source
                            .source()
                            .task_dependencies(native, direction, &request)
                            .await
                    }
                    Entity::Project => {
                        source
                            .source()
                            .project_dependencies(native, direction, &request)
                            .await
                    }
                }
            },
        )
        .await;
    }

    walk(
        start,
        budget,
        ceiling,
        |_| true,
        |cursor, limit| async move {
            calls.fetch_add(1, Ordering::Relaxed);
            let request = PageRequest { cursor, limit };
            let (ids, next) = match entity {
                Entity::Task => {
                    let page = source
                        .source()
                        .query_tasks(&TaskQuery::default(), &request)
                        .await?;
                    let ids: Vec<NativeId> = page.items.into_iter().map(|task| task.id).collect();
                    (ids, page.next)
                }
                Entity::Project => {
                    let page = source
                        .source()
                        .query_projects(&ProjectQuery::default(), &request)
                        .await?;
                    let ids: Vec<NativeId> =
                        page.items.into_iter().map(|project| project.id).collect();
                    (ids, page.next)
                }
            };

            let mut edges = Vec::new();
            for id in ids {
                let mut inner: Option<Cursor> = None;
                loop {
                    calls.fetch_add(1, Ordering::Relaxed);
                    let request = PageRequest {
                        cursor: inner.clone(),
                        limit,
                    };
                    let page = forward_edges(source, entity, &id, &request).await?;
                    // The inner half of the same bound the walk holds on its own pages:
                    // this scan keeps every matching edge of one source page, so a source
                    // that overruns here overruns the engine's memory just as surely.
                    fits(page.items.len(), limit)?;
                    edges.extend(page.items.into_iter().filter(|edge| &edge.to == native));
                    unrepeated(
                        page.next.as_ref(),
                        inner.as_ref(),
                        "its forward edges were being scanned",
                    )?;
                    match page.next {
                        Some(cursor) => inner = Some(cursor),
                        None => break,
                    }
                }
            }

            Ok(Page { items: edges, next })
        },
    )
    .await
}
