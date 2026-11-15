//! The onetaskgraph engine.
//!
//! It drives the sources a configuration names and reports what it had to do to
//! answer. Everything a *plugin author* needs lives in `onetaskgraph-plugin-api`
//! instead, and **no plugin crate may depend on this one**: `deny.toml` permits
//! this crate exactly one wrapper, the binary, failing the required `deny` job, and
//! `scripts/check-plugin-isolation.sh` reads the real `cargo metadata` graph inside
//! `just check`.
//!
//! # The invariant that shapes this crate
//!
//! No work data may be stored, cached, indexed or mirrored outside a plugin. The
//! engine compensates for a missing capability *transiently*: it holds at most one
//! source page plus the caller's page and writes nothing down. Three mechanisms
//! enforce that rather than asking for it: `deny.toml` refuses every embedded store,
//! index and cache crate, so reaching for one fails the required `deny` job; a sandboxed
//! journey plants sentinels, drives every verb, and fails if one reaches any file written
//! during the run; and a re-ask test fails if one query asked twice reaches the source
//! once. The latter two land with the verbs they drive.
//!
//! The repository README is included below so its worked Rust example is compiled and
//! executed as a doctest whenever this crate's documentation tests run.
#![doc = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/",
    env!("CARGO_PKG_README")
))]
#![deny(missing_docs)]

pub mod attestation;
pub mod cache;
mod clock;
pub mod config;

mod engine;

mod environment;
pub mod expandable_blocks;
mod failure;
mod global_id;
mod plan;
mod registry;
mod resolve;
mod schema;
mod secrets;
pub mod section_links;
pub mod subprocess;
pub mod task_projection;
// llmlint: ignore[code_lands_in_the_domain_that_owns_it] The task that introduced templates fixes their API at this crate's root; the head of `template/mod.rs` states why.
pub mod template;
pub mod version;
mod yaml;

pub use cache::{
    CacheCleared, CacheUse, ExpectedWrite, Served, WriteFallback, WritePart, WriteResult,
};
pub use clock::{
    ClockChoice, SIMULATED_CLOCK_VARIABLE, attach as attach_simulated_clock, clock_choice,
    process_clock,
};
pub use config::{
    Config, ConfigError, CreateSettings, Loaded, OutputFormat, SourceCacheConfig, SourceConfig,
};
pub use engine::boundary;
pub use engine::{
    AddedComment, BATCH_SCHEMA_VERSION, BatchOutcome, CommentAddResult, CommentBatch,
    CommentBatchItem, CommentsAdded, FindAnswer, FindIn, FindQuery, FindRequest, FoundTasks,
    TasksUpdated, UpdateBatch, UpdateBatchItem, UpdateResult,
};
pub use engine::{AssigneeCondition, TaskAssigneeSet};
pub use engine::{AttestRequest, PlanCreated, PlanMember, PlannedMember, PlannedProject};
pub use engine::{
    Body, BudgetSpent, COMMENTS_SINCE_SCHEMA_VERSION, CommentActivity, CommentCounts, CommentList,
    CommentWritten, CommentsSince, CommentsSinceRequest, CompleteAction, Completeness,
    ConfiguredSource, CopyAction, CopyConditions, CopyItems, CopyLink, CopyLookup, CopyOutcome,
    CopyReport, CopyRequest, CopyScope, CopyVia, CreateHolder, CreatesIn, DeletedComment,
    Delivered, DeliveryOutcome, DependencyRequest, DocumentDetail, DocumentFilters,
    DocumentRequest, Engine, EngineError, Filters, GraphDirection, GraphEdge, GraphNode, GroupKey,
    GuardOutcome, HeldAsset, IssueCompleted, IssuesCompleted, LabelRequest, LeftBehind,
    LocateLookup, LocateReport, LocateRequest, Located, LocatedCounterpart, MatchBy, MetadataSet,
    MirrorTarget, NoCounterpart, NotATask, Otherwise, Oversized, PROJECT_GRAPH_SCHEMA_VERSION,
    Paging, PriorityCondition, PriorityGuard, ProjectGraph, ProjectGraphRequest, ProjectRequest,
    ProjectSelector, ProjectStatusSet, PublicProjection, PublicProjections, Qualified,
    QualifiedEdge, QualifiedEndpoint, ReconcileOutcome, SearchHit, SearchKind, SearchRequest,
    SourceListing, SourceState, Spent, StatusCondition, StatusPolicy, StatusReconciled,
    StatusesReconciled, TaskAssetNames, TaskAssetsError, TaskContentSet, TaskDetail, TaskDetails,
    TaskPrioritySet, TaskRequest, TaskStatusSet, TaskUpdated, graph_label, settled,
};
pub use engine::{
    DocumentCreate, ProjectCreate, Regenerated, Regeneration, RenderRequest, RenderTemplate,
    RenderedRecord, TaskCreate, TaskCreated, TemplateAnswers, UnusedAnswers,
};
pub use engine::{
    FieldOutcome, ItemOutcome, SweepCoverage, SweepField, SweepItem, SweepKind, SweepReport,
    SweepRequests, Uncovered,
};
pub use engine::{
    LinkBatch, LinkItem, LinkOp, LinkOutcome, LinkResult, LinksWritten, OtherEnd, ParentBatch,
    ParentEdge, ParentOutcome, ParentResult, ParentsSet, RELATIONS_SCHEMA_VERSION, ReadLink,
    RelationsItem, TaskRelations,
};
pub use environment::Environment;
pub use failure::{Failure, FailureClass, FailureDocument, classify};
pub use global_id::GlobalId;
/// Named by the answer of [`TaskSource::update_task`], so it lives in the plugin api, and
/// re-exported here because [`TaskUpdated`] reports it.
///
/// [`TaskSource::update_task`]: onetaskgraph_plugin_api::TaskSource::update_task
pub use onetaskgraph_plugin_api::UpdatedField;
pub use plan::{
    PageToken, Predicate, QueryPlan, QueryResponse, SourceFailure, SourcePlan, Unanswered,
    UnresolvedDesignator, UnresolvedReason,
};
pub use registry::{PluginKind, plugin_for, plugin_kinds, registry};
pub use resolve::{
    ResolvedSource, UnavailableSource, resolve, resolve_available, resolve_available_with_clock,
    validate_sources,
};
pub use schema::{SCHEMA_BUNDLE_VERSION, schema_bundle};
pub use secrets::{CredentialLayer, CredentialName, ResolvedCredential, Secrets, SecretsReport};
pub use subprocess::{
    MAX_LINE, Program, RequestDeadline, SubprocessConfig, SubprocessPlugin, SubprocessSource,
    serve, serve_plugin,
};
pub use template::{
    Answers, ChainField, DECLARATION_KEYS, FRONT_MATTER_KEYS, ItemType, LoaderDocument,
    RenderedTemplate, Sha256Digest, Template, TemplateError, TemplateInput, TemplateLoader,
    TemplateProvenance, TemplateVariable, TemplateVariables, VariableType,
};
