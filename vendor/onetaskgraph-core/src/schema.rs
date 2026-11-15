//! The JSON Schema bundle both SDKs are generated from.
//!
//! It is emitted by `onetaskgraph schema` rather than committed, so the schema a
//! consumer generates against can never drift from the types this binary actually
//! serialises: they are the same types.

use std::collections::BTreeMap;

use onetaskgraph_plugin_api::{
    Asset, AssetContentType, AssetName, AssetPayload, AssetUpload, AssetUploads, AssetWrite,
    AssetsWritten, Capabilities, Classification, ClosedAs, Comment, Completion, ContradictedItem,
    DependencyEdge, DependencyEndpoint, DependencyKind, Direction, Document, DocumentQuery, Health,
    HeldTask, IssueHolding, IssueRef, ItemKind, Label, Location, NativeId, NewComment, Page,
    PageRequest, Priority, Project, ProjectQuery, Repository, SourceError, SourceName, Status,
    StatusCategory, StatusContradiction, StatusMapping, Task, TaskQuery, TaskRef, TaskUpdate,
    TaskUpdateOutcome, TextFields, UpdatedField,
};
use schemars::{Schema, schema_for};
use serde_json::{Value, json};

use crate::attestation::{
    Attestation, AttestationCheck, AttestationState, Attested, ProjectAttestationCheck, VoidReason,
};
use crate::boundary::{PublicWriteInput, RepositoryVisibility, WriteVerdict};
use crate::cache::{CacheCleared, CacheUse, ExpectedWrite};
use crate::config::{
    EffectiveConfig, Origin, OutputFormat, Placement, Setting, SourceCacheConfig, SourceRoute,
    SourceVisibility, WritePolicyConfig,
};
use crate::registry::registry;
use crate::secrets::{CredentialLayer, ResolvedCredential, SecretsReport};
use crate::template::{
    ItemType, MemberDeclaration, RenderedTemplate, TemplateProvenance, TemplateVariable,
    TemplateVariables, VariableType,
};
use crate::{
    CommentList, CommentWritten, CommentsSince, CompleteAction, CopyAction, CopyLink, CopyOutcome,
    CopyReport, CopyVia, DeletedComment, Delivered, DeliveryOutcome, DocumentDetail, Failure,
    FailureClass, FailureDocument, GlobalId, IssueCompleted, IssuesCompleted, LocateReport,
    MetadataSet, PageToken, Predicate, ProjectGraph, ProjectStatusSet, PublicProjections,
    Qualified, QualifiedEdge, QualifiedEndpoint, QueryPlan, QueryResponse, ReconcileOutcome,
    Regenerated, SearchHit, SearchKind, SourceFailure, SourceListing, SourcePlan, StatusReconciled,
    StatusesReconciled, TaskAssetNames, TaskContentSet, TaskDetail, TaskDetails, TaskPrioritySet,
    TaskStatusSet, TaskUpdated, TemplateAnswers,
};

/// The bundle's own version, bumped whenever any root's schema changes — added, removed,
/// renamed, **or altered inside**.
///
/// Consumers generate code from this document, so the version is part of the
/// contract rather than a convenience: an SDK can refuse a bundle it was not
/// generated against instead of silently emitting the wrong models. A property added to an
/// existing root is a new field in both SDKs' generated models exactly as a new root is a
/// new model, which is why the reach is the whole document rather than the set of names.
///
/// What each version brought is what `git log` answers; what this number owes a reader is
/// that it moves whenever [`schema_bundle`] below emits a different document. The golden
/// that holds it to that is `PUBLISHED_BUNDLES` in `tests/engine.rs`, which records every
/// root's schema by digest from this version on.
pub const SCHEMA_BUNDLE_VERSION: u32 = 64;

/// The same typed envelope the command line serializes, keeping the payload's root name.
pub(crate) fn write_answer_schema<T: schemars::JsonSchema>() -> Schema {
    let payload = schema_for!(T);
    let mut answer = schema_for!(crate::boundary::WriteAnswer<T>);
    for key in ["title", "description"] {
        if let Some(value) = payload.get(key) {
            answer.insert(key.to_owned(), value.clone());
        }
    }
    answer
}

/// Every contract root, keyed by name, plus each registered plugin's config schema.
#[must_use]
pub fn schema_bundle() -> Value {
    let mut roots: BTreeMap<&'static str, Schema> = BTreeMap::new();

    roots.insert("Task", schema_for!(Task));
    // llmlint: ignore[code_lands_in_the_domain_that_owns_it] The repository deliberately centralizes every schema root beside its append-only version golden. This single line registers the projection domain's own list; its declarations and schema derivation remain in that domain.
    roots.extend(crate::task_projection::schemas());
    roots.insert("Project", schema_for!(Project));
    roots.insert("Document", schema_for!(Document));
    // A root of its own although both `Task` and `Project` reach it inside their own
    // definitions, for the reason `TextFields` is one: a consumer acts on a location by
    // asking which of the two keys is present, so the shape it switches on has to be
    // nameable rather than only reachable.
    roots.insert("Location", schema_for!(Location));
    roots.insert("Label", schema_for!(Label));
    roots.insert("Status", schema_for!(Status));
    roots.insert("StatusCategory", schema_for!(StatusCategory));
    // A root of its own although both network plugins' configurations reach it, for the
    // reason `StatusCategory` is one: it is one grammar every source that names its statuses
    // is configured with, and a caller writing a configuration models it by name rather than
    // by whichever plugin's schema it happens to read it out of.
    roots.insert("StatusMapping", schema_for!(StatusMapping));
    // A root of its own although `Task` reaches it, for the reason `StatusCategory` is one:
    // `task priority set` and `task list --priority` take one by name.
    roots.insert("Priority", schema_for!(Priority));
    roots.insert("SourceName", schema_for!(SourceName));
    // A root of its own although every item reaches it, for the reason `SourceName` is one:
    // `project create --id` takes one by name, a source's own id for the project it writes.
    roots.insert("NativeId", schema_for!(NativeId));
    roots.insert("DependencyEdge", schema_for!(DependencyEdge));
    roots.insert("DependencyEndpoint", schema_for!(DependencyEndpoint));
    roots.insert("QualifiedEndpoint", schema_for!(QualifiedEndpoint));
    roots.insert("ItemKind", schema_for!(ItemKind));
    roots.insert("Repository", schema_for!(Repository));
    roots.insert("DependencyKind", schema_for!(DependencyKind));
    roots.insert("Direction", schema_for!(Direction));
    // Roots of their own although both are reachable inside `TaskQuery`'s definitions,
    // which is enough for a generator and not enough for a reconciliation: the command
    // line spells both deliberately differently (`both` for `title-or-content`, `task`
    // for `tasks`), so a variant added to either would leave the command line quietly
    // unable to name it. A root apiece gives that gate one document to read.
    roots.insert("TextFields", schema_for!(TextFields));
    roots.insert("SearchKind", schema_for!(SearchKind));

    roots.insert("TaskQuery", schema_for!(TaskQuery));
    roots.insert("ProjectQuery", schema_for!(ProjectQuery));
    roots.insert("DocumentQuery", schema_for!(DocumentQuery));
    roots.insert("PageRequest", schema_for!(PageRequest));
    roots.insert("PageOfTask", schema_for!(Page<Task>));
    roots.insert("PageOfProject", schema_for!(Page<Project>));
    roots.insert("PageOfDocument", schema_for!(Page<Document>));
    roots.insert("PageOfLabel", schema_for!(Page<Label>));
    roots.insert("PageOfDependencyEdge", schema_for!(Page<DependencyEdge>));

    roots.insert("Capabilities", schema_for!(Capabilities));
    roots.insert("Health", schema_for!(Health));
    roots.insert("SourceError", schema_for!(SourceError));

    roots.insert("GlobalId", schema_for!(GlobalId));
    roots.insert("PageToken", schema_for!(PageToken));
    roots.insert("QueryPlan", schema_for!(QueryPlan));
    roots.insert("SourcePlan", schema_for!(SourcePlan));
    roots.insert("Predicate", schema_for!(Predicate));
    roots.insert("SourceFailure", schema_for!(SourceFailure));
    // What a command writes to standard output when it exits `1` under machine output,
    // and the two types inside it a caller branches on by name.
    roots.insert("FailureDocument", write_answer_schema::<FailureDocument>());
    roots.insert("Failure", schema_for!(Failure));
    roots.insert("FailureClass", schema_for!(FailureClass));
    roots.insert("QualifiedTask", schema_for!(Qualified<Task>));
    roots.insert("QualifiedProject", schema_for!(Qualified<Project>));
    roots.insert("QualifiedDocument", schema_for!(Qualified<Document>));
    roots.insert("QualifiedLabel", schema_for!(Qualified<Label>));
    roots.insert("QualifiedEdge", schema_for!(QualifiedEdge));
    roots.insert("SearchHit", schema_for!(SearchHit));
    roots.insert("SourceListing", schema_for!(SourceListing));
    roots.insert("SourceListings", schema_for!(Vec<SourceListing>));
    roots.insert(
        "QueryResponseOfQualifiedTask",
        schema_for!(QueryResponse<Qualified<Task>>),
    );
    roots.insert(
        "QueryResponseOfQualifiedProject",
        write_answer_schema::<QueryResponse<Qualified<Project>>>(),
    );
    roots.insert(
        "QueryResponseOfQualifiedDocument",
        schema_for!(QueryResponse<Qualified<Document>>),
    );
    roots.insert(
        "QueryResponseOfQualifiedLabel",
        schema_for!(QueryResponse<Qualified<Label>>),
    );
    roots.insert(
        "QueryResponseOfQualifiedEdge",
        schema_for!(QueryResponse<QualifiedEdge>),
    );
    roots.insert(
        "QueryResponseOfSearchHit",
        schema_for!(QueryResponse<SearchHit>),
    );

    // The comment roots: the contract's own `Comment` and `NewComment`, which cross the
    // plugin protocol, and the three shapes the comment verbs and `task show` answer with.
    roots.insert("Comment", schema_for!(Comment));
    roots.insert("NewComment", schema_for!(NewComment));
    roots.insert("PageOfComment", schema_for!(Page<Comment>));
    roots.insert("CommentList", schema_for!(CommentList));
    roots.insert("DeletedComment", write_answer_schema::<DeletedComment>());
    roots.insert("TaskDetail", write_answer_schema::<TaskDetail>());
    // What `task show-many` answers with: one `TaskDetail` per id, in request order.
    roots.insert("TaskDetails", schema_for!(TaskDetails));
    // What `task comments --since` answers with: every comment made or edited since an instant,
    // each with who wrote it and its version, and the watermark a later read starts from.
    roots.insert("CommentsSince", schema_for!(CommentsSince));
    // One comment of that answer, named so a caller deduplicating them holds it by name.
    roots.insert("CommentActivity", schema_for!(crate::CommentActivity));
    roots.insert(
        "AuthorKind",
        schema_for!(onetaskgraph_plugin_api::AuthorKind),
    );
    // What `project graph --format json` answers with: one project's tasks and the edges
    // between them, laid out as its Mermaid form draws them.
    roots.insert("ProjectGraph", schema_for!(ProjectGraph));

    // What `document show` answers with: the document and the image assets it holds.
    roots.insert("DocumentDetail", write_answer_schema::<DocumentDetail>());
    // The image assets a task or a document holds, as `task show` and `document show` list
    // them, and what crosses the stdio plugin protocol beside a write that carries them:
    // `AssetWrite` is the `assets` and `recorded_assets` members of a `write_task` or a
    // `write_document`, `AssetsWritten` what a plugin answers it with, and `AssetUploads` the
    // `onetaskgraph.assets` record a hosted plugin keeps of what it uploaded.
    roots.insert("Asset", schema_for!(Asset));
    roots.insert("AssetName", schema_for!(AssetName));
    roots.insert("AssetContentType", schema_for!(AssetContentType));
    roots.insert("AssetPayload", schema_for!(AssetPayload));
    roots.insert("AssetUpload", schema_for!(AssetUpload));
    roots.insert("AssetUploads", schema_for!(AssetUploads));
    roots.insert("AssetWrite", schema_for!(AssetWrite));
    roots.insert("AssetsWritten", schema_for!(AssetsWritten));

    // What `task status set` answers with, and the per-task entries it and a copy report for
    // the delivered tasks they kept in step — plus the entry type both of a task's lists hold.
    roots.insert("TaskRef", schema_for!(TaskRef));
    roots.insert("TaskStatusSet", write_answer_schema::<TaskStatusSet>());
    // What `project status set` answers with.
    roots.insert(
        "ProjectStatusSet",
        write_answer_schema::<ProjectStatusSet>(),
    );
    roots.insert("Delivered", schema_for!(Delivered));
    roots.insert("DeliveryOutcome", schema_for!(DeliveryOutcome));

    // What `task status reconcile` and `task status complete` answer with, and the action
    // vocabularies inside each, which a caller branches on by name — with the issue reference
    // both the command line and a landing event spell, and the two answers that cross the
    // plugin protocol beneath them.
    roots.insert(
        "StatusesReconciled",
        write_answer_schema::<StatusesReconciled>(),
    );
    roots.insert("StatusReconciled", schema_for!(StatusReconciled));
    roots.insert("ReconcileOutcome", schema_for!(ReconcileOutcome));
    roots.insert("IssuesCompleted", write_answer_schema::<IssuesCompleted>());
    roots.insert("IssueCompleted", schema_for!(IssueCompleted));
    roots.insert("CompleteAction", schema_for!(CompleteAction));
    roots.insert("IssueRef", schema_for!(IssueRef));
    roots.insert("IssueHolding", schema_for!(IssueHolding));
    roots.insert("HeldTask", schema_for!(HeldTask));
    roots.insert("ClosedAs", schema_for!(ClosedAs));
    roots.insert("Completion", schema_for!(Completion));
    roots.insert("StatusContradiction", schema_for!(StatusContradiction));
    roots.insert("ContradictedItem", schema_for!(ContradictedItem));

    // What `task priority set` and `task content set` answer with.
    roots.insert("TaskPrioritySet", write_answer_schema::<TaskPrioritySet>());
    roots.insert(
        "TaskAssigneeSet",
        write_answer_schema::<crate::TaskAssigneeSet>(),
    );
    roots.insert("TaskContentSet", write_answer_schema::<TaskContentSet>());

    // The targeted update: what a caller names, what `task update` answers with and the
    // field vocabulary both report in, and the outcome a source answers it with across the
    // plugin protocol.
    roots.insert("TaskUpdate", schema_for!(TaskUpdate));
    roots.insert("TaskUpdated", write_answer_schema::<TaskUpdated>());
    roots.insert("UpdatedField", schema_for!(UpdatedField));
    roots.insert("TaskUpdateOutcome", schema_for!(TaskUpdateOutcome));

    // llmlint: ignore[code_lands_in_the_domain_that_owns_it] As the projection domain's line above: every root is registered here beside the append-only version golden, and this one line takes the batched calls' own list, which that domain declares.
    roots.extend(crate::engine::batch::schemas());
    // llmlint: ignore[code_lands_in_the_domain_that_owns_it] As the batched calls' line above: every root is registered here beside the append-only version golden, and this one line takes the relation calls' own list, which that domain declares.
    roots.extend(crate::engine::relations::schemas());

    // What `task`, `project` and `document metadata set` answer with.
    roots.insert("MetadataSet", write_answer_schema::<MetadataSet>());

    // Content-keyed attestations: what `attest` answers with, what `attestation check`
    // answers for one item and — under `project attestation check --tasks` — for every task of
    // a project, the record an item holds, and the two vocabularies a caller branches on.
    // llmlint: ignore-block[code_lands_in_the_domain_that_owns_it] The shapes are declared once, in `crate::attestation`; what lives here is their registration as roots, which is what both SDK generators read and `SCHEMA_BUNDLE_VERSION` and its golden track.
    roots.insert("Attested", write_answer_schema::<Attested>());
    roots.insert("Attestation", schema_for!(Attestation));
    roots.insert("AttestationCheck", schema_for!(AttestationCheck));
    roots.insert(
        "ProjectAttestationCheck",
        schema_for!(ProjectAttestationCheck),
    );
    roots.insert("AttestationState", schema_for!(AttestationState));
    roots.insert("VoidReason", schema_for!(VoidReason));
    // llmlint: ignore-end[code_lands_in_the_domain_that_owns_it]

    roots.insert("CopyReport", write_answer_schema::<CopyReport>());
    // A root of its own, for the reason `StatusMapping` is one: every source's block takes the
    // two settings of a create safe to retry by name, whatever plugin it names.
    roots.insert("CreateSettings", schema_for!(crate::config::CreateSettings));
    roots.insert("CopyOutcome", schema_for!(CopyOutcome));
    roots.insert("CopyAction", schema_for!(CopyAction));
    // Roots of their own although `CopyOutcome` reaches both, for the reason `CopyAction` is
    // one: a caller branches on which rule found an item and on what became of its link.
    roots.insert("CopyVia", schema_for!(CopyVia));
    roots.insert("CopyLink", schema_for!(CopyLink));
    // What `task locate` answers: where a copy would land each task, what it would update and
    // where a create would file its issue.
    roots.insert("LocateReport", schema_for!(LocateReport));
    // One task a locate placed, named so a caller holding one has it under its own name.
    roots.insert("Located", schema_for!(crate::Located));

    // What `template variables` and `template render` answer with, and the declaration and
    // the two vocabularies inside the first, which a caller branches on by name.
    // llmlint: ignore-block[code_lands_in_the_domain_that_owns_it] This table is the one
    // document both SDKs are generated from, and every verb's answer is a root of it by
    // design — `SCHEMA_BUNDLE_VERSION` and `PUBLISHED_BUNDLES` hold it whole. A template root
    // registered anywhere else would be one no SDK is generated against.
    roots.insert("TemplateVariables", schema_for!(TemplateVariables));
    roots.insert("TemplateVariable", schema_for!(TemplateVariable));
    roots.insert("VariableType", schema_for!(VariableType));
    roots.insert("ItemType", schema_for!(ItemType));
    roots.insert("RenderedTemplate", schema_for!(RenderedTemplate));
    // One member of a project template's plan graph, as its front matter declares it, and
    // what `project create --dry-run` answers with: the project and every member it would
    // create.
    roots.insert("MemberDeclaration", schema_for!(MemberDeclaration));
    roots.insert("PlannedProject", schema_for!(crate::PlannedProject));
    // What `task render` and `document render` answer with, what `task answers` and
    // `document answers` answer with, and the `onetaskgraph.template` entry an item rendered
    // from a template records — named, because a caller checking a hand edit or a changed
    // template reads its hashes by name.
    roots.insert("Regenerated", write_answer_schema::<Regenerated>());
    roots.insert("TemplateAnswers", schema_for!(TemplateAnswers));
    roots.insert("TemplateProvenance", schema_for!(TemplateProvenance));
    // llmlint: ignore-end[code_lands_in_the_domain_that_owns_it]
    // llmlint: ignore[code_lands_in_the_domain_that_owns_it] Every CLI response registers here for both SDK generators; the type lives in engine/assets.rs, and the versioned golden tracks this registration beside the other roots.
    roots.insert("TaskAssetNames", schema_for!(TaskAssetNames));

    roots.insert("EffectiveConfig", schema_for!(EffectiveConfig));
    // What `sources route` answers with, and the placement a routed copy's report names for
    // each item it landed.
    roots.insert("SourceRoute", schema_for!(SourceRoute));
    roots.extend(crate::engine::sweep_schemas());
    roots.extend(crate::boundary::schemas());
    roots.insert("Placement", schema_for!(Placement));
    roots.insert("Setting", schema_for!(Setting));
    roots.insert("Origin", schema_for!(Origin));
    roots.insert("OutputFormat", schema_for!(OutputFormat));
    roots.insert("SecretsReport", schema_for!(SecretsReport));
    roots.insert("ResolvedCredential", schema_for!(ResolvedCredential));
    roots.insert("CredentialLayer", schema_for!(CredentialLayer));
    // The public boundary's shapes: who may read an item, who a source is declared readable
    // by, the commands a store asks, and what those commands exchange — named, because a
    // caller writing a configuration or implementing a policy models each of them by name.
    roots.insert("Classification", schema_for!(Classification));
    roots.insert("SourceVisibility", schema_for!(SourceVisibility));
    roots.insert("WritePolicyConfig", schema_for!(WritePolicyConfig));
    roots.insert("RepositoryVisibility", schema_for!(RepositoryVisibility));
    roots.insert("PublicWriteInput", schema_for!(PublicWriteInput));
    // The map a private destination project records from each canonical task to its public
    // projection: the copy domain's one definition of it, a root so both SDKs model it by
    // name and their serialization tests round-trip what the binary records.
    roots.insert("PublicProjections", schema_for!(PublicProjections));
    roots.insert("WriteVerdict", schema_for!(WriteVerdict));

    // The local cache of a remote source: how a source is configured with one, what `cache
    // clear` answers with, how a read used one, and what a write made with `--expect` came
    // to — named, because a caller pinning a read or expecting a write branches on each.
    roots.insert("SourceCacheConfig", schema_for!(SourceCacheConfig));
    roots.insert("CacheCleared", schema_for!(CacheCleared));
    roots.insert("CacheUse", schema_for!(CacheUse));
    roots.insert("ExpectedWrite", schema_for!(ExpectedWrite));
    roots.insert("CommentWritten", write_answer_schema::<CommentWritten>());

    // A `linear` source's configuration, as a root of its own as well as under
    // `plugin_config`: both SDKs are generated from the roots, and a caller writing a
    // configuration for the source this host's Linear work lands in — its `status_mapping`
    // and its `project` — needs the shape named rather than reachable only as a plugin's.
    // llmlint: ignore-block[code_lands_in_the_domain_that_owns_it] The schema is the plugin's own `LinearConfig`, not restated here; what lives here is its registration as a root, and the roots are what both SDK generators read and what `SCHEMA_BUNDLE_VERSION` and its golden track — a root registered anywhere else is one no SDK is generated against and no version moves for. The engine already depends on and names every plugin by feature (`registry.rs`), and a contribution mechanism on `SourcePlugin` would change the api crate AGENTS.md asks be kept still.
    #[cfg(feature = "linear")]
    roots.insert(
        "LinearConfig",
        schema_for!(onetaskgraph_linear::LinearConfig),
    );
    // llmlint: ignore-end[code_lands_in_the_domain_that_owns_it]

    let plugins: BTreeMap<String, Schema> = registry()
        .iter()
        .map(|plugin| (plugin.kind().to_owned(), plugin.config_schema()))
        .collect();

    json!({
        "version": SCHEMA_BUNDLE_VERSION,
        "roots": roots,
        "plugin_config": plugins,
    })
}
