//! The copy verb: one item out of one source and into another, by the rules that make a
//! second copy an update rather than a duplicate.
//!
//! Correspondence lives on the items and never in a table. A copied item carries
//! [`GlobalId::ORIGIN_KEY`], whose value is the qualified id it was copied from, and the
//! item it was copied from carries [`MetadataKey::COPIES_KEY`], an object naming its
//! counterpart at each destination it was copied into. The rules below read exactly those —
//! so nothing here is written down outside the plugin that owns the item, and the invariant
//! this engine is built around is untouched.
//!
//! An explicit `target_task` is verified by destination and origin before these rules.
//! It is updated and its link replaces any prior destination entry.
//!
//! 0. **Follow the link.** An item whose `onetaskgraph.copies` names an item at the
//!    destination has that item read by id, and when that item's own origin names this one
//!    back it is the target: one read, and no scan. A link naming nothing there any more is
//!    refused as `stale-link` unless `--recreate` is given, for the reason a stale origin
//!    is; a link naming an item that no longer names this one back — somebody re-pointed it
//!    — is ignored, and the rules below decide.
//! 1. **Follow the origin.** An item already carrying an origin whose source half is the
//!    destination names the destination item *directly*, and the copy updates it. This is
//!    the half that makes an edit's copy-back an update: the local file came from the
//!    remote item and knows which one.
//! 2. **Search by origin.** Otherwise the destination is scanned, one page at a time, for
//!    an item whose origin is the id being copied. Found, the copy updates it; not found,
//!    the copy creates one carrying that origin.
//!
//! Which rule found the item decides what the copy records there. A copy that got its
//! target from rule 1 is a copy-back — the destination is the *original*, and the item
//! being copied is the one that came from it — so the destination keeps the origin it
//! already holds, holding none included. Every other copy records the id it was copied
//! from. See [`recorded`] for what stamping a copy-back's own id there costs.
//!
//! The same distinction decides the link. Once the whole copy has landed, every item whose
//! target was found by any rule but rule 1 — or was created — has its link for this
//! destination recorded or refreshed through its source's narrow metadata write, and that
//! write joins the undo journal like every destination write. Rule 1 records nothing: that
//! correspondence is already written down, on the destination item.
//!
//! A destination write is at the user's explicit request, names its destination, goes
//! through that source's own write interface into that source's own store, and is never
//! read back to answer a query. That is what makes it a write and not a cache.

mod comments;
pub(crate) mod intent;
mod locate;
mod privacy;
mod projections;
mod query;
mod record;
mod sections;
mod snapshot;
use crate::section_links::{LinearSectionForm, SectionForm};
pub use locate::{
    CreatesIn, LocateLookup, LocateReport, LocateRequest, Located, LocatedCounterpart, NotATask,
};
use onetaskgraph_plugin_api::StoredHeading;
pub use projections::{MirrorTarget, PublicProjection, PublicProjections};
pub(super) use snapshot::single as single_detail;

use query::{Slot, failed};

use std::collections::{BTreeMap, BTreeSet, HashMap};

use onetaskgraph_plugin_api::{
    AssetPayload, AssetUploads, AssetWrite, BodySize, Capabilities, Classification, Comment,
    CommentCopy, Cursor, DependencyEdge, DependencyEndpoint, DependencyKind, Direction, Document,
    DocumentQuery, ItemKind, ItemWrite, Location, MetadataKey, MetadataMatch, MetadataRecord,
    Metering, NativeId, Page, PageRequest, Priority, Project, ProjectFilter, ProjectQuery,
    Repository, SourceError, SourceName, Status, StatusCategory, Task, TaskDetailRead, TaskQuery,
    TaskRef, TextFields, TextQuery, WriteTarget,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::cache::{ExpectedWrite, WriteResult};
use crate::config::Placement;
use crate::resolve::ResolvedSource;
use crate::template::TemplateProvenance;
use crate::{Failure, GlobalId};

use super::assets;
use super::boundary::{Exposure, Outbound, Rewrite};
use super::delivery::{Delivered, targets};
use super::dependents::{self, Dependents};
use super::fetch::{fits, unrepeated};
use super::local::ProjectSelector;
use super::narrow::holds_priority;
use super::stored;
use super::{
    DocumentFilters, DocumentRequest, Engine, EngineError, Filters, LeftBehind, Paging, Qualified,
    TaskRequest,
};

/// Which status a copy writes when its destination already holds the item.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum StatusPolicy {
    /// Carry the source status, including on updates.
    #[default]
    Source,
    /// Keep an existing destination item's status; creates carry the source status.
    PreserveDestination,
}

/// What a status-guarded copy does with an item whose destination status is not one of the
/// categories it names — `--otherwise`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Otherwise {
    /// Write nothing for that item and report it `failed`, as `destination-status`.
    #[default]
    Refuse,
    /// Copy that item as [`StatusPolicy::PreserveDestination`] copies it.
    PreserveDestination,
}

/// The conditions a task copy puts on the counterpart it updates, decided on the copy's own
/// read of that counterpart before anything is written — `--when-destination-status`,
/// `--otherwise` and `--when-destination-priority`.
///
/// Empty, the default, is no condition: the copy writes exactly what it did before these
/// existed. A create is never conditioned, because there is no destination item to hold a
/// value; neither is a project or a document, which a task copy does not update.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CopyConditions {
    /// The categories a counterpart's status has to be at for the copy to write its status as
    /// it would without the condition. Empty sets no condition.
    pub status: Vec<StatusCategory>,
    /// What happens to a counterpart at another category; `None` is [`Otherwise::Refuse`].
    /// Refused, as [`EngineError::OtherwiseWithoutStatus`], beside an empty `status`.
    // llmlint: ignore[invalid_states_unrepresentable] The contract this lands names `--otherwise` as a flag of its own, taken only beside `--when-destination-status` and refused before any request without it; both SDKs map one keyword to each flag, so this request mirrors them field for field as `create` beside `match_by` does. The one combination that means nothing, `otherwise` with no category, is refused by `Engine::copy` as `EngineError::OtherwiseWithoutStatus` before anything is read, and `a_status_guard_beside_preserve_destination_and_otherwise_alone_are_refused_before_any_request` drives it.
    pub otherwise: Option<Otherwise>,
    /// The priorities a counterpart's priority has to be at for the copy to write the source's
    /// priority over it; at any other the counterpart keeps its own, and every other field is
    /// written as it would be. Empty sets no condition.
    pub priority: Vec<Priority>,
    /// The people a counterpart must be assigned to — exactly these, whatever their order,
    /// `[]` meaning unassigned — for the copy to write the source's assignee over it; holding
    /// any other set it keeps its own, and every other field is written as it would be.
    /// `None` sets no condition, and a copy then never writes an assignee over an existing
    /// item. Only for a destination that opted in to assignees.
    // llmlint: ignore[invalid_states_unrepresentable] An empty set is a condition of its own here — met only by an unassigned counterpart — so "no condition" needs its own state, which `None` is.
    pub assignees: Option<Vec<String>>,
    /// The version each named task's counterpart must be at for the copy to write it, keyed by
    /// the qualified id the task is copied from — `task copy --if-version <ID>=<VERSION>`.
    /// A counterpart at any other version is written nothing and answered `failed` as
    /// `version-moved`, its `held.version` the version it is at, while every other item is
    /// copied; a task this copy creates holds nothing, since there is no version for it to be
    /// at. Empty holds nothing.
    pub versions: HashMap<GlobalId, String>,
}

/// What a guarded copy found a counterpart's status at, and whether that met the condition.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct StatusCondition {
    /// The counterpart's status category, as the copy's own pre-write read held it.
    pub held: StatusCategory,
    /// Whether that category is one the condition names.
    pub met: bool,
}

/// What a guarded copy found a counterpart's priority at, and whether that met the condition.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PriorityCondition {
    /// The counterpart's priority, as the copy's own pre-write read held it.
    pub held: Priority,
    /// Whether that priority is one the condition names.
    pub met: bool,
}

/// A request to copy work into one configured destination.
#[derive(Debug, Clone)]
pub struct CopyRequest {
    /// The qualified items to copy, in the order they were named.
    pub items: CopyItems,
    /// What those ids name, and what comes with them.
    pub scope: CopyScope,
    /// How an existing destination item's status is treated.
    pub status_policy: StatusPolicy,
    /// The configured source to copy into — a source name, never a qualified id.
    pub destination: SourceName,
    /// How to re-establish a correspondence the two origin rules cannot find.
    pub match_by: Option<MatchBy>,
    /// The existing destination task to verify and update before following stored links.
    // llmlint: ignore[invalid_states_unrepresentable] The public CopyRequest contract requires an optional target_task beside the existing flag-for-flag fields; replacing them with a selection enum would depart from that agreed interface. Engine::copy rejects conflicting modes, queries, multiple ids and non-task scopes before reading, and the CLI and Rust copy_link journeys prove those refusals.
    pub target_task: Option<GlobalId>,
    /// Whether an origin naming nothing at the destination falls through to the search
    /// rule instead of refusing.
    pub recreate: bool,
    /// Whether the caller asserts the destination holds no counterpart of any item named,
    /// so each is created there without the correspondence lookup — no origin search of the
    /// destination at all.
    ///
    /// Sound for a caller that has just asked the destination itself: the origin query is
    /// an index that lags a write, so repeating it here would be behind by exactly as much as
    /// the caller's own was and could find nothing the caller's did not. Refused beside
    /// [`match_by`](Self::match_by) and [`recreate`](Self::recreate), which are ways of
    /// looking, and for an item that itself records a counterpart at the destination — a
    /// link or an origin naming it — which is a carrier the caller's assertion overlooked.
    // llmlint: ignore-block[invalid_states_unrepresentable] One enum of the ways a copy finds its target would make `create` beside `match_by` or `recreate` unrepresentable, but only by replacing `match_by` and `recreate`, which every Rust caller of this request sets by name and the command line maps flag for flag — rewriting what each existing caller writes. `create` is one more field beside them, as `recreate` and `dry_run` were, and the one combination it makes possible that means nothing is refused by `Engine::copy`, as `EngineError::CreateWith`, before anything is read.
    pub create: bool,
    // llmlint: ignore-end[invalid_states_unrepresentable]
    /// Whether to perform every read and no write.
    pub dry_run: bool,
    /// Whether the copy keeps the destination's copies of each task's comments in step with
    /// the source's — `--comments`.
    ///
    /// Without it a copy writes no comment at either end, and reads, writes and costs exactly
    /// what it did before comments could be carried — but for a copy by query narrowed by
    /// comment activity, which reads each matched task's comments once to confirm it. With it, the copy adds the
    /// source's comments the destination holds no copy of, edits a copy whose origin comment
    /// changed, and deletes a copy whose origin comment is gone; a comment native to the
    /// destination is never edited or deleted, and a source comment that is itself a copy of
    /// one of the destination's own is never sent back. Each item's outcome then reports
    /// [`CopyOutcome::comments`].
    ///
    /// A document holds no comments, so beside [`CopyScope::Documents`] it is refused, as
    /// `EngineError::CommentsNotDocuments`, before anything is read.
    // llmlint: ignore-block[invalid_states_unrepresentable] Making `comments` beside `CopyScope::Documents` unrepresentable would move it into the scope enum's task and project variants, which every Rust caller of this request matches and sets by name, and which `CopyScope` is shared with the command line and the report — rewriting what each existing caller writes for one flag. The one combination it allows that means nothing is refused by `Engine::copy` before anything is read, as `create` beside `match_by` is.
    pub comments: bool,
    // llmlint: ignore-end[invalid_states_unrepresentable]
    /// Whether the copy writes nothing at all to the source — `--no-link`.
    ///
    /// No `onetaskgraph.copies` link is recorded on any item copied, and each outcome reports
    /// [`CopyLink::Skipped`]. The counterpart is found on the next copy by the destination's
    /// own `onetaskgraph.origin`, so leaving the source untouched costs a later copy nothing
    /// but that lookup.
    pub no_link: bool,
    /// The version of the destination task the caller says it knows — `--expect`.
    ///
    /// Taken by a copy of one task bound to one destination task: the write takes that task's
    /// ids and current values from the destination's local cache, at that version or a later
    /// one, instead of reading it, and sends only the fields whose value differs. Its outcome
    /// then reports [`CopyOutcome::expected`]. Refused, before anything is read, beside more
    /// than one item, a query, a project or a document, and onto a destination keeping no
    /// cache.
    pub expect: Option<String>,
    /// The status categories a task copy keeps — `--status`, repeatable. Empty keeps every
    /// task.
    ///
    /// Beside a query it narrows the query exactly as `task list --status` does: a task at
    /// another category is not read for its comments, not copied and not named in the report.
    /// Beside ids, an id whose task is at another category is read only as far as its status,
    /// then reported [`CopyAction::Skipped`], neither copied nor read for its comments. Only a
    /// task copy takes it: beside a project or a document copy it is refused as
    /// [`EngineError::StatusesNotTasks`] before anything is read.
    pub statuses: Vec<StatusCategory>,
    /// The conditions on the counterpart each task updates — see [`CopyConditions`]. Only a
    /// task copy takes them: beside a project or a document copy any condition is refused, as
    /// [`EngineError::ConditionsNotTasks`], before anything is read.
    pub conditions: CopyConditions,
}

/// A copy selection option, named as the command line spells it in a conflict refusal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CopyLookup {
    /// [`CopyRequest::create`], `--create`.
    Create,
    /// [`CopyRequest::match_by`], `--match-by`.
    MatchBy,
    /// [`CopyRequest::recreate`], `--recreate`.
    Recreate,
}

impl std::fmt::Display for CopyLookup {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::Create => "--create",
            Self::MatchBy => "--match-by",
            Self::Recreate => "--recreate",
        })
    }
}

/// The items one copy names: at least one id, or the tasks one query matches.
///
/// A newtype rather than a bare `Vec`, for the reason [`Repository`] is one: the empty
/// list is not a copy of nothing, it is a caller mistake, and a type that can hold it
/// leaves every reader to decide what it means — a report with no entries, an error, a
/// silent success. None of those is better than not being able to say it. A query that
/// matches nothing is a different thing: the caller asked a question, and an empty report is
/// its answer.
#[derive(Debug, Clone, PartialEq)]
pub struct CopyItems(Named);

/// What a [`CopyItems`] holds.
#[derive(Debug, Clone, PartialEq)]
enum Named {
    /// Ids a caller named, at least one.
    Ids(Vec<GlobalId>),
    /// The query whose matches are copied — `task copy --to` with `task list`'s narrowing in
    /// place of ids.
    // llmlint: ignore[invalid_states_unrepresentable] Holding the query inside the task scope's variant would make `CopyScope` carry the items for one of its three variants and `CopyItems` for the other two, so every Rust caller that sets `items` and `scope` by name — and the command line, which maps both flag for flag — would be rewritten for one way of naming tasks. The one combination this allows that means nothing, a query beside a project or document scope, is refused by `Engine::copy` as `EngineError::QueryNotTasks` before anything is read, and `a_rust_caller_asking_what_only_a_task_copy_can_do_of_another_is_refused_first` in `tests/copy.rs` holds it.
    Matching(Box<TaskRequest>),
}

impl CopyItems {
    /// The items a caller named, or `None` when they named none.
    #[must_use]
    pub fn new(items: Vec<GlobalId>) -> Option<Self> {
        (!items.is_empty()).then_some(Self(Named::Ids(items)))
    }

    /// Every task `query` matches, read through every page of it when the copy runs.
    ///
    /// Only a task copy — [`CopyScope::Tasks`] — names its items this way. The page the query
    /// names is ignored: every page is walked, at its limit. Its `commented_since` is not handed
    /// to any source: the copy confirms it from the comments it reads anyway — or, for a task
    /// unchanged since it was last copied with its comments, from the destination's copies of
    /// them — so no task's comments are read twice in one invocation.
    #[must_use]
    pub fn matching(query: TaskRequest) -> Self {
        Self(Named::Matching(Box::new(query)))
    }

    /// The ids named, in the order they were named — none for a query, whose items are known
    /// only once it has run.
    #[must_use]
    pub fn as_slice(&self) -> &[GlobalId] {
        match &self.0 {
            Named::Ids(ids) => ids,
            Named::Matching(_) => &[],
        }
    }

    /// The query whose matches are copied, when these items are one.
    #[must_use]
    pub fn query(&self) -> Option<&TaskRequest> {
        match &self.0 {
            Named::Ids(_) => None,
            Named::Matching(query) => Some(query),
        }
    }
}

/// What the ids a copy names are, and what travels with them.
///
/// One value rather than a kind beside a flag, because three of the four combinations
/// those two would make are real and the fourth — tasks, with the tasks of each also
/// copied — means nothing. The member list is a variant for the same reason: it narrows a
/// project copy that carries its tasks, and it has nothing to say to the other three.
#[derive(Debug, Clone, PartialEq)]
pub enum CopyScope {
    /// The ids name tasks, and only those tasks are copied.
    Tasks,
    /// The ids name projects.
    Projects {
        /// Whether the tasks in each project are copied too.
        tasks: bool,
        /// Whether the documents filed under each project are copied too — `project copy
        /// --documents` — in the same copy: read, screened and sized with the project and its
        /// tasks before anything is written, and landed after them, each filed under the
        /// project's counterpart.
        documents: bool,
    },
    /// The ids name projects, and of the tasks in them exactly these are copied.
    ///
    /// A member the list does not name is not read at the destination, not compared, not
    /// written and not reported, and no walk for what the copy left behind runs. A named
    /// member with no counterpart there is still created: the list narrows which members
    /// are read and written, never which outcomes are possible.
    ///
    /// An edge from a copied item to a member the list does not name resolves to the
    /// destination id that member's own [`GlobalId::ORIGIN_KEY`] records at the source,
    /// without reading the destination for it — or, for a copy into a source with `routes`,
    /// the id its own `onetaskgraph.copies` link records in any source the copy reaches.
    /// When that member records neither, the copy is refused before anything is written.
    Members(CopyItems),
    /// The ids name projects the destination already holds, and of the tasks in them exactly
    /// these are copied into those projects; the projects themselves are left alone.
    ///
    /// Each project's counterpart is resolved from its own records alone — the
    /// `onetaskgraph.copies` link it records for a source the copy reaches, then the origin it
    /// records there, then a lookup by origin — and is never read for comparison, never
    /// planned, never written and never reported. A project with no counterpart is refused
    /// before anything is written, as [`EngineError::NoProjectCounterpart`], because creating
    /// it is what a whole project copy is for.
    ///
    /// Everything else is as [`CopyScope::Members`] has it: a member the list does not name is
    /// neither read nor reported, an edge to one resolves the same way, and no walk for what
    /// the copy left behind runs.
    MembersOnly(CopyItems),
    /// The ids name documents, and only those documents are copied.
    ///
    /// Nothing travels with a document: it takes part in no dependency graph, and it holds
    /// nothing of its own the way a project holds tasks.
    Documents,
}

/// The caller-named escape for a correspondence neither origin rule can find.
///
/// A person editing Markdown who deletes or corrupts the origin key leaves an item rule 1
/// cannot use and rule 2 cannot find, and the next copy would create a second item. This
/// is how that is re-established without hand-editing ids.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MatchBy {
    /// Match the item whose title is the same.
    Title,
    /// Match the item whose value at this metadata key is the same.
    Metadata(String),
}

impl MatchBy {
    /// The spelling a caller types, `title` or any metadata key.
    #[must_use]
    pub fn parse(key: &str) -> Self {
        if key == "title" {
            Self::Title
        } else {
            Self::Metadata(key.to_owned())
        }
    }
}

/// What a copy did, one entry per item.
///
/// The same per-item outcomes reach every consumer: the machine-readable output renders
/// this, the rendered output renders this, and a Rust caller is handed it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct CopyReport {
    /// One entry per item the copy considered, in the order it considered them.
    pub items: Vec<CopyOutcome>,
    // Three flat scalars rather than one nested object, and a `!skip_serializing_if` beside
    // every skip: both are load-bearing for the generated SDKs rather than matters of
    // taste, and AGENTS.md's note on what a copied document's references are pointed at is
    // where that reasoning lives.
    /// Reference occurrences the copy rewrote to the destination's own location for the
    /// record they name.
    ///
    /// A silent bound is indistinguishable from a bug, so the copy says what it did to the
    /// references the documents it carried hold. This and the two below are totals over the
    /// whole invocation rather than figures per document, and all three default to zero, so
    /// a consumer written against the output before they existed is unaffected.
    ///
    /// **What these figures do not claim.** The referent set is a document's own project,
    /// so a reference to a record in a *different* project is never recognised at all and
    /// cannot appear in [`Self::references_unresolved`] either. These are the references
    /// the copy recognised; they are not a census of every reference a document holds.
    /// Noticing an out-of-scope reference would need exactly the unbounded destination walk
    /// this design refuses.
    #[serde(default, skip_serializing_if = "nothing_to_report")]
    #[schemars(!skip_serializing_if)]
    pub references_rewritten: u64,
    /// Reference occurrences the copy recognised and left byte-for-byte as they were,
    /// because the correspondence could not be established.
    #[serde(default, skip_serializing_if = "nothing_to_report")]
    #[schemars(!skip_serializing_if)]
    pub references_unresolved: u64,
    /// How many of [`Self::references_unresolved`] were left alone because the
    /// correspondence was **ambiguous** rather than merely absent. A sub-count, never
    /// larger than it.
    ///
    /// Split out because the two mean different things to a reader. A reference with no
    /// counterpart is ordinary and expected under the bound above — the design working. An
    /// ambiguous one says the destination holds duplicate records for one work item, or the
    /// source reports one location for two records, and re-running the copy will never
    /// clear it.
    #[serde(default, skip_serializing_if = "nothing_to_report")]
    #[schemars(!skip_serializing_if)]
    // llmlint: ignore[invalid_states_unrepresentable] JSON Schema cannot express an
    // inequality between two numbers, so a private constructor here would hold this in one
    // consumer of three while both SDKs' generated models went on admitting it. What holds
    // it is `substitute`: `Resolution` has no variant that counts an occurrence ambiguous
    // without counting it unresolved.
    pub references_ambiguous: u64,
    /// `delivers` entries the copy rewrote to the destination's own id for a member of the
    /// copied set, over the whole invocation. Every other entry arrives qualified and is not
    /// counted. Left out when zero, as the three figures above are.
    #[serde(default, skip_serializing_if = "nothing_to_report")]
    #[schemars(!skip_serializing_if)]
    pub delivers_rewritten: u64,
    /// One entry per delivered task the copy kept in step with a task it landed, after the
    /// whole copy was complete — see `task status set`, which reports the same entries. Left
    /// out when there were none.
    ///
    /// A failed entry does not undo the copy: the tasks it landed stay landed, and the
    /// command exits `4`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[schemars(!skip_serializing_if)]
    pub delivered: Vec<Delivered>,
    /// What this copy spent, summed over the sources in the command that meter their own
    /// requests — and absent, never zero, when none of them does.
    ///
    /// Omitted from the wire when absent, like the three figures above, so a consumer
    /// written against the output before it existed reads the same document.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spent: Option<Spent>,
}

/// Whether one of [`CopyReport`]'s reference figures has anything to say.
///
/// A copy that recognised no reference reports that by leaving the figure out rather than
/// by writing a nought, so the machine output of a task or project copy is exactly what it
/// was before these figures existed. The human rendering says it in words either way,
/// because a reader there needs to be told the copy looked.
fn nothing_to_report(figure: &u64) -> bool {
    *figure == 0
}

/// What a copy did to one record's section links — links to one heading of a record — that it
/// could not write in the destination's form, and what finishing them cost: README "Section
/// links". Each figure is left out when zero, so an item whose links all landed, or that holds
/// none, reads exactly as it did before these figures existed.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SectionLinks {
    /// Section links whose fragment names no heading of the record they point at, carried
    /// byte for byte rather than guessed at.
    #[serde(default, skip_serializing_if = "nothing_to_report")]
    #[schemars(!skip_serializing_if)]
    pub section_links_unresolved: u64,
    /// Section links to a heading the destination's native form cannot address — a Linear
    /// record with no stored id for that heading. Cross-record links fall back to the
    /// record's URL alone; same-record links retain their authored fragment.
    #[serde(default, skip_serializing_if = "nothing_to_report")]
    #[schemars(!skip_serializing_if)]
    pub section_links_unaddressable: u64,
    /// Writes the copy added to this record to finish its section links, once the records they
    /// name — or the record itself — had landed and their headings could be addressed.
    #[serde(default, skip_serializing_if = "nothing_to_report")]
    #[schemars(!skip_serializing_if)]
    pub section_link_updates: u64,
}

/// Record `source`'s body figures as its final rendering counts them, adding `updates` to the
/// finishing writes already counted for it — its comments' edits among them, which the
/// rendering's figures do not replace.
fn settle(
    journal: &mut Journal,
    source: &GlobalId,
    figures: sections::SectionCounts,
    updates: u64,
) {
    let held = journal.sections.entry(source.clone()).or_default();
    *held = SectionLinks::of(figures, held.section_link_updates + updates);
}

impl SectionLinks {
    /// The figures of one rendering, and the updates already counted.
    fn of(counts: sections::SectionCounts, updates: u64) -> Self {
        Self {
            section_links_unresolved: counts.unresolved,
            section_links_unaddressable: counts.unaddressable,
            section_link_updates: updates,
        }
    }
}

/// What one command spent, summed over the sources in it that meter their own requests.
///
/// **Source-owned.** Every figure is what a source said it sent and spent while the command
/// ran, read through [`TaskSource::metering`](onetaskgraph_plugin_api::TaskSource::metering)
/// before the command and again after it. The engine adds the differences up by name and
/// interprets none of them, which is why the budget and unit names are open vocabulary.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Spent {
    /// How many HTTP requests those sources sent for this command.
    pub requests: u64,
    /// What those requests spent, one entry per budget and unit, ordered by budget name.
    pub budgets: Vec<BudgetSpent>,
}

/// What one command spent against one budget.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct BudgetSpent {
    /// The budget, as the source names it — `graphql`, `rest`.
    // llmlint: ignore[invalid_states_unrepresentable] The contract this report lands states `budget` and `unit` as strings in an open vocabulary the engine interprets none of, and both SDKs are generated from this type's schema; the one value such a vocabulary can refuse, the empty name, never reaches here, because `difference` below treats a source reporting one as not metering.
    pub budget: String,
    /// What it is metered in — `points`, `requests`.
    // llmlint: ignore[invalid_states_unrepresentable] As `budget` above.
    pub unit: String,
    /// How much was spent against it, in that unit.
    pub amount: u64,
    /// Whether any part of `amount` was modelled by a source rather than reported by its
    /// backend or counted, which makes `amount` a lower bound on what the backend charged
    /// rather than a measurement of it.
    pub lower_bound: bool,
}

/// One document's reference figures, before they are folded into the invocation's.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct Counted {
    /// Occurrences rewritten.
    rewritten: u64,
    /// Occurrences recognised and left alone.
    unresolved: u64,
    /// How many of those were ambiguous.
    ambiguous: u64,
}

impl Counted {
    /// Fold one document's figures into the invocation's.
    fn add(&mut self, other: Self) {
        self.rewritten += other.rewritten;
        self.unresolved += other.unresolved;
        self.ambiguous += other.ambiguous;
    }
}

/// What happened to one item.
///
/// `action` and `destination` are one value rather than two fields side by side: an
/// updated item without a destination id, or an orphan without one, are states this type
/// must not be able to say — the id *is* what those outcomes are about. The one outcome
/// that legitimately has none is a dry run that would create, because nothing was
/// created and there is no id to report.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct CopyOutcome {
    /// The qualified id the item was read from.
    pub source: GlobalId,
    /// What happened to it, and where.
    #[serde(flatten)]
    pub action: CopyAction,
    /// Which source a routed copy placed it in, and the index of the route entry that
    /// matched — `null` there when none did.
    ///
    /// Present for every item a copy into a source declaring `routes` landed, a dry run's
    /// included, so a would-be create still says where it would go; absent for a copy into
    /// a source with none, whose output is what it always was, and for an orphan, which was
    /// not placed at all.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub placed: Option<Placement>,
    /// What the write of an item copied with `--expect` came to: its `result`, its
    /// `fallback` and what it `spent` — absent for every other item.
    #[serde(flatten)]
    pub expected: ExpectedWrite,
    /// What `--when-destination-status` found the counterpart's status at, and whether that
    /// met it — present only when the copy was given the condition and this item was an update
    /// of a task, a refused one included.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status_condition: Option<StatusCondition>,
    /// What `--when-destination-priority` found the counterpart's priority at, and whether
    /// that met it — present only when the copy was given the condition and this item was an
    /// update of a task.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub priority_condition: Option<PriorityCondition>,
    /// What `--when-destination-assignees` found the counterpart's assignees at, and whether
    /// that met it — present only when the copy was given the condition and this item was an
    /// update of a task.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assignee_condition: Option<super::AssigneeCondition>,
    /// What the counterpart held, as the copy's read of it before writing found it — present
    /// only on an item the copy refused after that read, such as `destination-status`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub held: Option<WriteResult>,
    /// What the copy could not do for its section links, and what finishing them cost.
    #[serde(flatten)]
    pub sections: SectionLinks,
    /// The size of the body the destination would store for the item, in the unit its plugin
    /// states its limit in — the metadata slot the destination keeps and every asset reference
    /// rewritten to where it is served included. Present on a dry run's item whose destination
    /// can say, and absent on every other outcome.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body_size: Option<u64>,
}

/// One item a copy would write whose body is larger than its destination holds.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Oversized {
    /// The qualified id the item was read from.
    pub source: GlobalId,
    /// The configured source it would have been written to.
    pub destination: SourceName,
    /// The size of the body that write would store, as the destination's plugin counts it.
    pub body_size: u64,
    /// The most that destination holds in one body.
    pub limit: u64,
}

/// What a `--comments` copy did to one task's comments at the destination.
///
/// Every figure counts comments of the *source*: one added is a source comment the
/// destination held no copy of, one edited is a copy brought back in step with its origin
/// comment, one deleted is a copy whose origin comment the source no longer holds, and one
/// unchanged is a copy that already read as its origin. A comment native to the destination
/// is never counted, because a copy never touches it, and neither is a source comment that is
/// a copy of one of the destination's own, because it is never sent back.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CommentCounts {
    /// Copies the copy created.
    pub added: u64,
    /// Copies the copy edited.
    pub edited: u64,
    /// Copies the copy deleted.
    pub deleted: u64,
    /// Copies that already read as their origin comments.
    pub unchanged: u64,
}

/// Which rule found the destination counterpart an item was updated at, or found already
/// reading as it does.
///
/// Tried in this order, and the first that answers is the one reported: the explicit target
/// task, the link the item records, its own origin, a search of the destination for an item
/// recording it as its origin, and the caller's `--match-by`. When none answers, the item is created and says
/// so as [`NoCounterpart`] instead.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum CopyVia {
    /// The destination task explicitly named by the caller and verified by origin.
    TargetTask,
    /// The item's `onetaskgraph.copies` entry for the destination named an item whose own
    /// origin names this one back. Found by one read by id, and no scan.
    Link,
    /// The item's own `onetaskgraph.origin` named the destination item: a copy-back.
    Origin,
    /// A search of the destination for an item recording this one as its origin.
    Scan,
    /// A search of the destination by the caller's `--match-by`.
    Match,
    /// The item the destination holds under the key of a create an earlier attempt of this
    /// copy recorded its intent for, at `onetaskgraph.creating`: that attempt's create,
    /// finished rather than made twice.
    Intent,
}

/// What a created item reports as its `via`: no rule found a counterpart.
///
/// One word, and a type of its own rather than another [`CopyVia`], so an item updated at a
/// counterpart cannot say none was found and a created one cannot name a rule that found
/// one.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum NoCounterpart {
    /// No counterpart was found, so one was created — or, in a dry run, would have been.
    #[default]
    Created,
}

/// What a copy did to the `onetaskgraph.copies` entry the copied item holds for the
/// destination.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum CopyLink {
    /// The entry was written, because the item held none for this destination or held one
    /// naming another item.
    Recorded,
    /// Nothing was written: the entry already named this destination item, or the item was
    /// found by its own origin, where the correspondence is already recorded on the other
    /// side.
    Unchanged,
    /// The item's source cannot hold the entry — it has no write side, or it refuses the
    /// narrow metadata write — so the copy landed without recording it.
    Unrecorded,
    /// The copy was asked to write nothing to the source — `--no-link` — so no entry was
    /// recorded or changed. The next copy finds the counterpart by its own origin.
    Skipped,
}

impl CopyOutcome {
    /// The qualified id this outcome landed on, when it landed on one.
    #[must_use]
    pub fn destination(&self) -> Option<&GlobalId> {
        self.action.destination()
    }

    /// What the copy did to the task's comments at the destination, when it carried them.
    #[must_use]
    pub fn comments(&self) -> Option<&CommentCounts> {
        self.action.comments()
    }
}

/// The four things a copy can do to one item, and the id each of them is about.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "action", rename_all = "kebab-case")]
pub enum CopyAction {
    // llmlint: ignore[names_match_behavior] `created` is Contract D's serialized action
    // for both a completed create and a dry run that would create; the optional destination
    // distinguishes those cases, and renaming this public variant would break Rust callers.
    /// The destination held no counterpart, so one was created.
    Created {
        /// The id it was created under, or `null` for a dry run that would have created
        /// one — there is no id, because nothing was.
        destination: Option<GlobalId>,
        /// That no rule found a counterpart. The one word it can be, so a report written
        /// before there was a `via` reads as saying it.
        #[serde(default)]
        via: NoCounterpart,
        /// What the copy did to the link the copied item records for this destination at
        /// `onetaskgraph.copies`; absent for a dry run, which writes nothing.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        // llmlint: ignore[invalid_states_unrepresentable] Whether a copy is a dry run is a
        // property of the request, which the report does not carry, and the contract this
        // lands — shared with the follow-up tooling built against it — says a dry run leaves
        // `link` out rather than naming a sixth word for it. What holds it is `Engine::link`:
        // the one writer of this field, run for every item a copy that writes has landed and
        // for none of a dry run's, which `a_dry_run_says_which_rule_would_answer_and_records_no_link`
        // and every `link` assertion in tests/copy_link.rs drive.
        link: Option<CopyLink>,
        /// What the copy did to the task's comments at the destination — present for every
        /// task a `--comments` copy landed, a dry run's included, and absent for a copy without
        /// it and for a project and a document.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        comments: Option<CommentCounts>,
    },
    /// The destination held a counterpart and it now reads as the source does.
    Updated {
        /// The item that was updated.
        destination: GlobalId,
        /// Which rule found it.
        via: CopyVia,
        /// What the copy did to the copied item's link; absent for a dry run.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        // llmlint: ignore[invalid_states_unrepresentable] As `link` on `Created` above.
        link: Option<CopyLink>,
        /// What the copy did to the task's comments at the destination — present for every
        /// task a `--comments` copy landed, a dry run's included, and absent for a copy without
        /// it and for a project and a document.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        comments: Option<CommentCounts>,
    },
    /// The destination held a counterpart that already read that way; nothing of the item
    /// was written.
    ///
    /// A task's copy record — `onetaskgraph.copied_version`, `onetaskgraph.copied_with_comments`
    /// and `onetaskgraph.copied_destination_version` — may still be brought up to date on it,
    /// which changes nothing the item says.
    Unchanged {
        /// The item that already said it.
        destination: GlobalId,
        /// Which rule found it.
        via: CopyVia,
        /// What the copy did to the copied item's link; absent for a dry run.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        // llmlint: ignore[invalid_states_unrepresentable] As `link` on `Created` above.
        link: Option<CopyLink>,
        /// Whether the copy decided this from change tokens alone, without reading the item's
        /// body or comments from the source: the source's current `version` is the one the
        /// destination records at `onetaskgraph.copied_version`, and the destination reads as
        /// the copy left it. `false` for an item read and compared.
        #[serde(default)]
        skipped: bool,
        /// What the copy did to the task's comments at the destination — present for every
        /// task a `--comments` copy found unchanged, a skipped one's included, and absent for a
        /// copy without it and for a project and a document.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        comments: Option<CommentCounts>,
    },
    /// The destination holds a counterpart the source no longer does. A copy never
    /// deletes, so it was left exactly as it is — and no rule was asked where it goes, and
    /// no link was touched, because it was not copied.
    Orphaned {
        /// The item that was left alone.
        destination: GlobalId,
    },
    /// The item changed while the copy read it, and changed again while the copy read it a
    /// second time, so nothing was written for it — never a mix of two of its revisions.
    ///
    /// Every other item of the copy landed as it would have and is not undone, and the command
    /// exits `4`. Copying the item again once it has settled lands it.
    Failed {
        /// Why, in the failure document every verb reports a failure in.
        error: Failure,
    },
    /// The task named by id is at a status the copy's `--status` does not keep: read only as
    /// far as its status, and neither copied nor read for its comments. It does not change the
    /// copy's exit code.
    Skipped {
        /// Why, as `status <category> is not one of <categories>`.
        reason: String,
    },
    /// A dry run's item whose body is larger than its destination holds: the copy itself would
    /// refuse, as `oversize`, before writing anything. The command exits `4`.
    Oversize {
        /// The size of the body the item would store, as the destination's plugin counts it —
        /// what [`CopyOutcome::body_size`] reports beside it.
        size: u64,
        /// The most the destination holds in one body, as its plugin states it.
        limit: u64,
    },
}

impl CopyAction {
    /// The qualified id this action is about, when there is one.
    #[must_use]
    pub fn destination(&self) -> Option<&GlobalId> {
        match self {
            Self::Created { destination, .. } => destination.as_ref(),
            Self::Updated { destination, .. }
            | Self::Unchanged { destination, .. }
            | Self::Orphaned { destination } => Some(destination),
            Self::Failed { .. } | Self::Skipped { .. } | Self::Oversize { .. } => None,
        }
    }

    /// Which rule found the counterpart, or `None` when none did — the item was created —
    /// and for an orphan or a failed item, which no rule was asked about.
    #[must_use]
    pub fn found_by(&self) -> Option<CopyVia> {
        match self {
            Self::Updated { via, .. } | Self::Unchanged { via, .. } => Some(*via),
            Self::Created { .. }
            | Self::Orphaned { .. }
            | Self::Failed { .. }
            | Self::Skipped { .. }
            | Self::Oversize { .. } => None,
        }
    }

    /// What became of the copied item's link, or `None` for a dry run, an orphan and a
    /// failed item.
    #[must_use]
    pub fn link(&self) -> Option<CopyLink> {
        match self {
            Self::Created { link, .. }
            | Self::Updated { link, .. }
            | Self::Unchanged { link, .. } => *link,
            Self::Orphaned { .. }
            | Self::Failed { .. }
            | Self::Skipped { .. }
            | Self::Oversize { .. } => None,
        }
    }

    /// What a `--comments` copy did to the task's comments, or `None` for a copy without it, a
    /// project, a document, an orphan and a failed item.
    #[must_use]
    pub fn comments(&self) -> Option<&CommentCounts> {
        match self {
            Self::Created { comments, .. }
            | Self::Updated { comments, .. }
            | Self::Unchanged { comments, .. } => comments.as_ref(),
            Self::Orphaned { .. }
            | Self::Failed { .. }
            | Self::Skipped { .. }
            | Self::Oversize { .. } => None,
        }
    }

    /// Whether an unchanged item was decided from change tokens alone; `false` for every
    /// other action.
    #[must_use]
    pub fn skipped(&self) -> bool {
        matches!(self, Self::Unchanged { skipped: true, .. })
    }

    /// Whether this item landed — was created, updated or found unchanged — rather than being
    /// an orphan the copy left alone or an item it failed to read whole.
    fn landed(&self) -> bool {
        !matches!(
            self,
            Self::Orphaned { .. }
                | Self::Failed { .. }
                | Self::Skipped { .. }
                | Self::Oversize { .. }
        )
    }

    /// The link slot of an item this copy landed, `None` for an orphan and a failed item.
    fn link_slot(&mut self) -> Option<&mut Option<CopyLink>> {
        match self {
            Self::Created { link, .. }
            | Self::Updated { link, .. }
            | Self::Unchanged { link, .. } => Some(link),
            Self::Orphaned { .. }
            | Self::Failed { .. }
            | Self::Skipped { .. }
            | Self::Oversize { .. } => None,
        }
    }

    /// The word this action serializes as, taken from its own `Serialize`.
    ///
    /// Read back off the wire form rather than written out again in a `match`, for the
    /// reason `render::wire` gives: a second spelling of `unchanged` would be a second
    /// place for it to drift from the one a caller reads.
    #[must_use]
    pub fn name(&self) -> String {
        serde_json::to_value(self).expect("a contract enum serialises")["action"]
            .as_str()
            .expect("an internally tagged enum carries its tag")
            .to_owned()
    }
}

/// What one item points at, resolved as far as the copy has got: its forward edges and the
/// tasks it delivers.
struct Pointing<'a> {
    /// Its forward edges, `None` where the far end is a member not landed yet.
    edges: &'a [Option<DependencyEdge>],
    /// The tasks it delivers that can already be named at the destination.
    delivers: &'a [TaskRef],
}

/// Where one item is going at the destination.
#[derive(Clone)]
enum Target {
    /// Update the destination item with this id, reached by the rule named.
    Update {
        /// The destination item this copy updates.
        id: NativeId,
        /// Which rule found it.
        found: Found,
    },
    /// Create one.
    Create,
}

/// Which of the rules above found the destination item a copy is updating.
///
/// Every one is the same instruction — update that item — and rule 1 is a different answer
/// about the origin, which is why the distinction is carried this far rather than dropped
/// where it is made. See [`recorded`]. It is also what the report's [`CopyVia`] says and
/// what decides whether the copied item's link is written.
#[derive(Clone, PartialEq, Eq)]
enum Found {
    /// The caller named this task, whose origin was verified.
    TargetTask,
    /// The item's own link named it, and it names the item back.
    Link,
    /// Rule 1: the item being copied already named it, so this copy is a copy-back.
    Origin,
    /// Rule 2: the destination was searched for an item recording this one as its origin.
    Scan,
    /// The caller's matching escape.
    Match,
    /// An earlier attempt's create, found by the key its intent recorded.
    Intent,
    /// The project a copy would otherwise create is one the destination already holds of the
    /// same plan — the member its counterpart home elsewhere names there, or one naming that
    /// home as the project it is a member of — and it becomes this project's own copy.
    Member {
        /// That home, which names it no longer once it lands.
        home: GlobalId,
    },
}

impl Found {
    /// The word the report says it in.
    ///
    /// A member taken over is reported as found by a search of the destination: it is one,
    /// for the plan's project rather than for an origin, and a word of its own would be a new
    /// value in every consumer's schema for a distinction none of them acts on.
    fn via(&self) -> CopyVia {
        match self {
            Self::TargetTask => CopyVia::TargetTask,
            Self::Link => CopyVia::Link,
            Self::Origin => CopyVia::Origin,
            Self::Scan | Self::Member { .. } => CopyVia::Scan,
            Self::Match => CopyVia::Match,
            Self::Intent => CopyVia::Intent,
        }
    }
}

impl Target {
    /// Which rule found this target, or `None` for one this copy creates.
    fn found(&self) -> Option<&Found> {
        match self {
            Self::Update { found, .. } => Some(found),
            Self::Create => None,
        }
    }
}

/// What a scan of the destination is looking for.
enum Wanted {
    /// An item recording this qualified id as its origin.
    Origin(String),
    /// An item whose title is this.
    Title(String),
    /// An item holding this value at this metadata key.
    Metadata(String, Value),
}

impl Wanted {
    /// Whether one destination item is the one being looked for.
    fn found(&self, title: &str, metadata: &BTreeMap<String, Value>) -> bool {
        match self {
            Self::Origin(id) => {
                metadata.get(GlobalId::ORIGIN_KEY) == Some(&Value::String(id.clone()))
            }
            Self::Title(wanted) => title == wanted,
            Self::Metadata(key, value) => metadata.get(key) == Some(value),
        }
    }

    /// The task query that asks `capabilities`' source for this item rather than for all of
    /// them, when that source applies the predicate itself.
    ///
    /// Only ever a narrowing the source declared it applies, so a source that cannot is
    /// asked for everything exactly as before; and never the last word, because
    /// [`found`](Self::found) still confirms each row. A text search is wider than an equal
    /// title, and a source may answer a match it cannot yet see as absent — which is the
    /// same answer the whole-store walk gave for an item its own listing was behind on.
    ///
    /// A title or a value with no letter or digit is never pushed down: a source whose search
    /// indexes words cannot answer it and may refuse it, and the walk it replaces answered it
    /// already, so it is asked for everything as it always was.
    fn task_query(&self, capabilities: &Capabilities) -> TaskQuery {
        match self {
            Self::Origin(id) if capabilities.filter_by_origin.is_native() => TaskQuery {
                origin: Some(id.clone()),
                ..TaskQuery::default()
            },
            Self::Title(title) if capabilities.search_title.is_native() && has_words(title) => {
                TaskQuery {
                    text: Some(TextQuery {
                        terms: title.clone(),
                        fields: TextFields::Title,
                    }),
                    ..TaskQuery::default()
                }
            }
            Self::Metadata(key, Value::String(value))
                if capabilities.filter_by_metadata.is_native() && has_words(value) =>
            {
                MetadataMatch::new(key.clone(), Vec::new(), value.clone())
                    .map(|wanted| TaskQuery {
                        metadata: vec![wanted],
                        ..TaskQuery::default()
                    })
                    .unwrap_or_default()
            }
            _ => TaskQuery::default(),
        }
    }
}

/// How far a walk of the destination for what is [`Wanted`] reads.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Walk {
    /// To the first item found — what a copy needs to pick its counterpart.
    First,
    /// To the last page, for every item found — what `task locate` lists as carriers.
    Every,
}

/// Whether `terms` holds a letter or a digit — a word a search that indexes words can find.
fn has_words(terms: &str) -> bool {
    terms.chars().any(char::is_alphanumeric)
}

/// What the destination held before this copy touched one item.
///
/// Read once, in [`Engine::land`], and used three times over: to decide whether the write
/// would change anything, to repair the item's edges once the rest of the copy has landed,
/// and — if the copy cannot finish — to put the item back exactly as it was.
#[derive(Clone)]
struct Prior {
    /// The item as the destination held it.
    item: Item,
    /// Its forward edges there.
    edges: Vec<DependencyEdge>,
    /// The image assets it held there, with the bytes of each its own record does not
    /// already say the destination serves — `None` when it held none and no write of this
    /// copy carried any, so putting it back writes it exactly as it did before assets.
    assets: Option<Vec<AssetPayload>>,
    /// What its `onetaskgraph.assets` says the destination serves, checked where it was read.
    recorded: Option<AssetUploads>,
    /// Its comments there, every page of them, when a `--comments` copy read them — `None`
    /// for every other read, and for an item that is not a task.
    comments: Option<Vec<Comment>>,
    /// The far ends of each `related` edge the destination holds of its own accord, because a
    /// body this copy writes links one end from the other: see
    /// [`Engine::note_derived_relations`].
    derived: Vec<DependencyEndpoint>,
}

impl Prior {
    /// Its forward edges there as a copy compares them: without those the destination derived,
    /// unless this copy writes the same edge itself (`outgoing`).
    fn counted_edges(&self, outgoing: &[DependencyEdge]) -> Vec<DependencyEdge> {
        self.edges
            .iter()
            .filter(|edge| {
                !self.derived.contains(&edge.to)
                    || edge.kind != DependencyKind::Related
                    || outgoing.iter().any(|sent| sent.to == edge.to)
            })
            .cloned()
            .collect()
    }
}

/// One item that landed with an edge whose far end was not written yet.
///
/// Held until every item of the whole copy has landed, because the far end may be in
/// another project of the same command: a copy of two projects at once is one copied set,
/// not two, and an edge across them is remapped rather than written as a foreign id.
struct Deferred {
    /// The item, as it was read and resolved.
    item: Planned,
    /// The destination project it was filed under.
    filed: Option<NativeId>,
    /// Where it landed.
    destination: NativeId,
    /// What the destination held there before, when it held anything.
    prior: Option<Prior>,
}

/// Whether one write of a task stamps the record of the copy that lands it (see [`record`]),
/// or leaves the record the destination holds — none, for a task it creates — where it is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Stamping {
    /// Stamp it: a copy that can skip — `--comments`, or by query — always records itself, and
    /// so does one bringing a stale record up to date.
    Always,
    /// Stamp it only where the write carries what the record lives beside anyway: the task's
    /// content or a metadata key. A change of its title, status, priority or edges alone is
    /// then written as that member and nothing else, rather than as a rewrite of the body that
    /// would carry the record and nothing more. A record left as it was is only ever older
    /// than the task it is on, which a later copy reads as a reason to compare, never to skip,
    /// so the task is never served from a skip it should not have been.
    WithCarrier,
    /// Never: a task created together with others, whose edges to them could not be named when
    /// its content was sent, so no digest of what it holds could be true. It lands with no
    /// record, which a later copy reads as a reason to compare rather than to skip, and the next
    /// copy that writes its content records it.
    Never,
}

impl Stamping {
    /// How a write made by `request` records itself, `rerecord` when it is made only to bring
    /// a stale record up to date.
    fn of(request: &CopyRequest, rerecord: bool) -> Self {
        if rerecord || request.comments || request.items.query().is_some() {
            Self::Always
        } else {
            Self::WithCarrier
        }
    }

    /// Whether a write landing `task` over what the destination held as `prior` stamps this
    /// copy's record on it.
    fn stamps(self, task: &Task, prior: Option<&Prior>) -> bool {
        match self {
            Self::Always => true,
            Self::Never => false,
            Self::WithCarrier => match prior.map(|prior| &prior.item) {
                Some(Item::Task(held)) => {
                    held.content != task.content
                        || record::without_record(&held.metadata)
                            != record::without_record(&task.metadata)
                }
                _ => true,
            },
        }
    }
}

/// What one item's undo has to do to put the destination back.
///
/// Each entry names the source it was written to, because a routed copy writes into more
/// than one, and it is undone in every one of them.
enum Undo {
    /// The copy created it, so undoing means removing it.
    Created {
        /// The source it was created in.
        at: SourceName,
        /// Which write interface removes it.
        kind: Level,
        /// The destination id it was created under.
        id: NativeId,
    },
    /// The copy overwrote something, so undoing means writing that something back.
    ///
    /// No `kind` beside the id, unlike the variant above: what was there says which of the
    /// two write interfaces takes it back, and a second spelling of that could disagree
    /// with it.
    Updated {
        /// The source it was overwritten in.
        at: SourceName,
        /// The destination id that was overwritten.
        id: NativeId,
        /// What was there before.
        prior: Prior,
    },
    /// The copy asked for a public projection project carrying `allocation` — recorded before
    /// the create was sent, so a project stored under an id the answer never said is still
    /// found, and taken back, by the allocation alone.
    Allocated {
        /// The public source it was asked of.
        at: SourceName,
        /// The allocation it carries.
        allocation: projections::Allocation,
        /// The private home whose map records the allocation: kept as it is when the project
        /// cannot be taken back, so the next copy still finds the project it made.
        home: GlobalId,
    },
}

impl Undo {
    /// The source this entry was written to.
    fn at(&self) -> &SourceName {
        match self {
            Self::Created { at, .. } | Self::Updated { at, .. } | Self::Allocated { at, .. } => at,
        }
    }

    /// The destination id this entry is about: none for a project asked for behind an
    /// allocation, whose id is what a lost answer withholds.
    fn id(&self) -> Option<&NativeId> {
        match self {
            Self::Created { id, .. } | Self::Updated { id, .. } => Some(id),
            Self::Allocated { .. } => None,
        }
    }

    /// Which of the destination's three write interfaces this entry belongs to.
    ///
    /// An id alone does not identify a destination item: nothing stops a destination
    /// numbering its tasks and its projects in one namespace, and a local-Markdown store
    /// filing `alpha.md` under both is the ordinary case rather than the contrived one.
    /// So this pairs with `id` wherever one entry has to be told from another.
    fn kind(&self) -> Level {
        match self {
            Self::Created { kind, .. } => *kind,
            Self::Allocated { .. } => Level::Project,
            // Read off what was there, for the reason the variant carries no `kind` of
            // its own: two spellings of one fact can disagree, and this one cannot.
            Self::Updated { prior, .. } => prior.item.level(),
        }
    }
}

/// Refuse a write of `item` whose status `destination` has no name for, before the write: a
/// document carries no status, so it asks nothing.
async fn check_status(
    destination: &ResolvedSource,
    item: &Item,
    target: Option<&NativeId>,
) -> Result<(), EngineError> {
    let (kind, category) = match item {
        Item::Task(task) => (ItemKind::Task, task.status.category),
        Item::Project(project) => (ItemKind::Project, project.status.category),
        Item::Document(_) => return Ok(()),
    };
    destination
        .source()
        .check_status_write(kind, category, target)
        .await
        .map_err(|error| refused(destination, error))
}

/// One intent to create a copy took: the kind and id of the item it is on, the destination, the
/// item's file as it was before when the item is a file on this machine, and the intent itself.
type Intended = (
    Level,
    GlobalId,
    SourceName,
    Option<Vec<u8>>,
    Option<intent::Intent>,
);

/// Everything one copy has written, in the order it wrote it, so a copy that cannot finish
/// can undo its own writes.
///
/// This is not state the engine keeps: it lives for the length of one `copy` call and is
/// dropped with it, so the invariant that nothing of a user's work is written down outside
/// the plugin that owns it is untouched.
#[derive(Default)]
struct Journal {
    /// One entry per destination item this copy first touched, in that order.
    entries: Vec<Undo>,
    /// Every item this copy landed, by interface and source id, and the id it landed under —
    /// which a later body's link to it names.
    landed: HashMap<(Level, GlobalId), NativeId>,
    /// The title of each project and document this copy wrote, by destination and by its address
    /// there: what a destination that titles a bare link to one links it under.
    titles: HashMap<SourceName, BTreeMap<String, String>>,
    /// Each item written with links not addressable yet, its latest write alone.
    sections_pending: Vec<SectionsPending>,
    /// Each copied item's section-link figures, as its last rendering left them.
    sections: HashMap<GlobalId, SectionLinks>,
    /// Each comment written with links not addressable yet, finished with the rest.
    comments_pending: Vec<CommentPending>,
    /// Each task a `--comments` write stamped with a record, by its source and id at the
    /// destination: what that write sent, so a comment finished after it can be brought into
    /// the record it digests.
    stamped: HashMap<(SourceName, NativeId), Stamped>,
    /// What each copied task's carried comments could not do for their section links.
    comment_sections: HashMap<GlobalId, sections::SectionCounts>,
    /// One entry per copied item whose `onetaskgraph.copies` link this copy wrote, in that
    /// order.
    ///
    /// Kept apart from `entries` because these are the *sources'* items rather than the
    /// destination's, and an id alone does not say which store it is in: a copy inside one
    /// source can write a link on an item and land on another item with the same kind.
    links: Vec<Unlink>,
    /// One entry per comment write this copy made, in the order it made them.
    ///
    /// Undone before the items they are on, in reverse: a copy created on a task this copy
    /// created goes with the task anyway, and one on a task it updated is taken back here.
    comments: Vec<CommentUndo>,
    /// One follow-up write per destination item whose write's own answer said it stores other
    /// content than it was sent — the latest write of each item only — sent once every link is
    /// written; see [`Engine::follow_up`].
    ///
    /// Nothing here is undone of its own: each is a write of an item an entry above already
    /// puts back.
    follow_ups: Vec<FollowUp>,
    /// The change token each copied item's source answered with once this copy wrote its link
    /// there, by the item's qualified id — what a follow-up records the source as.
    linked_versions: HashMap<GlobalId, String>,
    /// Every intent to create this copy took, by the item it is on and its destination: a
    /// copy that cannot finish gives up each lease, keeping the key, so its retry takes over at
    /// once.
    intents: Vec<Intended>,
    /// Every item whose create was sent and not yet answered: a copy failing with one here
    /// cannot say whether the create landed.
    sending: Vec<GlobalId>,
    /// Every item, by its source id and the destination, whose create this copy has sent — or
    /// whose earlier attempt's create it took over — while it holds the intent: each further
    /// destination write for it renews the lease first.
    owned: Vec<(Level, GlobalId, SourceName)>,
    /// Whether a renewal found another attempt holding an intent this copy had held: the copy
    /// then stops, writing nothing more — not even to take its own writes back, which are the
    /// new holder's to resolve.
    ownership_lost: bool,
}

/// How a follow-up's own answer is held to the content it sends.
#[derive(Clone, Copy, PartialEq, Eq)]
enum FollowUpAnswer {
    /// It sends back what the destination stored, so its answer reports that byte for byte.
    Exact,
    /// It holds a link finished in it, which the destination has not been sent and may normalize.
    MayNormalize,
}

/// One write sent again, as its own request, because the destination stores other content than
/// it was sent: that stored content verbatim, with every fingerprint the item keeps recomputed
/// over it — see `stored`.
struct FollowUp {
    /// The source the destination item is in.
    at: SourceName,
    /// The destination item.
    id: NativeId,
    /// The item copied, at its source.
    source: GlobalId,
    /// The item as the write that lands it again sends it: the stored content, its rendering's
    /// provenance moved onto it where it vouched for what was sent, and the record below.
    item: Item,
    /// Its forward edges, as the first write sent them.
    edges: Vec<DependencyEdge>,
    /// The record of this copy it lands with, digested over the stored content when it is sent
    /// — `None` for a write that records none. A version that is its source's change token is
    /// replaced by the token the source's answer to this copy's own link write gave, which that
    /// write moves; a composed digest no link moves.
    record: Option<record::Record>,
    /// The comments its record digests, after a `--comments` copy.
    comments: Option<Vec<Value>>,
    /// The content the destination said it stores, which the follow-up's own answer has to
    /// report again.
    stored: String,
    /// How its answer is held to what it sends.
    answer: FollowUpAnswer,
}

/// How to take back one comment write a `--comments` copy made.
struct CommentUndo {
    /// The source the destination task is in.
    at: SourceName,
    /// The destination task.
    task: NativeId,
    /// What to do.
    undo: CommentUndoing,
}

/// What taking one comment write back does.
enum CommentUndoing {
    /// The copy added this comment: delete it.
    Added(NativeId),
    /// The copy edited this comment from what `before` said: write that back.
    Edited {
        /// The comment.
        id: NativeId,
        /// What it was, origin included.
        before: CommentCopy,
    },
    /// The copy deleted a copy that said this: write it back, as a new comment.
    Deleted(CommentCopy),
}

/// How to take back one link this copy wrote on a copied item.
struct Unlink {
    /// The item, at its own source.
    item: GlobalId,
    /// Which of that source's interfaces holds it.
    level: Level,
    /// Which of the two keys a copy records where an item landed under was written:
    /// `onetaskgraph.copies` or `onetaskgraph.copy_locations`.
    key: MetadataKey,
    /// What it held at that key before, or `None` when it held nothing there.
    before: Option<Value>,
}

impl Journal {
    /// Record what has to happen to put one destination item back.
    ///
    /// The *first* entry for an id is the one that matters and later ones are dropped: an
    /// item written twice — once as it lands, once when its edges are repaired — was only
    /// ever one thing before this copy started, and that is what undoing it restores.
    fn record(&mut self, entry: Undo) {
        // Each allocation is fresh, so an entry for one is never a second write of an item.
        if entry.id().is_some()
            && self.entries.iter().any(|held| {
                held.at() == entry.at() && held.kind() == entry.kind() && held.id() == entry.id()
            })
        {
            return;
        }
        self.entries.push(entry);
    }

    /// Carry what a later write of the destination item `id` in `at` changed of its metadata —
    /// from `before`, the item as that write read it, to `after`, as it sent it — into the
    /// follow-up still owed that item, if one is.
    ///
    /// A follow-up is composed from the write that landed the item, so without this it would
    /// send that write's metadata again and take back every key a write since recorded: the
    /// home's `onetaskgraph.members`, a task's dependents, a private home's
    /// `onetaskgraph.public_projections`. Only the keys that write changed are carried, so
    /// everything the follow-up itself restamps over the stored content stays as it composed it.
    fn carry_into_follow_up(
        &mut self,
        at: &SourceName,
        id: &NativeId,
        before: &BTreeMap<String, Value>,
        after: &BTreeMap<String, Value>,
    ) {
        let Some(follow) = self
            .follow_ups
            .iter_mut()
            .find(|follow| &follow.at == at && &follow.id == id)
        else {
            return;
        };
        let metadata = metadata_of(&mut follow.item);
        for (key, value) in after {
            if before.get(key) != Some(value) {
                metadata.insert(key.clone(), value.clone());
            }
        }
        for key in before.keys().filter(|key| !after.contains_key(*key)) {
            metadata.remove(key);
        }
    }
}

/// What one copy invocation carries from the item it lands to the next.
///
/// Like the [`Journal`] beside it, this lives for the length of one `copy` call and is
/// dropped with it: nothing here is state the engine keeps, and nothing in it is read back
/// to answer a later query.
#[derive(Default)]
struct Running {
    /// The private addresses checked by this invocation, never persisted.
    private_locations: Option<Vec<String>>,
    /// Every item an edge of this command resolves to a destination id rather than writing as
    /// it was read: the ids named, what travels with them, and — for a member copy — each
    /// member the copy does not carry whose own recorded origin already names its
    /// destination item. That last kind is resolvable without being copied.
    resolvable: Vec<GlobalId>,
    /// The destination item each of those corresponds to, as soon as it is known — whether
    /// this command found it, landed it, or read it off a member's recorded origin.
    ///
    /// Keyed by the qualified id's own rendering, which is what a recorded origin holds
    /// anyway — making `GlobalId` orderable for a local map would put an ordering on a
    /// contract type for a reason no caller of it has.
    ///
    /// Qualified, because a routed copy lands items in more than one source, and whether an
    /// edge to one is written bare or qualified depends on where both of its ends landed.
    counterparts: BTreeMap<String, GlobalId>,
    /// The items held back for [`Engine::repair`], across the whole request.
    deferred: Vec<Deferred>,
    /// The reference figures, one total for the whole invocation rather than one per
    /// document.
    references: Counted,
    /// The destination project each source project corresponds to, once this command has
    /// looked for it — so a second task filed under the same project does not walk the
    /// destination for it again.
    ///
    /// Keyed by the source project and the source the item lands in, because a routed task
    /// lands in its home's member project rather than in the home.
    filings: BTreeMap<(String, SourceName), Option<NativeId>>,
    /// Every source whose first page of projects this command read to pin them — see
    /// [`Engine::pin_filing_projects`] — so it is read at most once.
    projects_paged: BTreeSet<SourceName>,
    /// Every home project this copy has learned the member projects of, by the home's
    /// qualified id: which members it holds, and whether this copy added one.
    homes: BTreeMap<String, Home>,
    /// `delivers` entries naming a member of the copied set, over the whole invocation.
    delivers_rewritten: u64,
    /// Every task this copy landed, for the relation rule once the whole copy is complete.
    landed: Vec<LandedTask>,
    /// Every task this copy landed somewhere not declared private, with its forward edges as
    /// its source reported them: an edge of one to a task of a source declared private is
    /// withheld from it and recorded on that task once the whole copy has landed.
    dependents: Vec<Dependent>,
    /// The member projects each home this copy read names, by the home's qualified id: where a
    /// task filed under it can be recorded as depending on a task it cannot hold the edge to
    /// itself. Noted by the read that classifies the home, or read once per home per command.
    home_members: HashMap<String, Vec<GlobalId>>,
    /// Every task this copy landed in a source declared private, by its destination id, with
    /// the dependents it held there before: the record a copy keeps as it found it.
    recorders: BTreeMap<String, (GlobalId, Dependents)>,
    /// Every item this copy landed, in the order it landed them, for the link each records
    /// once the whole copy has landed.
    linking: Vec<Linking>,
    /// Each source project's classification once this command has worked it out — its own,
    /// its repositories', and every task's and document's it holds — so a second item filed
    /// under it does not read the whole project again.
    project_classes: HashMap<GlobalId, Classification>,
    /// Whether every item this command lands was already held to the public boundary, as a
    /// project copy does for all of them before it writes the first.
    preflighted: bool,
    /// Every item this command carries that is classified private, by its source id and by
    /// where it lands: an id naming one is withheld from a write anywhere not declared
    /// private (see [`Engine::withholds`]), whichever source it was read from.
    private: Vec<GlobalId>,
    /// Every item outside this copy that a carried reference names and whose classification
    /// this command has read as public, so a second reference to it is not read again.
    public_references: Vec<GlobalId>,
    /// Each task's first detail read, made for many tasks of one source at once before any of
    /// them is planned, keyed by the qualified id's own rendering and taken by the plan that
    /// reads it.
    prefetched: HashMap<String, Result<Option<TaskDetailRead>, SourceError>>,
    /// Where a task copied by query is going, already decided from its listing when the copy
    /// asked whether it could skip it — taken by the plan that lands it, so the destination is
    /// not read for it twice.
    aimed: HashMap<String, (Target, Option<Prior>)>,
    /// Each source project an item of this copy is filed under, read once when the copy read
    /// the destination counterparts ahead — see [`Engine::read_ahead`] — so the filing lookup's
    /// own read of it, [`Engine::linked_project`], is that read rather than a second one.
    source_projects: HashMap<GlobalId, Project>,
    /// Each destination project a document of this copy was filed under on its own project's
    /// link alone, unread, by the project's qualified destination id — and the source project
    /// whose link named it, which a write refused there is answered with a stale link of.
    trusted: HashMap<GlobalId, GlobalId>,
    /// Each destination the copy read ahead asked about the origin of a project, with that
    /// origin: its answer is that destination's to hold for this command, so the filing lookup
    /// confirms a link to that project by it rather than by a read of the project.
    asked_origins: Vec<(SourceName, GlobalId)>,
    /// The documents a `project copy --documents` carries with each project, by the project's
    /// qualified id, in the order its source lists them.
    plan_documents: HashMap<GlobalId, Vec<GlobalId>>,
    /// The size of the body each item this copy writes would store, by its source id, as its
    /// destination's plugin answered — taken before the first write, so a body larger than its
    /// destination holds refuses the whole copy while nothing is written.
    sizes: HashMap<GlobalId, BodySize>,
    /// Whether the copied set holds work — tasks or projects — beside any documents, as every
    /// copy but a document copy's does: a document's reference to a member of the set that the
    /// destination does not hold yet is then one this copy lands, and is finished once it has.
    carries_work: bool,
}

/// The member project a routed write was filed under, and what finding it wrote.
pub(super) struct Filed {
    /// The member project, or `None` when the home is not there to have one.
    pub(super) project: Option<NativeId>,
    /// The member created and the home's list rewritten, if either was.
    journal: Journal,
}

/// The tasks a member project is found or created to file, as a routed project copy knows them.
struct MemberFiling {
    /// The strictest of their classifications.
    classification: Classification,
    /// The tasks, by their source ids.
    tasks: Vec<GlobalId>,
}

/// Where a routed project copy lands its home, and how the tasks it carries follow it.
struct Homing {
    /// The source it lands in, and the route entry sending it there for the report.
    home: Placement,
    following: Following,
}

/// How the tasks a routed project copy carries follow its home.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Following {
    /// Every task goes with the home, whatever its repositories say: a plan holding anything
    /// private that a `classification: private` route sends wholly to one source.
    Whole,
    /// A task naming a repository goes where its own repositories place it, and one naming
    /// none goes with the home.
    ByRepositories,
}

/// One home project, and the member projects it names at `onetaskgraph.members`.
struct Home {
    /// The home, where it landed.
    id: GlobalId,
    /// Its title and status, which a member created for it takes.
    title: String,
    /// See `title`.
    status: onetaskgraph_plugin_api::Status,
    /// Who may read it, which a member created for it is too.
    classification: Classification,
    /// The members it names, at most one per source.
    members: Vec<GlobalId>,
    /// Whether this copy added, replaced or released one, so the home's own list is written
    /// once the copy has landed.
    changed: bool,
}

impl Home {
    /// A home as its source holds it, with the members it already names.
    ///
    /// # Errors
    ///
    /// The home's own source refusing, as malformed, a member list [`members_of`] cannot
    /// read: a copy filing into a plan it cannot read whole would add a second member beside
    /// one it failed to see.
    fn of(id: GlobalId, held: &Project) -> Result<Self, EngineError> {
        let members =
            members_of(&id, &held.metadata).map_err(|message| EngineError::SourceRefused {
                name: id.source.to_string(),
                error: SourceError::Malformed { message },
            })?;
        Ok(Self {
            id,
            title: held.title.clone(),
            status: held.status.clone(),
            classification: held.classification,
            members,
            changed: false,
        })
    }
}

/// The member projects the home `home` records at [`MetadataKey::MEMBERS_KEY`]; none when it
/// records the key not at all.
///
/// # Errors
///
/// Why, when the key holds anything but a list of qualified ids, each in a different source
/// from the home and from the others — the one shape the routed write that keeps it writes.
pub(crate) fn members_of(
    home: &GlobalId,
    metadata: &BTreeMap<String, Value>,
) -> Result<Vec<GlobalId>, String> {
    let Some(value) = metadata.get(MetadataKey::MEMBERS_KEY) else {
        return Ok(Vec::new());
    };
    let refuse = |why: String| {
        format!(
            "{home} records {key} as {value}, which {why}; a home's member list is a list of \
             qualified project ids, at most one per source and none in the home's own",
            key = MetadataKey::MEMBERS_KEY
        )
    };
    let Value::Array(entries) = value else {
        return Err(refuse("is not a list".to_owned()));
    };
    let mut members: Vec<GlobalId> = Vec::new();
    for entry in entries {
        let member = entry
            .as_str()
            .and_then(|entry| entry.parse::<GlobalId>().ok())
            .ok_or_else(|| refuse(format!("holds {entry}, which is not a qualified id")))?;
        if member.source == home.source || members.iter().any(|kept| kept.source == member.source) {
            return Err(refuse(format!(
                "names {member}, a second project in {}",
                member.source
            )));
        }
        members.push(member);
    }
    Ok(members)
}

/// One item a copy landed, and what settling its `onetaskgraph.copies` link needs.
struct Linking {
    /// The item, at its own source.
    item: GlobalId,
    /// Which of that source's interfaces holds it.
    level: Level,
    /// Which rule found where it landed, or `None` when this copy created it.
    found: Option<Found>,
    /// Where it landed.
    destination: GlobalId,
    /// What it held at `onetaskgraph.copies` when it was read, if anything.
    held: Option<Value>,
    /// What it held at `onetaskgraph.copy_locations` when it was read, if anything.
    held_locations: Option<Value>,
    /// Where the destination reports it landed, when the copy knows that without asking: the
    /// counterpart's read, or what the destination's write answered.
    location: Option<Location>,
    /// Whether the item records an intent to create for this destination, which the link
    /// write removes.
    intent: bool,
}

/// One task a copy landed, and what the relation rule reads of it once the copy is complete.
struct LandedTask {
    /// Where it landed.
    destination: GlobalId,
    /// The source it was read from, which a bare `delivers` entry names a task of.
    origin: SourceName,
    /// Its `delivers`, as its source reported them.
    delivers: Vec<TaskRef>,
    /// The `delivers` the destination held there before this copy, qualified.
    before: Vec<GlobalId>,
    /// Its status category, as the copy wrote it.
    category: StatusCategory,
    /// Who may read it, as the copy wrote it.
    classification: Classification,
}

/// A task a copy landed somewhere not declared private, with its forward edges as its source
/// reported them.
struct Dependent {
    destination: GlobalId,
    /// The source it was read from, which a bare far end names a task of.
    origin: SourceName,
    /// Its forward edges, as its source reported them.
    edges: Vec<DependencyEdge>,
    /// Whether an earlier copy can have kept an edge of it on a private source's side: the
    /// destination already held it, carrying a copy's record and not the mark of every edge on
    /// its own record — the rule [`Engine::reads_recorded_for`] applies to a dependency read.
    /// Only then are the sources that may record one asked about it.
    asks_recorders: bool,
}

/// What one task of a source declared private records once a copy has landed: what it held,
/// less every dependent the copy landed, plus each of those whose edges name it.
struct Recording {
    at: GlobalId,
    /// The dependents this copy landed whose edges name it.
    named: Dependents,
    /// What it held before, when this copy landed it and so already read it.
    held: Option<Dependents>,
}

/// Every dependent a copy landed, whose entries that copy decides, and what each task of a
/// source declared private it touches is to record.
struct Recordings {
    /// The destination ids of every task the copy landed somewhere not declared private.
    decided: Dependents,
    /// Those of them an earlier copy can have recorded on a private side, which alone the
    /// sources that may record one are asked about.
    asked: Vec<GlobalId>,
    /// One per task of a source declared private the copy landed or a landed edge names.
    each: Vec<Recording>,
}

/// A task a copy landed, with the tasks it delivers now and delivered before, qualified.
struct Deliverer {
    destination: GlobalId,
    classification: Classification,
    category: StatusCategory,
    now: Vec<GlobalId>,
    before: Vec<GlobalId>,
}

/// One item, read and resolved, on its way into the destination.
struct Planned {
    /// The caller's status policy, applied at comparison and write.
    status_policy: StatusPolicy,
    /// Where it came from.
    source: GlobalId,
    /// The item as its source reported it.
    item: Item,
    /// Its status before destination normalization, for the source's composed version.
    source_status: Option<Status>,
    /// Its forward edges, as its source reported them.
    edges: Vec<DependencyEdge>,
    /// Where it is going.
    target: Target,
    /// What the destination held at that target, read once where the target was found.
    ///
    /// The read that decided an item exists is the read of what it holds, so the two are
    /// one round trip rather than two against a hosted destination.
    held: Option<Prior>,
    /// The source it lands in: the copy's destination, or the source a route sent it to.
    to: SourceName,
    /// What the report says placed it there, for a copy into a source with routes.
    placed: Option<Placement>,
    /// The image assets its content references, with their bytes as its source holds them,
    /// in the order the content first references them; none for a project.
    assets: Vec<AssetPayload>,
    /// Who may read it: its own declaration tightened by its repositories and its project —
    /// for a project, by every task and document it holds. It lands as this.
    classification: Classification,
    /// Whether its source is declared private and where it lands is not, so its provenance —
    /// the origin naming its source, any link naming a private source — is left behind
    /// rather than written where it would name somewhere private.
    withheld: bool,
    /// The root of the chain of copies it is one of, recorded where it lands at
    /// [`GlobalId::LINEAGE_KEY`]: its source's own lineage, else the origin its source records,
    /// as the source holds it — `None` when its source records neither or records its own id,
    /// where the origin it lands with is the root, and when it names a source whose provenance
    /// this write withholds.
    lineage: Option<Value>,
    /// Its comments at the source, every page of one revision of them, exactly when the copy
    /// carries them — a task of a `--comments` copy — and `None` otherwise, a copy that read a
    /// task's comments only to confirm its comment activity included.
    carried: Option<Vec<Comment>>,
    /// What the copy does to its comments at the destination, decided before it lands.
    comment_plan: Option<comments::CommentPlan>,
    /// Whether the copy found it unchanged at both ends by its change tokens alone, so nothing
    /// of it is written or compared.
    skipped: bool,
    /// What the copy's status condition found at the counterpart it updates — `None` when the
    /// copy set none, or creates the item.
    status_condition: Option<StatusCondition>,
    /// What the copy's priority condition found at the counterpart task it updates — `None`
    /// when the copy set none, or creates the item. Unmet, the counterpart keeps its priority.
    priority_condition: Option<PriorityCondition>,
    /// What the copy's assignee condition found at the counterpart task it updates — `None`
    /// when the copy set none, or creates the item.
    assignee_condition: Option<super::AssigneeCondition>,
    /// The assignees the copy writes for a task — `None` leaving the destination's as they
    /// are, or creating it unassigned — decided by [`Engine::decide_assignees`].
    assignees: Option<Vec<String>>,
    /// The intent this copy holds to create its counterpart, once taken — and the one an
    /// earlier attempt recorded, when this copy creates again under it.
    intent: Option<intent::Intent>,
    /// Whether the destination is listed again by the create's key once the create is sent,
    /// to report a duplicate another attempt's create left beside it.
    relist: bool,
    /// Its content's references and section links, as its source spelled them, when it holds
    /// any a destination writes otherwise: rendered again at every write of it.
    links: Option<sections::BodyLinks>,
    /// The links of its content not addressable when it was planned, as [`Self::links`]
    /// wrote them.
    pending: Vec<sections::Pending>,
    /// Each carried comment whose links were not all addressable when it was planned, by the
    /// origin its copy carries: finished once the task has landed.
    comment_links: Vec<CommentLinks>,
    /// What its carried comments could not do for their section links, for the comments with
    /// nothing left to finish.
    comment_sections: sections::SectionCounts,
    /// What the sanitisation that admitted its copy rewrote, applied to every value the copy
    /// composes for it at a write — its origin, which names its source's id, among them.
    rewrite: Rewrite,
}

impl Planned {
    /// Whether the counterpart keeps its own priority, because the copy's priority condition
    /// did not hold of it.
    fn keeps_priority(&self) -> bool {
        self.priority_condition
            .is_some_and(|condition| !condition.met)
    }
}

/// One item read out of its source, before anything decides where it lands.
struct Read {
    /// Where it came from.
    source: GlobalId,
    /// The item as its source reported it.
    item: Item,
    /// Its comments, every page of one revision of them, when a `--comments` copy read them.
    comments: Option<Vec<Comment>>,
}

/// Why a copy names one destination project, which decides whether pinning the project itself
/// in that destination's cache can save a read.
#[derive(Clone, Copy, PartialEq, Eq)]
enum ProjectNamed {
    /// The project's counterpart, which the copy writes and so reads with its forward edges.
    Copied,
    /// The project a copied document is filed under, which the filing lookup reads alone.
    FiledUnder,
}

/// A task, a project or a document, so the copy path is written once.
#[derive(Clone)]
enum Item {
    /// A task.
    Task(Box<Task>),
    /// A project.
    Project(Box<Project>),
    /// A document.
    Document(Box<Document>),
}

impl Item {
    fn status(&self) -> Option<&Status> {
        match self {
            Self::Task(task) => Some(&task.status),
            Self::Project(project) => Some(&project.status),
            Self::Document(_) => None,
        }
    }

    fn id(&self) -> &NativeId {
        match self {
            Self::Task(task) => &task.id,
            Self::Project(project) => &project.id,
            Self::Document(document) => &document.id,
        }
    }

    fn level(&self) -> Level {
        match self {
            Self::Task(_) => Level::Task,
            Self::Project(_) => Level::Project,
            Self::Document(_) => Level::Document,
        }
    }
}

/// Which of a destination's three read-and-write interfaces one item belongs to.
///
/// Deliberately not [`ItemKind`]: that enum names what a *dependency endpoint* points at,
/// and the contract gives it no document variant because nothing may point at a document.
/// This one names which pair of methods reads and writes an item, which is a different
/// question with a third answer.
///
/// Ordered so it can key a map of what a destination holds, per interface: an id alone
/// does not identify a destination item, for the reason [`Undo::kind`] records.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(super) enum Level {
    /// `get_task`, `write_task`, `delete_task`.
    Task,
    /// `get_project`, `write_project`, `delete_project`.
    Project,
    /// `get_document`, `write_document`, `delete_document`.
    Document,
}

impl Level {
    /// The record a write at this level writes the content of.
    fn record(self) -> MetadataRecord {
        match self {
            Self::Task => MetadataRecord::Task,
            Self::Project => MetadataRecord::Project,
            Self::Document => MetadataRecord::Document,
        }
    }
}

/// One record filed under a document's own project, and the location string its source
/// reports for it.
///
/// A reference is a **literal occurrence, in a document's content, of the exact location
/// string the source reports for a related record** — the `String` inside
/// [`Location::Path`] or [`Location::Url`]. Both ends of a rewrite come from the plugins'
/// own reported [`Location`]: nothing here composes an address out of a name, an id or a
/// root, because a source that reports a canonical absolute path and a source that reports
/// an issue link are the two things the contract lets this ask about.
#[derive(Clone)]
struct Referent {
    /// Its qualified id at the source.
    id: GlobalId,
    /// The origin it records in its own metadata, when it records one.
    origin: Option<GlobalId>,
    /// Which of the destination's three interfaces its counterpart would be read from.
    level: Level,
    /// The non-empty location string its source reports for it.
    location: String,
    /// The well-formed entries of its own `onetaskgraph.copies`: the counterpart it records
    /// at each destination it was copied into.
    links: BTreeMap<SourceName, NativeId>,
    /// Where each of those counterparts is, by its own `onetaskgraph.copy_locations`, for the
    /// links that entry still stands beside.
    located: BTreeMap<SourceName, Location>,
    /// Its content at the source, whose headings a section link to it names.
    content: Option<String>,
    /// The ids its source stored those headings under, where it stores any.
    stored: Option<Vec<onetaskgraph_plugin_api::StoredHeading>>,
}

impl Referent {
    /// The two keys a destination record's own recorded origin is matched against.
    ///
    /// **A destination record is this referent's counterpart when its
    /// [`GlobalId::ORIGIN_KEY`] equals either this referent's own qualified source id, or
    /// the origin this referent itself records.** In plainer terms: when the destination
    /// record was copied directly from this referent, or directly from the one predecessor
    /// this referent itself records.
    ///
    /// That reach is exactly one recorded hop of ancestry on each side, and nothing more:
    ///
    /// - **A single hop resolves.** The destination record came straight from the referent.
    /// - **A one-level fan-out resolves.** A record copied from one store into two, with
    ///   the document arriving by one route and naming records that arrived by the other,
    ///   so both sides trace to one common predecessor. That is what the second key buys,
    ///   and it is the only thing it buys.
    /// - **A chain of two or more hops does not resolve**, on either side, and that is
    ///   permanent rather than pending: only one origin is ever recorded and every hop
    ///   overwrites it. [`carried`] removes the key from the incoming metadata outright and
    ///   [`recorded`] writes the id at the *immediate* source on every path but the
    ///   copy-back, so A → B → C leaves C keyed by B and A's id gone.
    ///
    /// The second key costs no read — the referent's metadata is already in hand. Chasing
    /// the chain further would need the intermediate stores configured and reachable, which
    /// would put a third party's availability inside a copy; a durable lineage id would
    /// identify only records written after it landed, so it would resolve nothing already
    /// on a destination.
    fn keys(&self) -> Vec<String> {
        let mut keys = vec![self.id.to_string()];
        if let Some(origin) = &self.origin {
            let recorded = origin.to_string();
            if recorded != keys[0] {
                keys.push(recorded);
            }
        }
        keys
    }

    /// The counterpart its own `onetaskgraph.copies` link names at `destination`, when it
    /// records one there.
    fn link(&self, destination: &SourceName) -> Option<NativeId> {
        self.links.get(destination).cloned()
    }
}

/// What every whole-reference occurrence of one location string becomes.
///
/// Three variants rather than a rewrite beside a flag, because the two ways of leaving an
/// occurrence alone are what the two figures a copy reports are *about*: one is the design
/// working and the other says the destination or the source holds something a re-run will
/// never fix. A bool would let a reader of this type read them as the same outcome.
enum Resolution {
    /// The counterpart's id at the destination, and the destination's own location string for
    /// it.
    Rewrite(NativeId, String),
    /// The destination holds no counterpart, or holds one it reports no location for, so
    /// the occurrence is left byte-for-byte. Ordinary and expected under the bound this
    /// design works to.
    NoCounterpart,
    /// The correspondence could not be established **confidently** — more than one
    /// destination record matches the referent's two keys, or two referents report this one
    /// location string. The occurrence is left byte-for-byte, no record is chosen, and a
    /// re-run will never clear it.
    Ambiguous,
}

/// Every destination record that records an origin, read **once per copy invocation**.
///
/// Several documents in one project is the ordinary case, and a walk per document would
/// multiply reads against a rate limiter for no gain — so this is built once, for the
/// levels the invocation's own documents really name, and every document and every
/// referent of that invocation is answered out of it. A copy whose documents hold no
/// candidate reference builds none at all.
///
/// **This is a stricter discipline than [`Engine::scan`], deliberately.** That lookup takes
/// the first hit and stops, and every consumer of the copy already depends on it doing so;
/// it chooses the copy's own target, where a caller named the item. This one edits the
/// content of somebody's document, where a wrong answer is silent corruption of prose a
/// person will act on — so where more than one record matches, it chooses none. The two
/// lookups answer different questions and are meant to disagree on a destination holding
/// duplicates.
#[derive(Default)]
struct Counterparts {
    /// The records at one interface recording one origin.
    by_origin: BTreeMap<(Level, String), Vec<Held>>,
}

/// One record a destination walk found: where it is there, and the location string that
/// destination reports for it — `None` when it reports none.
type Held = (NativeId, Option<String>);

impl Counterparts {
    /// Record one destination item, when it records an origin at all.
    fn note(
        &mut self,
        level: Level,
        id: &NativeId,
        location: Option<&Location>,
        metadata: &BTreeMap<String, Value>,
    ) {
        let Some(origin) = origin_of(metadata) else {
            return;
        };
        self.by_origin
            .entry((level, origin.to_string()))
            .or_default()
            .push((id.clone(), located(location)));
    }

    /// Record the counterpart a referent's own link names, at the location the referent recorded
    /// for it beside that link, without the destination being asked: the copy that wrote both
    /// wrote that record, so it is the one record the correspondence names, as a link confirmed
    /// by a read is.
    fn note_recorded(&mut self, referent: &Referent, id: &NativeId, location: &Location) {
        self.by_origin
            .entry((referent.level, referent.id.to_string()))
            .or_default()
            .push((id.clone(), located(Some(location))));
    }

    /// What one referent's occurrences become by its own qualified id alone — for a referent
    /// the copy itself writes, whose counterpart is the record copied from it.
    fn resolve_own(&self, referent: &Referent) -> Resolution {
        let own = Referent {
            origin: None,
            ..referent.clone()
        };
        self.resolve(&own)
    }

    /// What one referent's occurrences become, by the two-key rule.
    ///
    /// Where the correspondence cannot be established **confidently**, the text is left
    /// exactly as it is and no record is chosen — see the note on this type.
    fn resolve(&self, referent: &Referent) -> Resolution {
        let mut candidates: Vec<&Held> = Vec::new();
        for key in referent.keys() {
            for record in self
                .by_origin
                .get(&(referent.level, key))
                .into_iter()
                .flatten()
            {
                // One destination record matching both keys is one record, not two.
                if !candidates.iter().any(|held| held.0 == record.0) {
                    candidates.push(record);
                }
            }
        }
        match candidates.as_slice() {
            [] => Resolution::NoCounterpart,
            // A counterpart the destination reports no location for names nowhere a reader
            // could go, so the source's own string is left standing rather than removed.
            [(id, location)] => location
                .clone()
                .map_or(Resolution::NoCounterpart, |location| {
                    Resolution::Rewrite(id.clone(), location)
                }),
            _ => Resolution::Ambiguous,
        }
    }
}

impl Engine {
    /// Copy every item a request names into one configured destination.
    ///
    /// This is the whole of the verb, and the command line drives exactly this: a copy a
    /// Rust caller makes and a copy typed at a shell are the same call, so the two cannot
    /// answer the same copy differently.
    ///
    /// # Errors
    ///
    /// Returns [`EngineError`] when the destination is not configured, cannot be built,
    /// cannot be written, or — for a document copy — declares it has no documents; when an
    /// id names nothing; when an origin names an item the
    /// destination no longer holds and `--recreate` was not given; and when the
    /// destination refuses the write — including a field or a metadata key it cannot
    /// carry, which it names rather than dropping.
    pub async fn copy(&self, request: &CopyRequest) -> Result<CopyReport, EngineError> {
        let mut result = None;
        let mut operation: std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + '_>> =
            Box::pin(async { result = Some(self.copy_in_scope(request).await) });
        for source in self.ready() {
            operation = source.source().scope_visibility(operation);
        }
        operation.await;
        result.expect("the scoped operation completed")
    }

    async fn copy_in_scope(&self, request: &CopyRequest) -> Result<CopyReport, EngineError> {
        // Each destination's visibility is read once for the whole copy, and afresh by the next.
        let _held = self.hold_visibility();
        let _expecting = self.expecting_copy(request)?;
        // Before anything is read: `--create` says there is nothing to look for, and the other
        // two say how to look.
        if request.create {
            for (given, flag) in [
                (request.match_by.is_some(), CopyLookup::MatchBy),
                (request.recreate, CopyLookup::Recreate),
            ] {
                if given {
                    return Err(EngineError::CreateWith { flag });
                }
            }
        }
        if let Some(target) = &request.target_task {
            for (given, flag) in [
                (request.create, CopyLookup::Create),
                (request.match_by.is_some(), CopyLookup::MatchBy),
                (request.recreate, CopyLookup::Recreate),
            ] {
                if given {
                    return Err(EngineError::TargetTaskWith { flag });
                }
            }
            if request.scope != CopyScope::Tasks || request.items.as_slice().len() != 1 {
                return Err(EngineError::TargetTaskNeedsOne);
            }
            if target.source != request.destination {
                return Err(EngineError::TargetTaskSource {
                    target: target.clone(),
                    destination: request.destination.clone(),
                });
            }
        }
        if request.items.query().is_some() && request.scope != CopyScope::Tasks {
            return Err(EngineError::QueryNotTasks);
        }
        if request.comments && request.scope == CopyScope::Documents {
            return Err(EngineError::CommentsNotDocuments);
        }
        if !request.statuses.is_empty() && request.scope != CopyScope::Tasks {
            return Err(EngineError::StatusesNotTasks);
        }
        let conditions = &request.conditions;
        if (!conditions.status.is_empty()
            || conditions.otherwise.is_some()
            || !conditions.priority.is_empty()
            || conditions.assignees.is_some()
            || !conditions.versions.is_empty())
            && request.scope != CopyScope::Tasks
        {
            return Err(EngineError::ConditionsNotTasks);
        }
        if conditions.otherwise.is_some() && conditions.status.is_empty() {
            return Err(EngineError::OtherwiseWithoutStatus);
        }
        if !conditions.status.is_empty()
            && request.status_policy == StatusPolicy::PreserveDestination
        {
            return Err(EngineError::StatusConditionPreserved);
        }
        let destination = self.writable(&request.destination)?;
        // A guard on the destination's assignees names people a destination that has not opted
        // in holds none of: refused before anything is read.
        if conditions.assignees.is_some() {
            super::assignees::holds_assignees(destination, "each copied task")?;
        }
        // Before anything is read, and from the declaration rather than from a failed
        // write: a destination that says it has no documents has nowhere to put one.
        if request.scope == CopyScope::Documents {
            documentary(destination)?;
        }
        // And for the same reason, a `--comments` copy into a destination whose tasks hold no
        // comments — the copy's own, or any a route can send a task to.
        if request.comments {
            for name in std::iter::once(request.destination.clone())
                .chain(self.routes.reachable(&request.destination))
            {
                let landing = self.writable(&name)?;
                if !landing.source().capabilities().comments.is_native() {
                    return Err(EngineError::CommentlessDestination {
                        name: landing.name().to_string(),
                        kind: landing.kind().to_owned(),
                    });
                }
            }
        }
        let metered = self.metered(request, destination)?;
        let before = readings(&metered).await;
        let mut journal = Journal::default();
        let holding = self.hold_back(request);
        match self.copy_all(request, &mut journal).await {
            Ok((mut report, deliverers)) => {
                // After the whole copy is complete and outside the journal: what the relation
                // rule writes is the delivered tasks' own, and a failure there is reported
                // against that task rather than undoing a copy that landed.
                for deliverer in &deliverers {
                    report.delivered.extend(
                        self.deliver(
                            &deliverer.destination,
                            deliverer.classification,
                            deliverer.category,
                            &deliverer.now,
                            &deliverer.before,
                        )
                        .await,
                    );
                }
                report.spent = spent_between(&before, &readings(&metered).await);
                Ok(report)
            }
            Err(error) => {
                let error = self.undo(journal, error).await;
                // Nothing a copy that failed created is left to send — its undo took each one
                // back — so this ends the holding back without writing anything.
                for destination in &holding {
                    let _ = destination.source().finish_writes().await;
                }
                Err(error)
            }
        }
    }

    /// The sources `request` names — `destination`, every source its routes reach and every
    /// source it reads from — each read once before and once after, so what the command spent
    /// is the difference between two of each one's own running totals.
    fn metered<'a>(
        &'a self,
        request: &CopyRequest,
        destination: &'a ResolvedSource,
    ) -> Result<Vec<&'a ResolvedSource>, EngineError> {
        let mut metered = vec![destination];
        for routed in self.routes.reachable(&request.destination) {
            if let Some(source) = self.ready().find(|source| source.name() == &routed)
                && !metered.iter().any(|held| held.name() == source.name())
            {
                metered.push(source);
            }
        }
        let mut reading: Vec<SourceName> = request
            .items
            .as_slice()
            .iter()
            .map(|id| id.source.clone())
            .collect();
        if let Some(query) = request.items.query() {
            reading.extend(self.resolve_selection(&query.sources)?);
        }
        for name in reading {
            if let Some(source) = self.ready().find(|source| source.name() == &name)
                && !metered.iter().any(|held| held.name() == source.name())
            {
                metered.push(source);
            }
        }
        Ok(metered)
    }

    /// Every destination `request` can write, each told it may hold back the parts of the
    /// items it creates until [`Self::finished_writes`] — so a plan copy's field values and
    /// edges go to a board together rather than one item at a time.
    ///
    /// A dry run writes nothing, and a copy expecting a version of its target records each write
    /// whole for the store's cache, so neither holds anything back.
    fn hold_back(&self, request: &CopyRequest) -> Vec<&ResolvedSource> {
        if request.dry_run || self.expects_copy() || self.holds_copy() {
            return Vec::new();
        }
        let mut holding: Vec<&ResolvedSource> = Vec::new();
        for name in std::iter::once(request.destination.clone())
            .chain(self.routes.reachable(&request.destination))
        {
            if let Ok(destination) = self.writable(&name)
                && !holding.iter().any(|held| held.name() == destination.name())
            {
                destination.source().begin_deferred_writes();
                holding.push(destination);
            }
        }
        holding
    }

    /// Have every destination `request` can write send what it held back, before anything is
    /// linked to or reported as landing there.
    async fn finished_writes(
        &self,
        request: &CopyRequest,
        journal: &mut Journal,
    ) -> Result<(), EngineError> {
        if request.dry_run || self.expects_copy() || self.holds_copy() {
            return Ok(());
        }
        let mut finished: Vec<SourceName> = Vec::new();
        for name in std::iter::once(request.destination.clone())
            .chain(self.routes.reachable(&request.destination))
        {
            if finished.contains(&name) {
                continue;
            }
            if let Ok(destination) = self.writable(&name) {
                // What was held back includes the writes of creates this copy owns.
                let owned = Self::owned_at(journal, None, &name);
                self.guarded(owned, journal, destination.source().finish_writes())
                    .await?
                    .map_err(|error| refused(destination, error))?;
            }
            finished.push(name);
        }
        Ok(())
    }

    /// The copy itself, with everything it writes recorded so a failure can be undone.
    ///
    /// The ids named together are **one** copied set, and that is what makes an edge
    /// between any two of them a real edge at the destination: a copy of two projects at
    /// once knows that a task in the first depends on a task in the second, and a task
    /// knows that the project it belongs to is being created beside it. Copying them one
    /// at a time could not, and wrote the far end as the id it had at its *source* — a
    /// dangling reference to somewhere the destination has never heard of.
    async fn copy_all(
        &self,
        request: &CopyRequest,
        journal: &mut Journal,
    ) -> Result<(CopyReport, Vec<Deliverer>), EngineError> {
        let mut running = Running {
            carries_work: request.scope != CopyScope::Documents,
            ..Running::default()
        };
        // The whole copied set, established before anything is written. For a project
        // copy that means reading every named project's membership first: the set is the
        // whole request rather than one project of it.
        let items = match &request.scope {
            CopyScope::Tasks | CopyScope::Documents => {
                let kind = if request.scope == CopyScope::Documents {
                    Level::Document
                } else {
                    Level::Task
                };
                let slots = match request.items.query() {
                    Some(query) => {
                        let mut query = query.clone();
                        if !request.statuses.is_empty() {
                            query.filters.statuses.clone_from(&request.statuses);
                        }
                        self.candidates(request, &query, &mut running).await?
                    }
                    None if !request.statuses.is_empty() => {
                        self.at_statuses(request.items.as_slice(), &request.statuses)
                            .await?
                    }
                    None => request
                        .items
                        .as_slice()
                        .iter()
                        .cloned()
                        .map(Slot::Read)
                        .collect(),
                };
                let since = request
                    .items
                    .query()
                    .and_then(|query| query.commented_since);
                let ids: Vec<GlobalId> = slots
                    .iter()
                    .filter_map(|slot| match slot {
                        Slot::Read(id) => Some(id.clone()),
                        Slot::Done(_) => None,
                    })
                    .collect();
                running.resolvable.extend(ids.iter().cloned());
                self.prefetch(request, &ids, &mut running).await;
                let mut placed: Vec<Option<CopyOutcome>> = Vec::new();
                let mut reads = Vec::new();
                for slot in slots {
                    let id = match slot {
                        Slot::Done(outcome) => {
                            placed.push(Some(*outcome));
                            continue;
                        }
                        Slot::Read(id) => id,
                    };
                    let read = match self.read_for_copy(request, kind, &id, &mut running).await {
                        Ok(read) => read,
                        Err(error @ EngineError::ChangedWhileCopied { .. }) => {
                            placed.push(Some(failed(id, &error)));
                            continue;
                        }
                        Err(error) => return Err(error),
                    };
                    // A query's comment activity, confirmed from the comments just read rather
                    // than by a read of its own: a task without it is not one the query matches.
                    if let Some(since) = since {
                        let query = TaskQuery {
                            commented_since: Some(since),
                            ..TaskQuery::default()
                        };
                        if !query.comments_match(read.comments.iter().flatten()) {
                            continue;
                        }
                    }
                    reads.push(read);
                    placed.push(None);
                }
                // Every item is read before any is aimed, so the destination is read for all of
                // their counterparts together rather than one request per item.
                self.read_ahead(request, &reads, &mut running).await;
                let mut planned = Vec::new();
                for read in reads {
                    planned.push(self.plan_read(request, read, &mut running).await?);
                }
                let mut landed = self
                    .copy_items(request, planned, None, &mut running, journal)
                    .await?
                    .into_iter();
                placed
                    .into_iter()
                    .map(|slot| {
                        slot.or_else(|| landed.next())
                            .expect("every item planned is reported once")
                    })
                    .collect()
            }
            CopyScope::Projects { tasks, documents } => {
                // A destination with no documents has nowhere to put the plan's, so it is
                // refused before anything of the plan is read.
                if *documents {
                    documentary(self.writable(&request.destination)?)?;
                }
                let mut projects = Vec::new();
                for id in request.items.as_slice() {
                    let members = if *tasks {
                        self.project_members(id).await?
                    } else {
                        Vec::new()
                    };
                    if *documents {
                        let filed: Vec<GlobalId> = self
                            .project_documents(id)
                            .await?
                            .into_iter()
                            .map(|document| document.id)
                            .collect();
                        running.resolvable.extend(filed.iter().cloned());
                        running.plan_documents.insert(id.clone(), filed);
                    }
                    running.resolvable.push(id.clone());
                    running.resolvable.extend(members.iter().cloned());
                    projects.push((id.clone(), members));
                }
                self.copy_projects(request, &projects, &[], &mut running, journal)
                    .await?
            }
            CopyScope::Members(named) => {
                let (projects, unrecorded) =
                    self.named_members(request, named, &mut running).await?;
                self.copy_projects(request, &projects, &unrecorded, &mut running, journal)
                    .await?
            }
            CopyScope::MembersOnly(named) => {
                let (projects, unrecorded) =
                    self.named_members(request, named, &mut running).await?;
                self.copy_into_projects(request, &projects, &unrecorded, &mut running, journal)
                    .await?
            }
        };
        let references = running.references;
        let delivers_rewritten = running.delivers_rewritten;
        // Every member's destination id is known once the first pass has landed it, so the
        // tasks each landed task delivers are settled here rather than after the repair.
        let deliverers: Vec<Deliverer> = running
            .landed
            .iter()
            .map(|task| Deliverer {
                destination: task.destination.clone(),
                classification: task.classification,
                category: task.category,
                now: targets(
                    &resolved_entries(&mapped_delivers(
                        &task.delivers,
                        &task.origin,
                        &task.destination.source,
                        &running.resolvable,
                        &running.counterparts,
                    )),
                    &task.destination.source,
                ),
                before: task.before.clone(),
            })
            .collect();
        let linking = std::mem::take(&mut running.linking);
        // Which interface each landed item is read through, for what it holds once written.
        let levels: HashMap<String, Level> = linking
            .iter()
            .map(|entry| (entry.destination.to_string(), entry.level))
            .collect();
        let homes = std::mem::take(&mut running.homes);
        let sizes = std::mem::take(&mut running.sizes);
        let recordings = self.recordings(&running);
        // The repair writes an item's content again, so what the destination stores after it is
        // read off its answer exactly as the first write's was.
        self.repair(request, running, journal).await?;
        if !request.dry_run {
            self.record_dependents(recordings, journal).await?;
            self.record_members(homes, journal).await?;
        }
        // Everything held back lands before anything is linked to it: a copy that cannot finish
        // is undone with nothing recorded at its sources.
        self.finished_writes(request, journal).await?;
        let mut items = items;
        // A dry run says, of every item it sized, how large a body it would write — and, of one
        // larger than its destination holds, that the copy itself would refuse it.
        if request.dry_run {
            for outcome in &mut items {
                let Some(size) = sizes.get(&outcome.source) else {
                    continue;
                };
                outcome.body_size = Some(size.size);
                if let (true, Some(limit)) = (size.exceeds(), size.limit) {
                    outcome.action = CopyAction::Oversize {
                        size: size.size,
                        limit,
                    };
                }
            }
        }
        if !request.dry_run {
            intent::stop_at("before-link");
        }
        self.link(linking, &mut items, request.no_link, journal)
            .await?;
        // Each item whose destination stores other content than it was sent is written once
        // more, now that its source's link is written and so its change token is final; one the
        // destination then stores otherwise again is reported `failed` with why.
        self.finish_sections(journal).await?;
        let diverged = self.follow_up(journal).await?;
        self.written_results(&mut items, &levels).await;
        for outcome in &mut items {
            let mut figures = journal
                .sections
                .get(&outcome.source)
                .copied()
                .unwrap_or_default();
            if let Some(comments) = journal.comment_sections.get(&outcome.source) {
                figures.section_links_unresolved += comments.unresolved;
                figures.section_links_unaddressable += comments.unaddressable;
            }
            outcome.sections = figures;
        }
        for (source, error) in &diverged {
            for outcome in items.iter_mut().filter(|outcome| &outcome.source == source) {
                outcome.action = CopyAction::Failed {
                    error: Failure::from(error),
                };
            }
        }
        Ok((
            CopyReport {
                items,
                references_rewritten: references.rewritten,
                references_unresolved: references.unresolved,
                references_ambiguous: references.ambiguous,
                delivers_rewritten,
                // Filled in by `copy`, once the copy is complete.
                delivered: Vec::new(),
                // Filled in by `copy`, which is the one place both readings are taken.
                spent: None,
            },
            deliverers,
        ))
    }

    /// Bring each written item's `result` to what its destination's own answers say it holds
    /// now that every write of the copy is done — its token above all, which a later write of
    /// the same item (its edges, its comments, a follow-up) moves. A destination that cannot say
    /// keeps what the copy sent, and no version.
    async fn written_results(&self, items: &mut [CopyOutcome], levels: &HashMap<String, Level>) {
        for outcome in items.iter_mut() {
            if !matches!(
                outcome.action,
                CopyAction::Created { .. } | CopyAction::Updated { .. }
            ) {
                continue;
            }
            let Some(landed) = outcome.destination().cloned() else {
                continue;
            };
            let (Some(level), Ok(destination)) = (
                levels.get(&landed.to_string()),
                self.writable(&landed.source),
            ) else {
                continue;
            };
            let answered = match level {
                Level::Task => destination.source().written_task(&landed.native).await,
                Level::Project | Level::Document => None,
            };
            let version = match &answered {
                Some(task) => task.version.clone(),
                None => {
                    destination
                        .source()
                        .written_version(level.record(), &landed.native)
                        .await
                }
            };
            if let Some(result) = outcome.expected.result.as_mut() {
                if let Some(task) = answered {
                    result.status = Some(task.status);
                    result.priority = Some(task.priority);
                }
                result.version = version.or_else(|| result.version.take());
            }
        }
    }

    /// Run `call` — one call to a destination plugin — answering, before each write request the
    /// plugin sends inside it, whether it may send it: every create `owned` names has its lease
    /// renewed first, and once one cannot be, the answer is a refusal to that request and to
    /// every later one, a write taking back what the call made included. Answered `Err` with
    /// the renewal's own refusal, the journal marked when it is another attempt's hold, once
    /// the call has stopped; `Ok` with the call's own answer otherwise. With nothing owned the
    /// call runs as it is.
    ///
    /// The guard is served on this task beside the call, so a renewal is a write to the copy's
    /// source made between two of the destination's requests — and through no guard of its own.
    async fn guarded<T>(
        &self,
        owned: Vec<(Level, GlobalId, SourceName)>,
        journal: &mut Journal,
        call: impl std::future::Future<Output = T>,
    ) -> Result<T, EngineError> {
        if owned.is_empty() {
            return Ok(call.await);
        }
        type Ask = tokio::sync::oneshot::Sender<Result<(), SourceError>>;
        struct Asking(tokio::sync::mpsc::UnboundedSender<Ask>);
        #[async_trait::async_trait]
        impl onetaskgraph_plugin_api::WriteGuard for Asking {
            async fn before_write(&self) -> Result<(), SourceError> {
                let (answer, answered) = tokio::sync::oneshot::channel();
                if self.0.send(answer).is_err() {
                    return Ok(());
                }
                answered.await.unwrap_or(Ok(()))
            }
        }
        let (asks, mut asked) = tokio::sync::mpsc::unbounded_channel::<Ask>();
        let mut refusal: Option<EngineError> = None;
        let serve = async {
            let mut asks = 0_usize;
            while let Some(answer) = asked.recv().await {
                intent::pause_at("guarded-write");
                intent::pause_at(&format!("guarded-write-{asks}"));
                asks += 1;
                if refusal.is_none() {
                    for (level, item, to) in &owned {
                        if let Err(error) = self.renew_intent(*level, item, to).await {
                            refusal = Some(error);
                            break;
                        }
                    }
                }
                let _ = answer.send(match &refusal {
                    None => Ok(()),
                    Some(error) => Err(SourceError::Refused {
                        message: error.to_string(),
                    }),
                });
            }
        };
        let guard: std::sync::Arc<dyn onetaskgraph_plugin_api::WriteGuard> =
            std::sync::Arc::new(Asking(asks));
        let (answer, ()) = tokio::join!(
            <dyn onetaskgraph_plugin_api::WriteGuard>::scope(guard, call),
            serve
        );
        match refusal {
            None => Ok(answer),
            Some(error) => {
                if matches!(error, EngineError::CreateInProgress { .. }) {
                    journal.ownership_lost = true;
                }
                Err(error)
            }
        }
    }

    /// Every create this copy owns, of `item` when it names one, at `destination`.
    fn owned_at(
        journal: &Journal,
        item: Option<&GlobalId>,
        destination: &SourceName,
    ) -> Vec<(Level, GlobalId, SourceName)> {
        journal
            .owned
            .iter()
            .filter(|(_, owner, to)| to == destination && item.is_none_or(|item| owner == item))
            .cloned()
            .collect()
    }

    /// Renew the lease of the intent `source` holds for `destination`, when this copy owns that
    /// create — before a destination write it sends after its create request — and stop, marking
    /// the journal, once another attempt holds it.
    async fn renew_owned(
        &self,
        source: &GlobalId,
        destination: &SourceName,
        journal: &mut Journal,
    ) -> Result<(), EngineError> {
        let Some(level) = journal
            .owned
            .iter()
            .find(|(_, item, to)| item == source && to == destination)
            .map(|(level, ..)| *level)
        else {
            return Ok(());
        };
        let renewed = self.renew_intent(level, source, destination).await;
        if matches!(renewed, Err(EngineError::CreateInProgress { .. })) {
            journal.ownership_lost = true;
        }
        renewed
    }

    /// Record the intent to create `item`'s counterpart on the item it is copied from, when the
    /// copy creates one and may write to that item — taking over an earlier attempt's when its
    /// lease has run out, and refusing, before anything is written, when it has not.
    async fn intend(&self, item: &mut Planned) -> Result<bool, EngineError> {
        // A create, and an earlier attempt's create this copy found by its intent and finishes:
        // either way the copy holds the intent while it writes.
        let found_by_intent = matches!(
            item.target,
            Target::Update {
                found: Found::Intent,
                ..
            }
        );
        if !matches!(item.target, Target::Create) && !found_by_intent {
            return Ok(false);
        }
        if item.skipped || item.withheld {
            return Ok(false);
        }
        let Ok(source) = self.writable(&item.source.source) else {
            return Ok(false);
        };
        if !source.source().writes().is_supported()
            || (self.declared(&item.to) == crate::config::SourceVisibility::Private
                && self.declared(source.name()) != crate::config::SourceVisibility::Private)
        {
            return Ok(false);
        }
        // Held to the boundary as the link written at the end is: an intent is a write to the
        // item copied, and one its source may not take refuses the copy before anything else.
        self.at_write(
            source,
            &item.source.to_string(),
            &WriteTarget::Existing(&item.source.native),
        )
        .await?;
        let level = item.item.level();
        let (_, metadata) = described(&item.item);
        let earlier = intent::held(metadata, &item.to);
        let replaces = link_of(metadata, &item.to);
        let key = match &earlier {
            Ok(Some(earlier)) if found_by_intent => earlier.key,
            _ => intent::key(level, &item.source, &item.to, replaces.as_ref()),
        };
        // A source that will not keep the key — a file it cannot edit narrowly, an item it no
        // longer answers for, a write it drops — copies as it always did, with no intent, as it
        // then records no link either. Anything else refuses the copy before a destination
        // write: another attempt's live lease, a hold the source could not give — another
        // process's, or one it cannot judge — a read of the item that failed, and an intent there
        // that is not one, since copying on without the intent would leave the create
        // unprotected while another may be running.
        let (taken, local) = match self
            .take_intent(
                level,
                &item.source,
                &item.to,
                key,
                None,
                replaces
                    .as_ref()
                    .map_or(intent::PlannedLink::Absent, intent::PlannedLink::Replacing),
            )
            .await?
        {
            intent::Taken::Held { intent, exclusive } => (intent, exclusive),
            intent::Taken::Unkept => return Ok(false),
        };
        let destination = self.writable(&item.to)?;
        item.relist =
            destination.source().create_key_lags() && (!local || matches!(earlier, Ok(Some(_))));
        item.intent = Some(taken);
        Ok(true)
    }

    /// Write every deferred item again, now that every destination id is known.
    ///
    /// This is the second half of the two passes an edge between two items of one copy
    /// needs: the far end's destination id does not exist until it has been created, so
    /// the item that points at it lands first without that edge and is completed here.
    /// It runs once for the whole request rather than once per project, because a far end
    /// may be in a project this copy has not reached yet.
    async fn repair(
        &self,
        request: &CopyRequest,
        running: Running,
        journal: &mut Journal,
    ) -> Result<(), EngineError> {
        if request.dry_run {
            return Ok(());
        }
        let Running {
            resolvable,
            counterparts,
            deferred,
            private,
            ..
        } = running;
        for entry in deferred {
            let destination = self.writable(&entry.item.to)?;
            let edges = self.shown_edges(
                &entry.item.to,
                &entry.item.source,
                mapped_edges(
                    &entry.item.edges,
                    &entry.item.source.source,
                    &entry.item.to,
                    &resolvable,
                    &counterparts,
                ),
                &private,
            );
            let delivers = self.shown_delivers(
                &entry.item.to,
                delivers_of(&entry.item, &entry.item.to, &resolvable, &counterparts),
                &private,
            );
            self.write(
                destination,
                &entry.item,
                Some(entry.destination),
                entry.filed,
                &resolved(&edges),
                &resolved_entries(&delivers),
                entry.prior,
                Stamping::of(request, false),
                journal,
            )
            .await?;
        }
        Ok(())
    }

    /// Put the destination back the way this copy found it, then report why it failed.
    ///
    /// Undone in reverse, and an item this copy created is removed rather than restored —
    /// the entry recording what it looked like a moment after creation is not a state
    /// anybody asked for. When the destination cannot take one of them back, the refusal
    /// says so and names what is still there, because a user told "the copy failed" about
    /// a destination that is not as they left it will copy again over a tree nobody
    /// described.
    async fn undo(&self, mut journal: Journal, error: EngineError) -> EngineError {
        if journal.ownership_lost {
            return error;
        }
        // A copy that takes every write back made nothing, so its intents are taken back with
        // its links, byte for byte; one whose writes may have landed unseen — an answer that
        // never arrived — keeps each intent for its retry to resolve, giving up only the lease.
        let intents = std::mem::take(&mut journal.intents);
        // A create a source refused left nothing (§4.9); one that got no answer at all may have.
        let unanswered = Failure::from(&error).class() == crate::FailureClass::Transient;
        let sending = std::mem::take(&mut journal.sending);
        let error = self.undo_writes(journal, error).await;
        let uncertain = matches!(error, EngineError::CopyDestinationUncertain { .. });
        let not_undone = matches!(error, EngineError::CopyNotUndone { .. });
        for (level, item, destination, snapshot, intent) in intents {
            // Kept, its lease given up, wherever its create may still stand: a create whose
            // answer never arrived, or one a copy could not take back.
            let keep = uncertain
                || ((not_undone || (unanswered && sending.contains(&item)))
                    && self
                        .may_have_landed(level, &destination, intent.as_ref())
                        .await);
            let _ = if keep {
                self.release_lease(level, &item, &destination).await
            } else if let Some(bytes) = snapshot
                && self.restore_snapshot(level, &item, &bytes).is_ok()
            {
                Ok(())
            } else {
                // Restored byte for byte where it can be, and otherwise its intent alone taken
                // off; an item that takes neither keeps the intent, which its retry resolves once
                // the lease has run out.
                self.drop_intent(level, &item, &destination).await
            };
        }
        error
    }

    /// Put back every write `journal` recorded, answering the error the copy then fails with.
    async fn undo_writes(&self, journal: Journal, error: EngineError) -> EngineError {
        let created: Vec<(&SourceName, Level, &NativeId)> = journal
            .entries
            .iter()
            .filter_map(|entry| match entry {
                Undo::Created { at, kind, id } => Some((at, *kind, id)),
                Undo::Updated { .. } | Undo::Allocated { .. } => None,
            })
            .collect();
        // The ids and the refusal are one value rather than two, because they are one
        // fact: an item is only left behind because the destination refused to take it
        // back, so the first refusal carries the first id and neither half can be
        // recorded without the other.
        let mut unrestored: Option<(LeftBehind, SourceError)> = None;
        // The links first, because they were written last: once the whole copy had landed.
        for entry in journal.links.iter().rev() {
            if let Err(problem) = self.unlink(entry).await {
                match &mut unrestored {
                    Some((left_behind, _)) => left_behind.push(entry.item.clone()),
                    None => unrestored = Some((LeftBehind::new(entry.item.clone()), problem)),
                }
            }
        }
        // The comments next, in reverse: each was written after the item it is on, and before
        // the links.
        for entry in journal.comments.iter().rev() {
            let outcome = match self.writable(&entry.at) {
                Err(failed) => Err(SourceError::Refused {
                    message: failed.to_string(),
                }),
                Ok(destination) => match &entry.undo {
                    CommentUndoing::Added(id) => destination
                        .source()
                        .delete_comment(&entry.task, id)
                        .await
                        .map(|_| ()),
                    CommentUndoing::Edited { id, before } => destination
                        .source()
                        .write_comment_copy(&entry.task, Some(id), before)
                        .await
                        .map(|_| ()),
                    CommentUndoing::Deleted(before) => destination
                        .source()
                        .write_comment_copy(&entry.task, None, before)
                        .await
                        .map(|_| ()),
                },
            };
            if let Err(problem) = outcome {
                let id = GlobalId::new(entry.at.clone(), entry.task.clone());
                match &mut unrestored {
                    Some((left_behind, _)) => left_behind.push(id),
                    None => unrestored = Some((LeftBehind::new(id), problem)),
                }
            }
        }
        // A private home whose map records the allocation of a public projection project that
        // could not be taken back: left as it is, so the next copy finds that project by its
        // allocation rather than making another, and named among what is still there.
        let mut kept: Vec<GlobalId> = Vec::new();
        for entry in journal.entries.iter().rev() {
            let item = match entry {
                Undo::Created { at, id, .. } | Undo::Updated { at, id, .. } => {
                    GlobalId::new(at.clone(), id.clone())
                }
                Undo::Allocated { home, .. } => home.clone(),
            };
            if entry.kind() == Level::Project && kept.contains(&item) {
                continue;
            }
            // A write held to a version that its check refused sent nothing, so there is
            // nothing of it to put back — and putting it back would itself move the version.
            if matches!(entry, Undo::Updated { .. })
                && self.hold_refused(&item.source, &item.native)
            {
                continue;
            }
            let outcome = match self.writable(entry.at()) {
                Err(failed) => Err(SourceError::Refused {
                    message: failed.to_string(),
                }),
                Ok(destination) => match entry {
                    Undo::Created { kind, id, .. } => remove(destination, *kind, id).await,
                    Undo::Updated { at, id, prior }
                        if !created.contains(&(at, prior.item.level(), id)) =>
                    {
                        restore(destination, id, prior).await
                    }
                    Undo::Updated { .. } => Ok(()),
                    Undo::Allocated {
                        allocation, home, ..
                    } => match self.take_back_allocated(destination, allocation).await {
                        Ok(()) => Ok(()),
                        Err((still, problem)) => {
                            for id in still {
                                let id = GlobalId::new(entry.at().clone(), id);
                                match &mut unrestored {
                                    Some((left_behind, _)) => left_behind.push(id),
                                    None => {
                                        unrestored = Some((LeftBehind::new(id), problem.clone()));
                                    }
                                }
                            }
                            kept.push(home.clone());
                            match &mut unrestored {
                                Some((left_behind, _)) => {
                                    left_behind.push(home.clone());
                                    Ok(())
                                }
                                None => Err(problem),
                            }
                        }
                    },
                },
            };
            if let Err(problem) = outcome {
                match &mut unrestored {
                    Some((left_behind, _)) => left_behind.push(item),
                    None => unrestored = Some((LeftBehind::new(item), problem)),
                }
            }
        }
        match unrestored {
            None => error,
            Some((left_behind, refusal)) => EngineError::CopyNotUndone {
                error: Box::new(error),
                left_behind,
                refusal,
            },
        }
    }

    /// Record, on every item this copy landed, where it landed — once the whole copy has
    /// landed, so a copy that fails before this point has written nothing on any source.
    ///
    /// Each outcome is told what happened to its item's link. The items landed are exactly
    /// the outcomes that are not orphans, in the order they landed; a dry run records
    /// nothing, so this is never asked about one.
    async fn link(
        &self,
        linking: Vec<Linking>,
        items: &mut [CopyOutcome],
        no_link: bool,
        journal: &mut Journal,
    ) -> Result<(), EngineError> {
        for entry in linking {
            let slot = items
                .iter_mut()
                .filter(|outcome| outcome.action.landed() && outcome.action.link().is_none())
                .find(|outcome| outcome.source == entry.item)
                .and_then(|outcome| outcome.action.link_slot())
                .expect("every item a copy landed is reported");
            *slot = Some(if no_link {
                // `--no-link`: nothing at all is written to the source.
                CopyLink::Skipped
            } else {
                self.linked(entry, journal).await?
            });
        }
        Ok(())
    }

    /// Write one landed item's `onetaskgraph.copies` entry for the destination, when it
    /// does not already say this, and say what was done.
    ///
    /// An item found by its own origin records nothing: that correspondence is already
    /// recorded, on the destination item. A source that cannot hold the entry — it has no
    /// write side, or refuses the narrow metadata write — leaves the copy as it landed and
    /// is reported so; any other failure fails the copy, which then undoes every write it
    /// made, this one's journal entry included.
    async fn linked(&self, entry: Linking, journal: &mut Journal) -> Result<CopyLink, EngineError> {
        // Keyed by the source the item really landed in, which a routed copy decides per item.
        let landed_in = entry.destination.source.clone();
        if entry.found == Some(Found::Origin) {
            return Ok(CopyLink::Unchanged);
        }
        let wanted = Value::String(entry.destination.to_string());
        // Only the entries that are links are kept. Anything else under the key — a value
        // that is not an object, or an entry a person spelled wrong — is not a link this or
        // any later copy can read, so it is replaced by what is, exactly as an entry naming
        // another item is rewritten.
        let mut links = well_formed(entry.held.as_ref());
        let link_held = links.get(landed_in.as_str()) == Some(&wanted);
        let mut locations = located_entries(entry.held_locations.as_ref());
        let location = entry
            .location
            .as_ref()
            .and_then(|location| serde_json::to_value(location).ok())
            .map(|location| serde_json::json!({"id": wanted, "location": location}));
        let location =
            location.filter(|location| locations.get(landed_in.as_str()) != Some(location));
        if link_held && location.is_none() && !entry.intent {
            return Ok(CopyLink::Unchanged);
        }
        let source = self.readable(&entry.item.source)?;
        if !source.source().writes().is_supported() {
            return Ok(if link_held {
                CopyLink::Unchanged
            } else {
                CopyLink::Unrecorded
            });
        }
        // A link naming a private destination is not written where it would name it to
        // anybody who may read the source.
        if self.declared(&landed_in) == crate::config::SourceVisibility::Private
            && self.declared(source.name()) != crate::config::SourceVisibility::Private
        {
            return Ok(if link_held {
                CopyLink::Unchanged
            } else {
                CopyLink::Unrecorded
            });
        }
        self.at_write(
            source,
            &entry.item.to_string(),
            &WriteTarget::Existing(&entry.item.native),
        )
        .await?;
        let recorded = if link_held && !entry.intent {
            CopyLink::Unchanged
        } else {
            links.insert(landed_in.to_string(), wanted);
            // Journalled before the write, for the reason a destination write is: a write that
            // stopped part way is only put back by what this journal holds.
            journal.links.push(Unlink {
                item: entry.item.clone(),
                level: entry.level,
                key: MetadataKey::copies(),
                before: entry.held,
            });
            // An item recording an intent to create here has it removed in the same write.
            let written = if entry.intent {
                self.link_settling_intent(
                    entry.level,
                    &entry.item,
                    &landed_in,
                    Value::Object(links),
                )
                .await
            } else {
                set_link_versioned(
                    source,
                    entry.level,
                    &entry.item.native,
                    &MetadataKey::copies(),
                    &Value::Object(links),
                )
                .await
            };
            match written {
                Ok(Some(version)) => {
                    if let Some(version) = version {
                        journal.linked_versions.insert(entry.item.clone(), version);
                    }
                    // A write made only to take an intent off leaves the link as it was.
                    if link_held {
                        CopyLink::Unchanged
                    } else {
                        CopyLink::Recorded
                    }
                }
                // Nothing was written: the source no longer holds the item, or said it cannot
                // hold the key.
                Ok(None) | Err(SourceError::Refused { .. }) => {
                    journal.links.pop();
                    return Ok(CopyLink::Unrecorded);
                }
                // §4.18 once let a plugin refuse any key in this namespace as malformed, and one
                // written then still may. Its answer says nothing about whether it wrote, so the
                // journal entry stays and a later failure puts the item back regardless.
                Err(SourceError::Malformed { .. }) => return Ok(CopyLink::Unrecorded),
                Err(error) => return Err(refused(source, error)),
            }
        };
        // Where the counterpart is, beside the link that names it, so a later document copy can
        // point a reference at it without reading it. Nothing depends on it landing: a source
        // that will not hold it — a plugin built before the key existed refuses it as malformed
        // — leaves the link as it was recorded, and a reader without it reads as it always did.
        if let Some(location) = location {
            locations.insert(landed_in.to_string(), location);
            journal.links.push(Unlink {
                item: entry.item.clone(),
                level: entry.level,
                key: MetadataKey::copy_locations(),
                before: entry.held_locations,
            });
            match set_link_versioned(
                source,
                entry.level,
                &entry.item.native,
                &MetadataKey::copy_locations(),
                &Value::Object(locations),
            )
            .await
            {
                Ok(Some(version)) => {
                    if let Some(version) = version {
                        journal.linked_versions.insert(entry.item.clone(), version);
                    }
                }
                Err(SourceError::Malformed { .. }) => {}
                Ok(None) | Err(SourceError::Refused { .. }) => {
                    journal.links.pop();
                }
                Err(error) => return Err(refused(source, error)),
            }
        }
        Ok(recorded)
    }

    /// The `level` record `id` written back whole without `key`, for a source that cannot take
    /// one key off on its own — answered with its version token after the write, or `None` when
    /// the source holds no such record.
    async fn without_key(
        &self,
        source: &ResolvedSource,
        level: Level,
        id: &NativeId,
        key: &MetadataKey,
    ) -> Result<Option<Option<String>>, SourceError> {
        let Some(mut held) = self
            .prior(source, level, id)
            .await
            .map_err(|error| match error {
                EngineError::SourceRefused { error, .. } => error,
                other => SourceError::Refused {
                    message: other.to_string(),
                },
            })?
        else {
            return Ok(None);
        };
        if metadata_of(&mut held.item).remove(key.as_str()).is_some() {
            restore(source, id, &held).await?;
        }
        Ok(Some(
            source.source().written_version(level.record(), id).await,
        ))
    }

    /// Put one copied item's `onetaskgraph.copies`, or its `onetaskgraph.copy_locations`, back
    /// to what it held before this copy.
    ///
    /// A key it held is written back through the same narrow write. A key it did not hold
    /// cannot be removed through that write, so the item is read as it now is and written
    /// back whole, without the key, through the source's own write interface — the same
    /// way an overwritten destination item is restored.
    async fn unlink(&self, entry: &Unlink) -> Result<(), SourceError> {
        let as_source_error = |error: EngineError| match error {
            EngineError::SourceRefused { error, .. } => error,
            other => SourceError::Refused {
                message: other.to_string(),
            },
        };
        let source = self.readable(&entry.item.source).map_err(as_source_error)?;
        if let Some(before) = &entry.before {
            return set_link(source, entry.level, &entry.item.native, &entry.key, before)
                .await
                .map(|_| ());
        }
        let Some(mut held) = self
            .prior(source, entry.level, &entry.item.native)
            .await
            .map_err(as_source_error)?
        else {
            return Ok(());
        };
        metadata_of(&mut held.item).remove(entry.key.as_str());
        restore(source, &entry.item.native, &held).await
    }

    /// The member project `home` names in `to`, created and recorded on the home when it
    /// names none — for a write that is not a copy, such as `task create`, routed away from
    /// the source its project is in. `None` when the home itself is not there.
    ///
    /// What it wrote is handed back, so a write that then fails takes it back through
    /// [`Engine::unfile`] and leaves no member nobody asked for.
    pub(super) async fn member_project(
        &self,
        home: &GlobalId,
        to: &SourceName,
    ) -> Result<Filed, EngineError> {
        let mut running = Running::default();
        let mut journal = Journal::default();
        let mut filed = self
            .member_in(false, home, to, None, &mut running, &mut journal)
            .await;
        if filed.is_ok() {
            let homes = std::mem::take(&mut running.homes);
            if let Err(error) = self.record_members(homes, &mut journal).await {
                filed = Err(error);
            }
        }
        match filed {
            Ok(project) => Ok(Filed { project, journal }),
            Err(error) => Err(self.undo(journal, error).await),
        }
    }

    /// Take back what [`Engine::member_project`] wrote, because the write it filed for failed
    /// with `error`.
    pub(super) async fn unfile(&self, filed: Filed, error: EngineError) -> EngineError {
        self.undo(filed.journal, error).await
    }

    /// The destination source, once it is established it exists and can be written.
    fn writable(&self, name: &SourceName) -> Result<&ResolvedSource, EngineError> {
        let name = self.known(name)?;
        if let Some(unavailable) = self.unavailable().find(|source| source.name() == &name) {
            return Err(EngineError::DestinationUnavailable {
                name: name.to_string(),
                error: unavailable.error().clone(),
            });
        }
        let source = self
            .ready()
            .find(|source| source.name() == &name)
            .ok_or(EngineError::NoSources)?;
        if !source.source().writes().is_supported() {
            return Err(EngineError::NotWritable {
                name: name.to_string(),
                kind: source.kind().to_owned(),
            });
        }
        Ok(source)
    }

    /// Copy each project and what travels with it: every task in it, the members a member
    /// copy names, or nothing at all.
    ///
    /// Every item of every project is read and its target decided **before any of them is
    /// written, and each exactly once.** That is what lets a member copy refuse an edge it
    /// cannot resolve while the destination is still as it was found, and what stops a
    /// whole copy resolving one target twice — once to learn the ids the project's own
    /// edges name, and again to land it — which against a hosted destination is a read per
    /// item spent on an answer the command already had.
    async fn copy_projects(
        &self,
        request: &CopyRequest,
        projects: &[(GlobalId, Vec<GlobalId>)],
        unrecorded: &[GlobalId],
        running: &mut Running,
        journal: &mut Journal,
    ) -> Result<Vec<CopyOutcome>, EngineError> {
        let mut plans = Vec::new();
        for (id, members) in projects {
            if self.routes.routes(&request.destination) {
                // Every task is read before the project is aimed, because where its home goes
                // is decided by where they do.
                let mut project = self.read(Level::Project, id).await?;
                let mut reads = Vec::new();
                let mut changed = Vec::new();
                self.prefetch(request, members, running).await;
                for member in members {
                    match self
                        .read_for_copy(request, Level::Task, member, running)
                        .await
                    {
                        Ok(read) => reads.push(read),
                        Err(error @ EngineError::ChangedWhileCopied { .. }) => {
                            changed.push(failed(member.clone(), &error));
                        }
                        Err(error) => return Err(error),
                    }
                }
                let mut document_reads = self.read_documents(request, id, running).await?;
                {
                    let mut ahead = vec![project];
                    ahead.append(&mut reads);
                    let carried = document_reads.len();
                    ahead.append(&mut document_reads);
                    self.read_ahead(request, &ahead, running).await;
                    document_reads = ahead.split_off(ahead.len() - carried);
                    let mut ahead = ahead.into_iter();
                    project = ahead.next().expect("the project was read first");
                    reads = ahead.collect();
                }
                let class = self.read_project_class(&project, &reads, running).await?;
                let homing = self
                    .home_placement(request, &project, &reads, class)
                    .await?;
                let home = homing.home;
                // A private plan homed somewhere declared private lands whole there, every task
                // of it canonically beside its home; its public tasks are seen publicly only
                // through views made where their repositories route them (see `projections`).
                let projects = self.makes_projections(&home.destination, class);
                let project = self
                    .aim(
                        request,
                        project,
                        home.clone(),
                        class,
                        None,
                        &mut running.home_members,
                    )
                    .await?;
                let mut tasks = Vec::new();
                for read in reads {
                    let (placement, classification) = match &read.item {
                        Item::Task(task) if projects => (
                            home.clone(),
                            self.classify(task.classification, &task.repositories).await,
                        ),
                        // A task naming no repository has no say in where it goes: it follows
                        // its home, wherever that went.
                        Item::Task(task)
                            if homing.following == Following::ByRepositories
                                && !task.repositories.is_empty() =>
                        {
                            let own = self
                                .own_class(class, task.classification, &task.repositories)
                                .await;
                            let placement = self.routes.place_classified(
                                &request.destination,
                                own,
                                &task.repositories,
                            );
                            // A task lands as private as its own record and repositories
                            // say, beside its home or in a member project elsewhere: the
                            // plan's classification never tightens it.
                            (placement, own)
                        }
                        Item::Task(task) => (
                            home.clone(),
                            self.classify(task.classification, &task.repositories).await,
                        ),
                        Item::Project(_) | Item::Document(_) => {
                            (self.placed_at(request, &request.destination), class)
                        }
                    };
                    tasks.push(
                        self.aim(
                            request,
                            read,
                            placement,
                            classification,
                            None,
                            &mut running.home_members,
                        )
                        .await?,
                    );
                }
                let mut projecting = projections::Projecting::default();
                if projects {
                    projecting = projections::Projecting::active();
                    for task in &tasks {
                        if let Some(to) =
                            self.projection_target(&request.destination, &home.destination, task)
                        {
                            projecting.add(task, to);
                        }
                    }
                }
                let projecting = projecting.settled(&project);
                let mut documents = Vec::new();
                for read in document_reads {
                    documents.push(self.plan_read(request, read, running).await?);
                }
                plans.push((project, tasks, changed, projecting, documents));
            } else {
                let project = self
                    .read_for_copy(request, Level::Project, id, running)
                    .await?;
                let mut reads = Vec::new();
                let mut changed = Vec::new();
                self.prefetch(request, members, running).await;
                for member in members {
                    match self
                        .read_for_copy(request, Level::Task, member, running)
                        .await
                    {
                        Ok(read) => reads.push(read),
                        Err(error @ EngineError::ChangedWhileCopied { .. }) => {
                            changed.push(failed(member.clone(), &error));
                        }
                        Err(error) => return Err(error),
                    }
                }
                let mut document_reads = self.read_documents(request, id, running).await?;
                // The project, every task and every document it carries read before any is
                // aimed, so the destination is read for all of their counterparts together.
                reads.insert(0, project);
                let carried = document_reads.len();
                reads.append(&mut document_reads);
                self.read_ahead(request, &reads, running).await;
                let document_reads = reads.split_off(reads.len() - carried);
                let mut reads = reads.into_iter();
                let project = reads.next().expect("the project was read first");
                let mut project = self.plan_read(request, project, running).await?;
                let mut tasks = Vec::new();
                for read in reads {
                    tasks.push(self.plan_read(request, read, running).await?);
                }
                // The project is as private as anything it holds; each task stays as private as
                // its own record says, which a private project does not tighten.
                let mut class = tasks.iter().fold(project.classification, |class, task| {
                    class.strictest(task.classification)
                });
                if self.boundary_active() {
                    class = class.strictest(
                        self.source_project_class(id, &mut running.project_classes)
                            .await?,
                    );
                }
                project.classification = class;
                // A plan its destination no longer routes anywhere public makes no view, and the
                // map it recorded while it did forgets each one.
                let projecting = if self.makes_projections(&request.destination, class) {
                    projections::Projecting::active().settled(&project)
                } else {
                    projections::Projecting::default()
                };
                let mut documents = Vec::new();
                for read in document_reads {
                    documents.push(self.plan_read(request, read, running).await?);
                }
                plans.push((project, tasks, changed, projecting, documents));
            }
        }
        for (project, tasks, _, _, documents) in &plans {
            for item in std::iter::once(project).chain(tasks).chain(documents) {
                unrecorded_far_end(item, unrecorded)?;
            }
        }
        for (project, tasks, _, _, documents) in &mut plans {
            for item in std::iter::once(project)
                .chain(tasks.iter_mut())
                .chain(documents.iter_mut())
            {
                self.prepare_public_body(item, running).await?;
            }
        }
        // Every item of every project held to the public boundary before any is written, so
        // a refusal leaves nothing anywhere — not even a project its tasks never reached.
        let rewrite = self
            .preflight_copy(
                plans
                    .iter()
                    .flat_map(|(project, tasks, _, _, documents)| {
                        std::iter::once(project).chain(tasks).chain(documents)
                    })
                    .filter(|item| !item.skipped)
                    .collect(),
                running,
            )
            .await?;
        // Only what lands somewhere screened is written as the policy passed it: a destination
        // declared private takes the original, as it always did.
        for (project, tasks, _, _, documents) in &mut plans {
            for item in std::iter::once(project)
                .chain(tasks.iter_mut())
                .chain(documents.iter_mut())
            {
                if self.declared(&item.to) != crate::config::SourceVisibility::Private {
                    rewritten(item, &rewrite)?;
                }
            }
        }
        // Every item of every plan sized as the policy passed it, before anything is written.
        let sized: Vec<&Planned> = plans
            .iter()
            .flat_map(|(project, tasks, _, _, documents)| {
                std::iter::once(project).chain(tasks).chain(documents)
            })
            .collect();
        self.size_bodies(request, &sized, running)?;
        // Every public view too, before anything of any plan is written.
        for (_, _, _, projecting, _) in &mut plans {
            if projecting.lands() {
                if projecting.could_address() && running.private_locations.is_none() {
                    running.private_locations = Some(self.private_locations().await?);
                }
                projecting.public_bodies(
                    self,
                    running.private_locations.as_deref().unwrap_or_default(),
                )?;
            }
            self.preflight_projections(projecting).await?;
        }
        running.preflighted = true;
        for (project, tasks, _, _, documents) in &plans {
            for item in std::iter::once(project).chain(tasks).chain(documents) {
                note_private(running, item);
            }
        }
        let mut outcomes = Vec::new();
        for (project, tasks, changed, projecting, documents) in plans {
            let landed = self
                .copy_project(request, project, tasks, &changed, running, journal)
                .await?;
            if projecting.lands() && !request.dry_run {
                self.land_projections(&projecting, &landed, journal).await?;
            }
            outcomes.extend(landed);
            // The plan's documents last, each filed under the project's counterpart this copy
            // has just landed and naming the records it has just written.
            if !documents.is_empty() {
                outcomes.extend(
                    self.copy_items(request, documents, None, running, journal)
                        .await?,
                );
            }
            // A task that would not hold still is reported where its project's tasks are,
            // having written nothing.
            outcomes.extend(changed);
        }
        Ok(outcomes)
    }

    /// Read each document a `project copy --documents` carries with `project`, as a document
    /// copy reads it: once, since a document holds no comments to read beside it.
    async fn read_documents(
        &self,
        request: &CopyRequest,
        project: &GlobalId,
        running: &mut Running,
    ) -> Result<Vec<Read>, EngineError> {
        let documents = running
            .plan_documents
            .get(project)
            .cloned()
            .unwrap_or_default();
        let mut reads = Vec::with_capacity(documents.len());
        for document in documents {
            reads.push(
                self.read_for_copy(request, Level::Document, &document, running)
                    .await?,
            );
        }
        Ok(reads)
    }

    /// Copy the members a members-only copy names into the projects the destination already
    /// holds, leaving each project as it is: see [`CopyScope::MembersOnly`].
    ///
    /// Every project's counterpart, and the member project of it in each other source a task
    /// is routed to, is settled before anything is read at the destination, so a project with
    /// none is refused while nothing is written. The tasks are then landed as a task copy lands
    /// them, each filed under the counterpart settled here rather than under one it looks for.
    async fn copy_into_projects(
        &self,
        request: &CopyRequest,
        projects: &[(GlobalId, Vec<GlobalId>)],
        unrecorded: &[GlobalId],
        running: &mut Running,
        journal: &mut Journal,
    ) -> Result<Vec<CopyOutcome>, EngineError> {
        let mut planned = Vec::new();
        let mut changed = Vec::new();
        for (id, members) in projects {
            let home = self.held_counterpart(request, id).await?;
            running.counterparts.insert(id.to_string(), home.clone());
            running.filings.insert(
                (id.to_string(), home.source.clone()),
                Some(home.native.clone()),
            );
            self.prefetch(request, members, running).await;
            let mut reads = Vec::new();
            for member in members {
                match self
                    .read_for_copy(request, Level::Task, member, running)
                    .await
                {
                    Ok(read) => reads.push(read),
                    Err(error @ EngineError::ChangedWhileCopied { .. }) => {
                        changed.push(failed(member.clone(), &error));
                    }
                    Err(error) => return Err(error),
                }
            }
            self.read_ahead(request, &reads, running).await;
            for read in reads {
                let classification = self.copied_classification(request, &read, running).await?;
                // A task naming no repository follows its project, wherever that is held.
                let placement = match &read.item {
                    Item::Task(task) if !task.repositories.is_empty() => self
                        .routes
                        .place_classified(&request.destination, classification, &task.repositories),
                    _ => self.placed_at(request, &home.source),
                };
                let key = (id.to_string(), placement.destination.clone());
                if let std::collections::btree_map::Entry::Vacant(vacant) =
                    running.filings.entry(key)
                {
                    let member = self
                        .held_member(request, id, &home, &placement.destination)
                        .await?;
                    vacant.insert(Some(member));
                }
                let aimed = running.aimed.remove(&read.source.to_string());
                planned.push(
                    self.aim(
                        request,
                        read,
                        placement,
                        classification,
                        aimed,
                        &mut running.home_members,
                    )
                    .await?,
                );
            }
        }
        for item in &planned {
            unrecorded_far_end(item, unrecorded)?;
        }
        let mut outcomes = self
            .copy_items(request, planned, None, running, journal)
            .await?;
        // A task that would not hold still is reported beside the ones that landed, having
        // written nothing.
        outcomes.extend(changed);
        Ok(outcomes)
    }

    /// The project the destination — or a source it routes to — already holds for `project`,
    /// found without reading it: the `onetaskgraph.copies` link `project` records there, else
    /// its recorded origin when that names one of those sources, else the first project there
    /// recording `project` as its origin.
    ///
    /// # Errors
    ///
    /// [`EngineError::NoProjectCounterpart`] when none of the three finds one.
    async fn held_counterpart(
        &self,
        request: &CopyRequest,
        project: &GlobalId,
    ) -> Result<GlobalId, EngineError> {
        let reachable = self.routes.reachable(&request.destination);
        let held = self.read(Level::Project, project).await?;
        let (_, metadata) = described(&held.item);
        if let Some(link) = reachable.iter().find_map(|there| link_of(metadata, there)) {
            return Ok(link);
        }
        if let Some(origin) =
            origin_of(metadata).filter(|origin| reachable.contains(&origin.source))
        {
            return Ok(origin);
        }
        for there in reachable {
            let at = self.writable(&there)?;
            if let Some(found) = self
                .scan(at, Level::Project, &Wanted::Origin(project.to_string()))
                .await?
            {
                return Ok(GlobalId::new(there, found));
            }
        }
        Err(EngineError::NoProjectCounterpart {
            project: project.clone(),
            destination: request.destination.clone(),
            missing: request.destination.clone(),
        })
    }

    /// The member project `home` already has in `to`, for a task a members-only copy routes
    /// there: the project in `to` recording `home` at `onetaskgraph.member_of`. The home itself
    /// is not read for the list it keeps, because a members-only copy reads nothing of it.
    ///
    /// # Errors
    ///
    /// [`EngineError::NoProjectCounterpart`] when `to` holds none, which only a whole project
    /// copy creates.
    async fn held_member(
        &self,
        request: &CopyRequest,
        project: &GlobalId,
        home: &GlobalId,
        to: &SourceName,
    ) -> Result<NativeId, EngineError> {
        let there = self.writable(to)?;
        self.scan(
            there,
            Level::Project,
            &Wanted::Metadata(
                MetadataKey::MEMBER_OF_KEY.to_owned(),
                Value::String(home.to_string()),
            ),
        )
        .await?
        .ok_or_else(|| EngineError::NoProjectCounterpart {
            project: project.clone(),
            destination: request.destination.clone(),
            missing: to.clone(),
        })
    }

    /// A project's classification from its read and its tasks' (see
    /// [`Engine::project_class`]), remembered for the rest of this command.
    async fn read_project_class(
        &self,
        project: &Read,
        tasks: &[Read],
        running: &mut Running,
    ) -> Result<Classification, EngineError> {
        let Item::Project(held) = &project.item else {
            return Ok(Classification::Public);
        };
        let source = self.readable(&project.source.source)?;
        let class = self
            .project_class(
                source,
                &project.source,
                held,
                tasks.iter().filter_map(|read| match &read.item {
                    Item::Task(task) => Some(&**task),
                    Item::Project(_) | Item::Document(_) => None,
                }),
            )
            .await?;
        running
            .project_classes
            .insert(project.source.clone(), class);
        Ok(class)
    }

    /// Where a routed project copy lands the project: its **home**, and whether everything it
    /// carries goes there with it.
    ///
    /// A project holding anything private goes, with everything it holds, wholly to the
    /// private source a `classification: private` route names — and so does one in a store
    /// whose boundary is inactive, which is today's rule for it.
    ///
    /// Otherwise the home is decided by its **voters**: every task copied that names a
    /// repository, and the project's own repositories when it names any. A task naming none,
    /// and every document, never votes, and follows the home wherever it goes. When every voter
    /// places at one source the home goes there. When they disagree, a plan holding anything
    /// private while the boundary is active goes to the one source declared private its private
    /// voters place at — refused, before anything is written, when they place at two — and any
    /// other plan's home is the copy's destination. With no voter at all, it is the copy's
    /// destination too.
    ///
    /// A home that already exists, in any source the copy can reach, stays where it is —
    /// found by the project's own link or origin — and is looked for before a split among the
    /// private voters is refused, since that refusal is about choosing a new home.
    async fn home_placement(
        &self,
        request: &CopyRequest,
        project: &Read,
        tasks: &[Read],
        class: Classification,
    ) -> Result<Homing, EngineError> {
        let Item::Project(held) = &project.item else {
            return Ok(Homing {
                home: self.placed_at(request, &request.destination),
                following: Following::ByRepositories,
            });
        };
        if class == Classification::Private
            && (self.routes.classifies_private(&request.destination) || !self.boundary_active())
        {
            return Ok(Homing {
                home: self
                    .routes
                    .place_classified(&request.destination, class, &[]),
                following: Following::Whole,
            });
        }
        let mut voters: Vec<(GlobalId, Classification, Placement)> = Vec::new();
        for read in tasks {
            if let Item::Task(task) = &read.item
                && !task.repositories.is_empty()
            {
                let own = self
                    .own_class(class, task.classification, &task.repositories)
                    .await;
                let placement =
                    self.routes
                        .place_classified(&request.destination, own, &task.repositories);
                voters.push((read.source.clone(), own, placement));
            }
        }
        if !held.repositories.is_empty() {
            let own = self
                .own_class(class, held.classification, &held.repositories)
                .await;
            let placement =
                self.routes
                    .place_classified(&request.destination, own, &held.repositories);
            voters.push((project.source.clone(), own, placement));
        }
        // Where a new home would go. A refusal to choose one waits until no home is found to
        // exist, because an existing home stays where it is whatever its voters now say.
        let rule = match voters.first() {
            Some((_, _, first))
                if voters
                    .iter()
                    .all(|(_, _, placement)| placement.destination == first.destination) =>
            {
                Ok(first.clone())
            }
            Some(_) if class == Classification::Private && self.boundary_active() => {
                self.private_home(request, &project.source, &voters)
            }
            _ => Ok(self.placed_at(request, &request.destination)),
        };
        for elsewhere in self.routes.reachable(&request.destination) {
            // A home in the source the rule chooses is found there by the copy's own target.
            if rule
                .as_ref()
                .is_ok_and(|rule| rule.destination == elsewhere)
            {
                continue;
            }
            let there = self.writable(&elsewhere)?;
            let named = link_of(&held.metadata, &elsewhere)
                .or_else(|| origin_of(&held.metadata).filter(|origin| origin.source == elsewhere));
            let Some(named) = named else {
                continue;
            };
            let Some(found) = self.prior(there, Level::Project, &named.native).await? else {
                continue;
            };
            let names_back = origin_of(described(&found.item).1).as_ref() == Some(&project.source);
            if names_back || origin_of(&held.metadata).as_ref() == Some(&named) {
                return Ok(Homing {
                    home: self.placed_at(request, &elsewhere),
                    following: Following::ByRepositories,
                });
            }
        }
        Ok(Homing {
            home: rule?,
            following: Following::ByRepositories,
        })
    }

    /// The one source declared private that a plan's private voters place at, when its voters
    /// disagree: the copy's destination when none places anywhere declared private, which the
    /// boundary then refuses.
    ///
    /// # Errors
    ///
    /// [`EngineError::SplitPrivateHome`] when they place at more than one, naming each with the
    /// voters placing there.
    fn private_home(
        &self,
        request: &CopyRequest,
        project: &GlobalId,
        voters: &[(GlobalId, Classification, Placement)],
    ) -> Result<Placement, EngineError> {
        let mut destinations: Vec<(Placement, Vec<GlobalId>)> = Vec::new();
        for (voter, own, placement) in voters {
            if *own != Classification::Private || !self.routes.is_private(&placement.destination) {
                continue;
            }
            match destinations
                .iter_mut()
                .find(|(held, _)| held.destination == placement.destination)
            {
                Some((_, items)) => items.push(voter.clone()),
                None => destinations.push((placement.clone(), vec![voter.clone()])),
            }
        }
        match destinations.len() {
            0 => Ok(self.placed_at(request, &request.destination)),
            1 => Ok(destinations.remove(0).0),
            _ => Err(EngineError::SplitPrivateHome {
                project: project.clone(),
                destinations: destinations
                    .into_iter()
                    .map(|(placement, items)| (placement.destination, items))
                    .collect(),
            }),
        }
    }

    /// Who may read one voter of a plan classified `class`, by its own record and repositories
    /// alone: public without asking anything, where the plan is.
    async fn own_class(
        &self,
        class: Classification,
        declared: Classification,
        repositories: &[Repository],
    ) -> Classification {
        if class.is_public() {
            return Classification::Public;
        }
        self.classify(declared, repositories).await
    }

    /// Land one planned project, then the tasks planned beside it.
    async fn copy_project(
        &self,
        request: &CopyRequest,
        project: Planned,
        tasks: Vec<Planned>,
        changed: &[CopyOutcome],
        running: &mut Running,
        journal: &mut Journal,
    ) -> Result<Vec<CopyOutcome>, EngineError> {
        let (carries_tasks, walks_orphans) = match request.scope {
            CopyScope::Projects { tasks, .. } => (tasks, tasks),
            CopyScope::Members(_) | CopyScope::MembersOnly(_) => (true, false),
            CopyScope::Tasks | CopyScope::Documents => (false, false),
        };
        let id = project.source.clone();
        // A member that would not hold still is still a member: its counterpart is not an
        // orphan for having been left unwritten this time.
        let members: Vec<GlobalId> = tasks
            .iter()
            .map(|task| task.source.clone())
            .chain(changed.iter().map(|outcome| outcome.source.clone()))
            .collect();
        // The project lands with every edge it can already resolve — its members' targets
        // are known from their plans — so a project that has not changed is not written at
        // all. What it *reports* is read the way it always has been: against the ids known
        // before its own members' were, unless every edge resolves and nothing differs.
        // Reading it any other way would move the word a repeat copy reports for a project
        // whose only difference is an edge, which is not this change's to move.
        let mut before_members = running.counterparts.clone();
        if let Target::Update { id: target, .. } = &project.target {
            before_members.insert(
                id.to_string(),
                GlobalId::new(project.to.clone(), target.clone()),
            );
        }
        for item in std::iter::once(&project).chain(&tasks) {
            if let Target::Update { id: target, .. } = &item.target {
                running.counterparts.insert(
                    item.source.to_string(),
                    GlobalId::new(item.to.clone(), target.clone()),
                );
            }
        }
        let home_source = project.to.clone();
        let home = self.writable(&home_source)?;
        let mut project = project;
        self.store_statuses(std::slice::from_mut(&mut project))
            .await?;
        let unchanged = match &project.target {
            Target::Update { id: target, .. } => {
                let settled = self.shown_edges(
                    &home_source,
                    &id,
                    mapped_edges(
                        &project.edges,
                        &id.source,
                        &home_source,
                        &running.resolvable,
                        &running.counterparts,
                    ),
                    &running.private,
                );
                let first = self.shown_edges(
                    &home_source,
                    &id,
                    mapped_edges(
                        &project.edges,
                        &id.source,
                        &home_source,
                        &running.resolvable,
                        &before_members,
                    ),
                    &running.private,
                );
                let held = project.held.as_ref();
                let covers = self.version_covers_a_copy(&project);
                let unchanged_with = |edges: &[Option<DependencyEdge>]| {
                    !changes(
                        held,
                        &project,
                        target,
                        &None,
                        &resolved(edges),
                        &[],
                        home,
                        covers,
                    )
                };
                Some(
                    (!settled.iter().any(Option::is_none) && unchanged_with(&settled))
                        || unchanged_with(&first),
                )
            }
            Target::Create => None,
        };
        // A project this copy creates holds nothing it did not file itself, so nothing filed
        // under it can be an orphan and the destination is not walked for one.
        let created = matches!(project.target, Target::Create);
        let promotes = match project.target.found() {
            Some(Found::Member { home }) => Some(home.clone()),
            _ => None,
        };
        let mut outcomes = self
            .copy_items(request, vec![project], None, running, journal)
            .await?;
        // The home whose member this project took over names it no longer, once the copy has
        // landed.
        if let (Some(home), Some(landed), false) = (
            promotes,
            outcomes[0].destination().cloned(),
            request.dry_run,
        ) {
            self.release_member(&home, &landed, running).await?;
        }
        if let (Some(unchanged), Some(landed), Some(via)) = (
            unchanged,
            outcomes[0].destination().cloned(),
            outcomes[0].action.found_by(),
        ) {
            let link = outcomes[0].action.link();
            outcomes[0].action = if unchanged {
                CopyAction::Unchanged {
                    destination: landed,
                    via,
                    link,
                    skipped: false,
                    comments: None,
                }
            } else {
                CopyAction::Updated {
                    destination: landed,
                    via,
                    link,
                    comments: None,
                }
            };
        }
        if !carries_tasks {
            return Ok(outcomes);
        }
        // `None` when a dry run would have created the project: nothing was written, so
        // there is no destination project id to file the tasks under. Every task is still
        // read and still reported, because that is what a dry run is for.
        let filed = outcomes.first().and_then(CopyOutcome::destination).cloned();
        let filing = match &filed {
            None => None,
            Some(home) => {
                let mut filing = Filing::from([(home.source.clone(), home.native.clone())]);
                // A task routed to another source than its home's is filed under the home's
                // member project there, found or created before any of those tasks lands.
                let mut elsewhere: Vec<SourceName> = Vec::new();
                for task in &tasks {
                    if task.to != home.source && !elsewhere.contains(&task.to) {
                        elsewhere.push(task.to.clone());
                    }
                }
                for to in elsewhere {
                    // As private as the tasks it files, which a plan's home may not be: a
                    // private plan's public work lands in a public member of its own source.
                    let filed = || tasks.iter().filter(|task| task.to == to);
                    let files = MemberFiling {
                        classification: filed().fold(Classification::Public, |class, task| {
                            class.strictest(task.classification)
                        }),
                        tasks: filed().map(|task| task.source.clone()).collect(),
                    };
                    if let Some(member) = self
                        .member_in(request.dry_run, home, &to, Some(&files), running, journal)
                        .await?
                    {
                        filing.insert(to, member);
                    }
                }
                Some(filing)
            }
        };
        outcomes.extend(
            self.copy_items(request, tasks, filing.as_ref(), running, journal)
                .await?,
        );
        // A member copy was told which members it carries, so it cannot tell a member it
        // left out from one the source no longer holds, and does not walk for either.
        if walks_orphans
            && !created
            && let Some(filed) = filed
        {
            let destination = self.writable(&filed.source)?;
            outcomes.extend(
                self.orphans(destination, &id, &filed.native, &members)
                    .await?,
            );
            // A routed plan's tasks are filed under its members too, so each member is walked
            // in its own source for what the source plan no longer holds.
            if self.routes.routes(&request.destination) {
                for member in self.members_named(&filed, running).await? {
                    let there = self.writable(&member.source)?;
                    outcomes.extend(self.orphans(there, &id, &member.native, &members).await?);
                }
            }
        }
        Ok(outcomes)
    }

    /// Take `member` off the member projects `home` names, to be written once the whole copy
    /// has landed — nothing at all when the home is gone or names it not.
    async fn release_member(
        &self,
        home: &GlobalId,
        member: &GlobalId,
        running: &mut Running,
    ) -> Result<(), EngineError> {
        let key = home.to_string();
        if !running.homes.contains_key(&key) {
            let at = self.writable(&home.source)?;
            let Some(held) = at
                .source()
                .get_project(&home.native)
                .await
                .map_err(|error| refused(at, error))?
            else {
                return Ok(());
            };
            running
                .homes
                .insert(key.clone(), Home::of(home.clone(), &held)?);
        }
        let entry = running
            .homes
            .get_mut(&key)
            .expect("the home was recorded above");
        if entry.members.contains(member) {
            entry.members.retain(|named| named != member);
            entry.changed = true;
        }
        Ok(())
    }

    /// The member projects `home` names: the list this copy already holds for it, with every
    /// member it added or released, or else the one the home itself records.
    async fn members_named(
        &self,
        home: &GlobalId,
        running: &Running,
    ) -> Result<Vec<GlobalId>, EngineError> {
        if let Some(held) = running.homes.get(&home.to_string()) {
            return Ok(held.members.clone());
        }
        let at = self.writable(&home.source)?;
        let Some(held) = at
            .source()
            .get_project(&home.native)
            .await
            .map_err(|error| refused(at, error))?
        else {
            return Ok(Vec::new());
        };
        members_of(home, &held.metadata).map_err(|message| EngineError::SourceRefused {
            name: home.source.to_string(),
            error: SourceError::Malformed { message },
        })
    }

    /// The members a member copy names, per project and in the order they were named, and
    /// every member it does not name that records no destination id.
    ///
    /// Settled before anything is read at the destination. A named id that is a member of
    /// none of the projects is refused here, naming it. An unnamed member whose recorded
    /// origin names the destination joins the copied set with that id already known, so an
    /// edge to it resolves exactly as an edge to a copied item does, and costs no read. An
    /// unnamed member recording none is returned, so an edge to it is refused before
    /// anything is written — see [`unrecorded_far_end`].
    async fn named_members(
        &self,
        request: &CopyRequest,
        named: &CopyItems,
        running: &mut Running,
    ) -> Result<(Vec<(GlobalId, Vec<GlobalId>)>, Vec<GlobalId>), EngineError> {
        let projects = request.items.as_slice();
        // A routed copy can land a member in any source it reaches, so an origin naming any
        // of them is a destination id already known.
        let reachable = self.routes.reachable(&request.destination);
        let mut carried = Vec::new();
        let mut unrecorded = Vec::new();
        for project in projects {
            let held = self.project_member_tasks(project, FOR_A_COPY).await?;
            let mut members: Vec<GlobalId> = Vec::new();
            for id in named.as_slice() {
                if held.iter().any(|task| &task.id == id) && !members.contains(id) {
                    members.push(id.clone());
                }
            }
            for task in held {
                if members.contains(&task.id) {
                    continue;
                }
                // A routed copy also reads the member's own link, because a plan authored
                // where it is copied from records where each task landed rather than an
                // origin, and its tasks may have landed in any source the copy reaches.
                let linked = || {
                    reachable
                        .iter()
                        .find_map(|there| link_of(&task.item.metadata, there))
                        .filter(|_| self.routes.routes(&request.destination))
                };
                match origin_of(&task.item.metadata)
                    .filter(|origin| reachable.contains(&origin.source))
                    .or_else(linked)
                {
                    Some(origin) => {
                        running.counterparts.insert(task.id.to_string(), origin);
                        running.resolvable.push(task.id);
                    }
                    None => unrecorded.push(task.id),
                }
            }
            running.resolvable.push(project.clone());
            running.resolvable.extend(members.iter().cloned());
            carried.push((project.clone(), members));
        }
        if let Some(stray) = named
            .as_slice()
            .iter()
            .find(|id| !carried.iter().any(|(_, members)| members.contains(id)))
        {
            return Err(EngineError::NotAMember {
                id: stray.clone(),
                projects: projects.to_vec(),
            });
        }
        Ok((carried, unrecorded))
    }

    /// Every task the source holds in `project` and, when it is a home, in each member
    /// project it names, by qualified id: the plan a whole-project copy carries, read as
    /// `task list --project <home> --members` reads it, so a home copied on takes its members'
    /// tasks with it and the destination's own routes file each one.
    async fn project_members(&self, project: &GlobalId) -> Result<Vec<GlobalId>, EngineError> {
        Ok(self
            .project_tasks(project, FOR_A_COPY, true)
            .await?
            .into_iter()
            .map(|task| task.id)
            .collect())
    }

    /// Every task the source holds in `project`, as the source reported it.
    ///
    /// The ids alone are what a copy files under a project; the whole task is what a
    /// document's references need, because the location a reference names is a field of it,
    /// and what `project graph` draws. `reading` says which of those the walk was for, in the
    /// refusal a source that cycles its cursors earns.
    pub(super) async fn project_member_tasks(
        &self,
        project: &GlobalId,
        reading: &str,
    ) -> Result<Vec<Qualified<Task>>, EngineError> {
        self.project_tasks(project, reading, false).await
    }

    /// [`project_member_tasks`](Self::project_member_tasks), with the tasks of every member
    /// project `project` names when `members` asks.
    async fn project_tasks(
        &self,
        project: &GlobalId,
        reading: &str,
        members: bool,
    ) -> Result<Vec<Qualified<Task>>, EngineError> {
        let mut request = TaskRequest {
            sources: vec![project.source.clone()],
            filters: Filters::default(),
            project: ProjectSelector::Qualified(project.clone()),
            priorities: Vec::new(),
            commented_since: None,
            metadata: Vec::new(),
            origin: None,
            include_members: members,
            paging: Paging {
                limit: PROJECT_PAGE,
                token: None,
            },
        };
        let mut members = Vec::new();
        // Pages by this engine's own token rather than by a source cursor, and the
        // asymmetry with the three walks below is deliberate. A source answering two
        // cursors with each other advances on every page, so `unrepeated` under the list
        // verb never fires; the cycle shows only as a token handed back unchanged, which
        // is what this loop pages by. Point it at the source and a project copy spins.
        //
        // No page bound here for the same reason: `Engine::tasks` merges under the budget
        // it was asked, so nothing longer than `PROJECT_PAGE` can arrive. `fits` in
        // `fetch::walk` refuses the page a source can really overrun, its own, while
        // these very members are read.
        let misbehaved = |error| EngineError::SourceRefused {
            name: project.source.to_string(),
            error,
        };
        loop {
            let asked = request.paging.token.clone();
            let response = self.tasks(&request).await?;
            if let Some(failure) = response.errors.first() {
                return Err(EngineError::SourceRefused {
                    name: failure.source.to_string(),
                    error: failure.error.clone(),
                });
            }
            unrepeated(
                response.next.as_ref(),
                asked.as_ref(),
                &format!("the tasks of a project were being read {reading}"),
            )
            .map_err(misbehaved)?;
            members.extend(response.items);
            match response.next {
                Some(token) => request.paging.token = Some(token),
                None => return Ok(members),
            }
        }
    }

    /// Every document the source holds in `project`, as the source reported it.
    ///
    /// Paged by this engine's own token for the reason the member walk above is, and the
    /// note there says why.
    async fn project_documents(
        &self,
        project: &GlobalId,
    ) -> Result<Vec<Qualified<Document>>, EngineError> {
        let mut request = DocumentRequest {
            sources: vec![project.source.clone()],
            filters: DocumentFilters::default(),
            project: ProjectSelector::Qualified(project.clone()),
            paging: Paging {
                limit: PROJECT_PAGE,
                token: None,
            },
        };
        let mut held = Vec::new();
        let misbehaved = |error| EngineError::SourceRefused {
            name: project.source.to_string(),
            error,
        };
        loop {
            let asked = request.paging.token.clone();
            let response = self.documents(&request).await?;
            if let Some(failure) = response.errors.first() {
                return Err(EngineError::SourceRefused {
                    name: failure.source.to_string(),
                    error: failure.error.clone(),
                });
            }
            unrepeated(
                response.next.as_ref(),
                asked.as_ref(),
                "the documents of a project were being read for a copy",
            )
            .map_err(misbehaved)?;
            held.extend(response.items);
            match response.next {
                Some(token) => request.paging.token = Some(token),
                None => return Ok(held),
            }
        }
    }

    /// Point every reference the documents of this copy hold at the destination's own
    /// records, and say how many it could not.
    ///
    /// A document copied out of a local Markdown store used to arrive naming absolute paths
    /// under one checkout on one machine, dead for the only reader the copy exists for,
    /// while the destination held its own record for every one of them the whole time. This
    /// is what closes that, and it is deliberately **not** a Markdown-link parser: the
    /// artifact that motivated it holds bare absolute paths inside backticks in a table
    /// cell, which `[text](target)` matching would have left exactly as it found them.
    ///
    /// The correspondence is re-established from what the *destination* records at
    /// [`GlobalId::ORIGIN_KEY`], not from the mapping this copy holds. That mapping is not
    /// available when it is needed: `project copy` and `document copy` are separate verbs,
    /// one invocation carries one [`CopyScope`], and a project copy carries no documents —
    /// so by the time the document is copied, the tasks were written by a process that has
    /// exited. Reading the destination is also what makes a document copied on its own
    /// work, which a same-run mapping never could.
    ///
    // llmlint: ignore-block[contracts_have_one_source_or_a_drift_gate] This comment is required to state the rule; it is held by the journeys `a_rendering_whose_references_a_copy_rewrites_records_the_digest_of_what_it_was_given`, `a_copy_carries_provenance_it_cannot_vouch_for_verbatim` and `a_rendering_whose_references_a_copy_leaves_alone_carries_its_provenance_unchanged` in `crates/onetaskgraph-e2e/tests/e2e/rendered.rs`, and the entry's fields are `TemplateProvenance`'s own.
    /// Tasks and projects are not touched. A document's content is rewritten, and so is
    /// one thing beside it: when this substitutes at least one reference into a rendering
    /// that still hashes to the `body_digest` its [`TemplateProvenance`] records, the copy
    /// records as `body_digest` the digest of the rendering as this rewrote its references,
    /// carrying the entry's other three fields; any other entry, or none, is carried
    /// verbatim. The answers that rendering was made from, and the `answers_digest` naming
    /// them, still hold the pre-copy locations, so the copy is not a fresh rendering of them
    /// and a regenerate from them reproduces the pre-copy content.
    // llmlint: ignore-end[contracts_have_one_source_or_a_drift_gate]
    async fn rewrite_references(
        &self,
        planned: &mut [Planned],
        copied: &[GlobalId],
        carries_work: bool,
        counts: &mut Counted,
        private_locations: Option<&[String]>,
    ) -> Result<(), EngineError> {
        // Read at the source, once per project rather than once per body: several bodies of
        // one project is the ordinary case.
        let mut by_project: BTreeMap<String, Vec<Referent>> = BTreeMap::new();
        let mut named: Vec<Vec<Referent>> = Vec::new();
        for item in planned.iter() {
            named.push(if item.links.is_some() {
                Vec::new()
            } else {
                self.named_referents(
                    item,
                    &mut by_project,
                    self.reading_of(item, private_locations),
                )
                .await?
            });
        }
        // What this very call writes is found by the copy's own target rather than by asking
        // the destination, which on a first copy holds nothing of it yet: an update's
        // counterpart is where the read that found it says, and a create's is known once
        // it has landed.
        let written: HashMap<WrittenKey, Option<(NativeId, Option<String>)>> = planned
            .iter()
            .map(|item| {
                let at = match &item.target {
                    Target::Update { id, .. } => Some((
                        id.clone(),
                        item.held
                            .as_ref()
                            .and_then(|held| located(location_of(&held.item))),
                    )),
                    Target::Create => None,
                };
                (
                    (item.to.clone(), item.item.level(), item.source.clone()),
                    at,
                )
            })
            .collect();
        // The destination is walked only for what a body really names and this call does not
        // write, and only for the interfaces those referents are read from — in each source a
        // body of this call lands in, which a routed copy decides per item.
        let mut wanted: BTreeMap<SourceName, Vec<&Referent>> = BTreeMap::new();
        for (item, referents) in planned.iter().zip(&named) {
            for referent in referents {
                if written.contains_key(&(item.to.clone(), referent.level, referent.id.clone())) {
                    continue;
                }
                let at = wanted.entry(item.to.clone()).or_default();
                if !at
                    .iter()
                    .any(|held| held.id == referent.id && held.level == referent.level)
                {
                    at.push(referent);
                }
            }
        }
        let mut walked: BTreeMap<SourceName, Counterparts> = BTreeMap::new();
        for (to, referents) in wanted {
            let destination = self.writable(&to)?;
            walked.insert(to, self.counterparts(destination, &referents).await?);
        }
        // The copied set names records by qualified id alone, and an id alone does not identify a
        // record: a document copy's set is its documents, never a task or project sharing an id.
        // A project copy carrying its documents holds its work too, rewritten a body at a time.
        let copies_work = carries_work
            || planned
                .iter()
                .any(|item| item.item.level() != Level::Document);
        let none = Counterparts::default();
        let mut read_for_headings: BTreeSet<(SourceName, Level, NativeId)> = BTreeSet::new();
        for (item, referents) in planned.iter_mut().zip(named) {
            if item.links.is_some() {
                continue;
            }
            let content = content_of(&item.item).unwrap_or_default().to_owned();
            if content.is_empty() && item.carried.as_ref().is_none_or(Vec::is_empty) {
                continue;
            }
            let source = self.readable(&item.source.source)?;
            let destination = self.writable(&item.to)?;
            let from = SectionForm::for_kind(source.kind());
            let to = SectionForm::for_kind(destination.kind());
            let counterparts = walked.get(&item.to).unwrap_or(&none);
            let linked = referents
                .iter()
                .map(|referent| {
                    let mut linked = sections::Linked::new(
                        referent.level,
                        referent.id.clone(),
                        referent.location.clone(),
                        from,
                        referent.content.as_deref(),
                        referent.stored.clone(),
                    );
                    linked.counterpart = match written.get(&(
                        item.to.clone(),
                        referent.level,
                        referent.id.clone(),
                    )) {
                        Some(Some((id, Some(location)))) => sections::Counterpart::At {
                            id: id.clone(),
                            location: location.clone(),
                        },
                        Some(_) => sections::Counterpart::InCopy,
                        // A member of the copied set is found by its own id alone: the origin it
                        // records names an ancestor, whose other copies — a fan-out landed here
                        // before — are not the counterpart this copy writes it to.
                        None => match if referent.level != Level::Document
                            && copies_work
                            && copied.contains(&referent.id)
                        {
                            counterparts.resolve_own(referent)
                        } else {
                            counterparts.resolve(referent)
                        } {
                            Resolution::Rewrite(id, location) => {
                                sections::Counterpart::At { id, location }
                            }
                            // A member of the copied set planned in another part of this copy —
                            // as a project is planned apart from its tasks — that the destination
                            // does not hold yet: it is known once it has landed.
                            Resolution::NoCounterpart
                                if referent.level != Level::Document
                                    && copies_work
                                    && copied.contains(&referent.id) =>
                            {
                                sections::Counterpart::InCopy
                            }
                            Resolution::NoCounterpart => sections::Counterpart::None,
                            Resolution::Ambiguous => sections::Counterpart::Ambiguous,
                        },
                    };
                    linked
                })
                .collect::<Vec<_>>();
            // Two referents reporting one location string: an occurrence of it cannot be
            // attributed to either, and a rewrite would be *confidently wrong* rather than
            // merely unhelpful. So neither is chosen and every occurrence is counted.
            let linked: Vec<sections::Linked> =
                linked
                    .into_iter()
                    .fold(Vec::new(), |mut kept: Vec<sections::Linked>, referent| {
                        match kept
                            .iter_mut()
                            .find(|held| held.location == referent.location)
                        {
                            Some(held) => held.counterpart = sections::Counterpart::Ambiguous,
                            None => kept.push(referent),
                        }
                        kept
                    });
            let level = item.item.level();
            let links = sections::BodyLinks::new(
                content.clone(),
                from,
                to,
                level,
                located(location_of(&item.item)),
                source
                    .source()
                    .stored_headings(level.record(), &item.source.native),
                linked,
            )
            .screen_public(private_locations);
            let reading = self.reading_of(item, private_locations);
            let links = if self.declared(&item.to) == crate::config::SourceVisibility::Private {
                links.for_private_destination()
            } else {
                links
            };
            let own = match &item.target {
                Target::Update { id, .. } => item
                    .held
                    .as_ref()
                    .and_then(|held| located(location_of(&held.item)))
                    .map(|location| (id.clone(), location)),
                Target::Create => None,
            };
            // Each carried comment's links, read as the task's own: a fragment alone in one
            // names a heading of the task it is a comment on.
            let landing = item.to.clone();
            if let Some(carried) = item.carried.as_mut() {
                // A Linear record a comment links and this copy does not write is read once,
                // first, for the ids its headings are stored under.
                if matches!(
                    to,
                    Some(SectionForm::Linear(LinearSectionForm::HeadingLink))
                ) {
                    let probing = Planning {
                        destination,
                        own: own.clone(),
                        missed: std::cell::RefCell::new(Vec::new()),
                    };
                    for comment in carried.iter() {
                        let links = links.for_comment(comment.body.clone());
                        if could_be_read(&comment.body, reading) && links.matters() {
                            let _ = links.render(&probing, sections::Pass::Provisional);
                        }
                    }
                    for (level, id) in probing.missed.take() {
                        if read_for_headings.insert((landing.clone(), level, id.clone())) {
                            read_destination(destination, level, &id).await?;
                        }
                    }
                }
                let planning = Planning {
                    destination,
                    own: own.clone(),
                    missed: std::cell::RefCell::new(Vec::new()),
                };
                for comment in carried.iter_mut() {
                    let links = links.for_comment(comment.body.clone());
                    if private_locations.is_none()
                        && (!could_be_read(&comment.body, reading) || !links.matters())
                    {
                        continue;
                    }
                    let rendered = links.render(&planning, sections::Pass::Provisional);
                    comment.body = rendered.content;
                    if rendered.pending.is_empty() {
                        // A comment with nothing left to finish reports what it could not do now;
                        // one finished later reports it then.
                        item.comment_sections.add(rendered.sections);
                    } else {
                        // Keyed by the origin its copy carries, which a comment that is a copy
                        // already names as the first origin rather than as itself.
                        item.comment_links.push((
                            comments::key(&comments::origin_of(&item.source, comment)),
                            links,
                            rendered.pending,
                        ));
                    }
                }
            }
            if content.is_empty() || (!links.matters() && private_locations.is_none()) {
                continue;
            }
            let mut planning = Planning {
                destination,
                own: own.clone(),
                missed: std::cell::RefCell::new(Vec::new()),
            };
            let mut rendered = links.render(&planning, sections::Pass::Provisional);
            // A Linear record the copy does not write is read once, for the ids its headings
            // are stored under: a record it writes is answered by its own write.
            if matches!(
                to,
                Some(SectionForm::Linear(LinearSectionForm::HeadingLink))
            ) {
                let missed = planning.missed.take();
                let mut read = false;
                for (level, id) in missed {
                    if read_for_headings.insert((item.to.clone(), level, id.clone())) {
                        read_destination(destination, level, &id).await?;
                        read = true;
                    }
                }
                if read {
                    planning = Planning {
                        destination,
                        own,
                        missed: std::cell::RefCell::new(Vec::new()),
                    };
                    rendered = links.render(&planning, sections::Pass::Provisional);
                }
            }
            if rendered.content != content
                && let Some(provenance) =
                    stored::restamped(metadata_ref(&item.item), &[&content], &rendered.content)
            {
                metadata_of(&mut item.item)
                    .insert(TemplateProvenance::KEY.to_owned(), provenance.to_value());
            }
            set_content(&mut item.item, rendered.content);
            counts.add(rendered.counts);
            item.links = Some(links);
            item.pending = rendered.pending;
        }
        Ok(())
    }

    /// What one planned body is read for: a relative path written alone as well when it is
    /// screened for a public destination or bound for one declared private.
    fn reading_of(
        &self,
        item: &Planned,
        private_locations: Option<&[String]>,
    ) -> sections::Reading {
        if private_locations.is_some()
            || self.declared(&item.to) == crate::config::SourceVisibility::Private
        {
            sections::Reading::BarePathsToo
        } else {
            sections::Reading::WholeReferences
        }
    }

    /// The records of one body's own project whose location its content really names, in the
    /// source kind's own form: as a whole reference, or — from a folder of Markdown — as a
    /// Markdown link to a path relative to the body's own file, and in a
    /// [`sections::Reading::BarePathsToo`] as such a path alone.
    ///
    /// A body with no content, or one of no project at the source, names nothing: the
    /// referent set is the body's own project, its tasks and its documents, and there is no
    /// such set without a project. Neither does a body that holds nothing a link could be,
    /// whose project is then never read for it.
    async fn named_referents(
        &self,
        item: &Planned,
        by_project: &mut BTreeMap<String, Vec<Referent>>,
        reading: sections::Reading,
    ) -> Result<Vec<Referent>, EngineError> {
        let Some(project) = project_of(&item.item) else {
            return Ok(Vec::new());
        };
        // Its own content, and the comments a `--comments` copy carries with it, which name
        // the records of the same project.
        let bodies: Vec<&str> = content_of(&item.item)
            .into_iter()
            .chain(
                item.carried
                    .iter()
                    .flatten()
                    .map(|comment| comment.body.as_str()),
            )
            .filter(|body| could_be_read(body, reading))
            .collect();
        if bodies.is_empty() {
            return Ok(Vec::new());
        }
        let from = SectionForm::for_kind(self.readable(&item.source.source)?.kind());
        let own = located(location_of(&item.item));
        let project = GlobalId::new(item.source.source.clone(), project);
        let key = project.to_string();
        if !by_project.contains_key(&key) {
            let read = self.referents(&project).await?;
            by_project.insert(key.clone(), read);
        }
        let level = item.item.level();
        Ok(by_project[&key]
            .iter()
            // A body does not name itself as another record: its own links are same-record
            // links. Told by the interface as well as the id, because an id alone does not
            // identify a record — a folder of Markdown filing `A.md` under both `tasks/` and
            // `documents/` is the ordinary case rather than the contrived one.
            .filter(|referent| referent.level != level || referent.id != item.source)
            .filter(|referent| {
                bodies.iter().any(|content| {
                    sections::BodyLinks::names(
                        from,
                        content,
                        own.as_deref(),
                        &referent.location,
                        reading,
                    )
                })
            })
            .cloned()
            .collect())
    }

    /// Every record filed under one project at its own source, with the location string
    /// that source reports for it.
    ///
    /// The project record itself, every task filed under it, and every document filed under
    /// it. All three are reads this engine already knows how to make.
    async fn referents(&self, project: &GlobalId) -> Result<Vec<Referent>, EngineError> {
        let source = self.readable(&project.source)?;
        let mut referents = Vec::new();
        if let Some(held) = source
            .source()
            .get_project(&project.native)
            .await
            .map_err(|error| refused(source, error))?
        {
            note(
                &mut referents,
                source,
                project.clone(),
                Level::Project,
                held.location.as_ref(),
                &held.metadata,
                held.content.as_deref(),
            );
        }
        for task in self.project_member_tasks(project, FOR_A_COPY).await? {
            note(
                &mut referents,
                source,
                task.id,
                Level::Task,
                task.item.location.as_ref(),
                &task.item.metadata,
                task.item.content.as_deref(),
            );
        }
        for document in self.project_documents(project).await? {
            note(
                &mut referents,
                source,
                document.id,
                Level::Document,
                document.item.location.as_ref(),
                &document.item.metadata,
                document.item.content.as_deref(),
            );
        }
        Ok(referents)
    }

    /// The destination's records for `referents`, found from what each referent records
    /// rather than by walking everything the destination holds.
    ///
    /// **A recorded location before all.** A referent whose `onetaskgraph.copy_locations` entry
    /// names exactly what its link names here is answered from that entry, and the destination
    /// is not asked about it at all.
    ///
    /// **A link first.** A referent whose own `onetaskgraph.copies` names a record at this
    /// destination has that record read by id — every linked record read ahead together, by
    /// one [`TaskSource::read_ahead_for_copy`] — and when the record's own origin is one of the
    /// referent's two keys it is the counterpart, exactly as a copy takes a link it follows
    /// (rule 0). The copy that recorded the link wrote that record, so it is the one record
    /// the correspondence names, and no other record is looked for.
    ///
    /// **Then the origin.** Every other referent — no link here, or a link the record it
    /// names no longer confirms — has the destination asked, at its own level, for the
    /// records recording each of its two keys as their origin: a question about one origin,
    /// which a source declaring [`Capabilities::filter_by_origin`] answers without listing
    /// what it holds. A source that does not is walked once per level instead, as every
    /// destination used to be, because asking it once per key would walk it once per key.
    ///
    /// What is kept from either is three fields of each record found — its id, the origin it
    /// records and the location the destination reports — in the index
    /// [`Counterparts::resolve`] reads, which keeps every match, so a destination holding two
    /// records for one origin is reported as ambiguous rather than resolved to the first.
    /// That is the difference from [`Engine::scan`]. Nothing is written down and the index is
    /// dropped with the call.
    async fn counterparts(
        &self,
        destination: &ResolvedSource,
        referents: &[&Referent],
    ) -> Result<Counterparts, EngineError> {
        let mut found = Counterparts::default();
        let name = destination.name();
        let mut unlinked: Vec<&Referent> = Vec::new();
        // **A recorded location before all.** A referent that recorded where its linked
        // counterpart is, beside the link, is answered from that and the destination is not
        // asked about it.
        let mut asking: Vec<&Referent> = Vec::new();
        for referent in referents {
            match (referent.link(name), referent.located.get(name)) {
                (Some(id), Some(location)) => found.note_recorded(referent, &id, location),
                _ => asking.push(referent),
            }
        }
        let referents = asking;
        let linked: Vec<NativeId> = referents
            .iter()
            .filter_map(|referent| referent.link(name))
            .collect();
        if linked.len() > 1 {
            // Nothing is decided by it: each read below is made as before when it holds nothing.
            let _ = destination.source().read_ahead_for_copy(&linked).await;
        }
        for referent in referents {
            let Some(id) = referent.link(name) else {
                unlinked.push(referent);
                continue;
            };
            let held = match referent.level {
                Level::Task => destination
                    .source()
                    .get_task(&id)
                    .await
                    .map_err(|error| refused(destination, error))?
                    .map(|task| (task.id, task.location, task.metadata)),
                Level::Project => destination
                    .source()
                    .get_project(&id)
                    .await
                    .map_err(|error| refused(destination, error))?
                    .map(|project| (project.id, project.location, project.metadata)),
                Level::Document => destination
                    .source()
                    .get_document(&id)
                    .await
                    .map_err(|error| refused(destination, error))?
                    .map(|document| (document.id, document.location, document.metadata)),
            };
            // A record answered for another id than the link names is no answer about the link,
            // however its origin reads: the referent is asked for by its origin instead.
            match held {
                Some((held, location, metadata)) if held == id && confirms(referent, &metadata) => {
                    found.note(referent.level, &held, location.as_ref(), &metadata);
                }
                _ => unlinked.push(referent),
            }
        }
        if unlinked.is_empty() {
            return Ok(found);
        }
        if !destination
            .source()
            .capabilities()
            .filter_by_origin
            .is_native()
        {
            let mut levels: Vec<Level> = unlinked.iter().map(|referent| referent.level).collect();
            levels.sort();
            levels.dedup();
            self.walk_counterparts(destination, &levels, &mut found)
                .await?;
            return Ok(found);
        }
        let mut asked: BTreeSet<(Level, String)> = BTreeSet::new();
        for referent in unlinked {
            for key in referent.keys() {
                if asked.insert((referent.level, key.clone())) {
                    self.copied_from(destination, referent.level, &key, &mut found)
                        .await?;
                }
            }
        }
        Ok(found)
    }

    /// Note in `found` every record of `destination` at `level` recording `origin` as its
    /// origin, asked of the destination as that one question and walked to its end.
    async fn copied_from(
        &self,
        destination: &ResolvedSource,
        level: Level,
        origin: &str,
        found: &mut Counterparts,
    ) -> Result<(), EngineError> {
        let mut cursor: Option<Cursor> = None;
        let tasks = TaskQuery {
            origin: Some(origin.to_owned()),
            ..TaskQuery::default()
        };
        // Every cursor already sent, so a source cycling through several is caught as one
        // repeating the last is.
        let mut asked_before: BTreeSet<String> = BTreeSet::new();
        loop {
            if let Some(next) = &cursor
                && !asked_before.insert(next.0.clone())
            {
                return Err(refused(
                    destination,
                    SourceError::Malformed {
                        message: "the source returned a cursor it had already been given while \
                                  it was being asked for the records copied from one origin, so \
                                  the walk would never end"
                            .to_owned(),
                    },
                ));
            }
            let asked = cursor.clone();
            let request = request_for(destination, cursor);
            let source = destination.source();
            let (held, next): (Vec<HeldRecord>, _) = match level {
                Level::Task => {
                    let page = source.query_tasks(&tasks, &request).await;
                    let page = page.map_err(|error| refused(destination, error))?;
                    let next = page.next;
                    let held = page
                        .items
                        .into_iter()
                        .map(|task| (task.id, task.location, task.metadata))
                        .collect();
                    (held, next)
                }
                Level::Project => {
                    let page = source.query_projects_by_origin(origin, &request).await;
                    let page = page.map_err(|error| refused(destination, error))?;
                    let next = page.next;
                    let held = page
                        .items
                        .into_iter()
                        .map(|project| (project.id, project.location, project.metadata))
                        .collect();
                    (held, next)
                }
                Level::Document => {
                    let page = source.query_documents_by_origin(origin, &request).await;
                    let page = page.map_err(|error| refused(destination, error))?;
                    let next = page.next;
                    let held = page
                        .items
                        .into_iter()
                        .map(|document| (document.id, document.location, document.metadata))
                        .collect();
                    (held, next)
                }
            };
            fits(held.len(), request.limit).map_err(|error| refused(destination, error))?;
            for (id, location, metadata) in &held {
                found.note(level, id, location.as_ref(), metadata);
            }
            unrepeated(
                next.as_ref(),
                asked.as_ref(),
                "the destination was being asked for the records copied from one origin a \
                 document's references name",
            )
            .map_err(|error| refused(destination, error))?;
            match next {
                Some(next) => cursor = Some(next),
                None => return Ok(()),
            }
        }
    }

    /// Walk the destination once for every record it holds at the levels named, one page
    /// at a time — what [`Self::counterparts`] asks of a destination that cannot answer a
    /// question about one origin without listing everything anyway.
    ///
    /// One page is *read* at a time, and what is kept from each is three fields of each
    /// record, as [`Self::counterparts`] keeps — never the page. One pass answers every
    /// referent of the invocation at those levels, so what it learns outlives the page it
    /// learned it from.
    async fn walk_counterparts(
        &self,
        destination: &ResolvedSource,
        levels: &[Level],
        found: &mut Counterparts,
    ) -> Result<(), EngineError> {
        for level in levels {
            // Every cursor this level has already been sent. `unrepeated` below catches a
            // source that hands back the cursor it was just given, and its own note says
            // why it catches no more than that: a source cycling through two cursors
            // advances on every page, and seeing it needs memory the walks that share that
            // helper do not keep. This walk does keep memory — it is building an index that
            // outlives each page — so here the memory exists and the cycle is caught. The
            // walks one level up catch the same defect as a page token handed back
            // unchanged; this one pages by the source's own cursor and has no level above
            // it, so nothing else would.
            let mut asked_before: BTreeSet<String> = BTreeSet::new();
            let mut cursor: Option<Cursor> = None;
            loop {
                if let Some(next) = &cursor
                    && !asked_before.insert(next.0.clone())
                {
                    return Err(refused(
                        destination,
                        SourceError::Malformed {
                            message: "the source returned a cursor it had already been \
                                      given while the destination was being walked for the \
                                      records a document's references name, so the walk \
                                      would never end"
                                .to_owned(),
                        },
                    ));
                }
                let asked = cursor.clone();
                let request = request_for(destination, cursor);
                let next = match level {
                    Level::Task => {
                        let page = destination
                            .source()
                            .query_tasks(&TaskQuery::default(), &request)
                            .await
                            .map_err(|error| refused(destination, error))?;
                        fits(page.items.len(), request.limit)
                            .map_err(|error| refused(destination, error))?;
                        for task in &page.items {
                            found.note(*level, &task.id, task.location.as_ref(), &task.metadata);
                        }
                        page.next
                    }
                    Level::Project => {
                        let page = destination
                            .source()
                            .query_projects(&ProjectQuery::default(), &request)
                            .await
                            .map_err(|error| refused(destination, error))?;
                        fits(page.items.len(), request.limit)
                            .map_err(|error| refused(destination, error))?;
                        for project in &page.items {
                            found.note(
                                *level,
                                &project.id,
                                project.location.as_ref(),
                                &project.metadata,
                            );
                        }
                        page.next
                    }
                    Level::Document => {
                        let page = destination
                            .source()
                            .query_documents(&DocumentQuery::default(), &request)
                            .await
                            .map_err(|error| refused(destination, error))?;
                        fits(page.items.len(), request.limit)
                            .map_err(|error| refused(destination, error))?;
                        for document in &page.items {
                            found.note(
                                *level,
                                &document.id,
                                document.location.as_ref(),
                                &document.metadata,
                            );
                        }
                        page.next
                    }
                };
                unrepeated(
                    next.as_ref(),
                    asked.as_ref(),
                    "the destination was being walked for the records a document's \
                     references name",
                )
                .map_err(|error| refused(destination, error))?;
                match next {
                    Some(next) => cursor = Some(next),
                    None => break,
                }
            }
        }
        Ok(())
    }

    /// Destination tasks filed under the copied project whose origin the source no longer
    /// holds.
    ///
    /// Asked of the destination project's own members — a query narrowed to that one project
    /// — so what it costs is that project's tasks and never the rest of the store; each answer
    /// is still held to its filing here, so a source that answers the wider set is read
    /// exactly as before. A copy never deletes, so each is left exactly as it is and reported.
    async fn orphans(
        &self,
        destination: &ResolvedSource,
        project: &GlobalId,
        at_destination: &NativeId,
        copied: &[GlobalId],
    ) -> Result<Vec<CopyOutcome>, EngineError> {
        let mut orphans = Vec::new();
        let mut cursor: Option<Cursor> = None;
        let members = TaskQuery {
            project: ProjectFilter::Is(at_destination.clone()),
            ..TaskQuery::default()
        };
        loop {
            let asked = cursor.clone();
            let request = request_for(destination, cursor);
            let page: Page<Task> = destination
                .source()
                .query_tasks(&members, &request)
                .await
                .map_err(|error| refused(destination, error))?;
            fits(page.items.len(), request.limit).map_err(|error| refused(destination, error))?;
            for task in &page.items {
                if task.project.as_ref() != Some(at_destination) {
                    continue;
                }
                let Some(origin) = origin_of(&task.metadata) else {
                    continue;
                };
                if origin.source != project.source || copied.contains(&origin) {
                    continue;
                }
                orphans.push(CopyOutcome {
                    body_size: None,
                    sections: SectionLinks::default(),
                    status_condition: None,
                    priority_condition: None,
                    assignee_condition: None,
                    held: None,
                    expected: ExpectedWrite::default(),
                    source: origin,
                    action: CopyAction::Orphaned {
                        destination: GlobalId::new(destination.name().clone(), task.id.clone()),
                    },
                    placed: None,
                });
            }
            unrepeated(
                page.next.as_ref(),
                asked.as_ref(),
                "the destination was being read for items the copy left behind",
            )
            .map_err(|error| refused(destination, error))?;
            match page.next {
                Some(next) => cursor = Some(next),
                None => return Ok(orphans),
            }
        }
    }

    /// Make each planned item's status what its destination will store — see
    /// `TaskSource::stored_status` — so it is compared and written in that form. Its content is
    /// sent as composed: what the destination stores of it is read off the write's own answer.
    async fn store_statuses(&self, planned: &mut [Planned]) -> Result<(), EngineError> {
        for item in planned {
            let destination = self.writable(&item.to)?;
            if item.status_policy == StatusPolicy::PreserveDestination && item.held.is_some() {
                continue;
            }
            match &mut item.item {
                Item::Task(task) => {
                    task.status = destination
                        .source()
                        .stored_status(ItemKind::Task, &task.status)
                        .await
                        .map_err(|error| refused(destination, error))?;
                }
                Item::Project(project) => {
                    project.status = destination
                        .source()
                        .stored_status(ItemKind::Project, &project.status)
                        .await
                        .map_err(|error| refused(destination, error))?;
                }
                Item::Document(_) => {}
            }
        }
        Ok(())
    }

    /// Resolve and write every item planned, holding back the ones whose edges are not
    /// resolvable yet.
    ///
    /// An edge between two items of one copy can point at a member whose destination id
    /// does not exist until it has been created, so the item that points at it lands
    /// without that edge and is handed to `deferred`. [`Engine::repair`] finishes it once
    /// the *whole* request has landed — not once this call has, because the far end may
    /// be in another project of the same command.
    async fn copy_items(
        &self,
        request: &CopyRequest,
        mut planned: Vec<Planned>,
        project: Option<&Filing>,
        running: &mut Running,
        journal: &mut Journal,
    ) -> Result<Vec<CopyOutcome>, EngineError> {
        // Only a document's own content names other records, and only once every document
        // of this call has been read: the destination is walked once for all of them, and
        // the content the rest of this call lands is the rewritten one — which is what
        // makes a repeat copy of an already-rewritten document report `unchanged`.
        if !running.preflighted {
            for item in &mut planned {
                self.prepare_public_body(item, running).await?;
            }
        }
        self.rewrite_references(
            &mut planned,
            &running.resolvable,
            running.carries_work,
            &mut running.references,
            None,
        )
        .await?;
        // Each record a body links to is written before the body, so a link is addressable when
        // its body is first written rather than finished by a write of its own.
        let planned = targets_first(planned, false);
        // Reported in that order: a task written ahead of another only so its edges land with
        // its own create moves no line of the report.
        let reported: Vec<GlobalId> = planned.iter().map(|item| item.source.clone()).collect();
        let mut planned = targets_first(planned, true);
        self.note_derived_relations(&mut planned)?;
        // Once a document's references point where they land, and before anything is compared
        // with the destination or written to it: every status is what its destination will
        // store, so an unchanged item reads as unchanged.
        let mut refused_for_status = decide_conditions(request, &mut planned);
        // Each task's assignees as it lands, on the same read: one whose source holds several
        // is left out of a task copy below, and refuses a project copy whole.
        for (index, error) in self.decide_assignees(request, &mut planned)? {
            if request.scope != CopyScope::Tasks || project.is_some() {
                return Err(error);
            }
            refused_for_status.entry(index).or_insert(error);
        }
        self.store_statuses(&mut planned).await?;
        // A task copy carries every task past one its destination cannot represent: each is
        // asked about before anything is written, and one refused is left out — reported
        // `failed` with the destination's own reason — and read by every other task as an item
        // outside the copy. A project copy is still one write, whole or not at all.
        let mut left_out: Vec<(usize, CopyOutcome)> = Vec::new();
        if request.scope == CopyScope::Tasks && project.is_none() {
            let mut kept = Vec::with_capacity(planned.len());
            for (index, item) in planned.into_iter().enumerate() {
                let refusal = match refused_for_status.remove(&index) {
                    Some(error) => Some(error),
                    None => self.unrepresentable(&item, running).await?,
                };
                match refusal {
                    None => kept.push(item),
                    Some(error) => {
                        running.resolvable.retain(|copied| copied != &item.source);
                        running.counterparts.remove(&item.source.to_string());
                        let mut outcome = failed(item.source.clone(), &error);
                        outcome.status_condition = item.status_condition;
                        outcome.priority_condition = item.priority_condition;
                        outcome.assignee_condition = item.assignee_condition.clone();
                        outcome.held = item.held.as_ref().map(|held| item_result(&held.item));
                        left_out.push((index, outcome));
                    }
                }
            }
            planned = kept;
        }
        for item in &planned {
            if let Target::Update { id, .. } = &item.target {
                running.counterparts.insert(
                    item.source.to_string(),
                    GlobalId::new(item.to.clone(), id.clone()),
                );
            }
            if let Item::Task(task) = &item.item {
                running.delivers_rewritten +=
                    members_named(&task.delivers, &item.source.source, &running.resolvable);
            }
        }

        // A task skips only once every counterpart this copy found is known, because what it
        // would write depends on the copied set as well as on its source: an edge to a member of
        // the set names that member's counterpart, and one to anything else names it as its
        // source does. So beside the two tokens, the edges and the `delivers` it would write
        // have to be the ones the destination holds.
        //
        // Only a `--comments` copy and a copy by query skip at all. A copy by id without
        // `--comments` reads and compares every task exactly as it did before there were
        // tokens, so what it costs is what it cost then; it still records itself on every
        // task it writes, for the next copy that can skip.
        let skipping = request.comments || request.items.query().is_some();
        for item in planned.iter_mut().filter(|_| skipping) {
            let (Item::Task(task), Target::Update { .. }, Some(held), false) =
                (&item.item, &item.target, &item.held, item.skipped)
            else {
                continue;
            };
            if task.version.is_none() {
                continue;
            }
            let Item::Task(held_task) = &held.item else {
                continue;
            };
            let edges = self.shown_edges(
                &item.to,
                &item.source,
                mapped_edges(
                    &item.edges,
                    &item.source.source,
                    &item.to,
                    &running.resolvable,
                    &running.counterparts,
                ),
                &running.private,
            );
            let delivers = self.shown_delivers(
                &item.to,
                delivers_of(item, &item.to, &running.resolvable, &running.counterparts),
                &running.private,
            );
            if edges.iter().any(Option::is_none) || delivers.iter().any(Option::is_none) {
                continue;
            }
            let as_written = same_edges(&held.edges, &resolved(&edges))
                && targets(&held_task.delivers, &item.to)
                    == targets(&resolved_entries(&delivers), &item.to);
            // A previous preserve-destination copy can record the same source token while
            // retaining a different status. Switching back to source must still write it.
            let status_matches = item.status_policy == StatusPolicy::PreserveDestination
                || self
                    .writable(&item.to)?
                    .source()
                    .status_as_written(ItemKind::Task, &task.status)
                    == held_task.status;
            // And likewise a priority a guarded copy kept beside the same source token.
            let priority_matches = item.keeps_priority() || task.priority == held_task.priority;
            // And an assignee the copy writes, which the counterpart has to hold already.
            let assignees_match = item.assignees.as_ref().is_none_or(|written| {
                super::assignees::same_people(
                    written,
                    held_task.assignees.as_deref().unwrap_or_default(),
                )
            });
            // A counterpart an earlier release landed lacks the mark a copy now leaves on a task
            // whose edges are all on its record, and is written once more to carry it.
            let marked = self.declared(&item.to) == crate::config::SourceVisibility::Private
                || edges.len() != item.edges.len()
                || held_task
                    .metadata
                    .contains_key(MetadataKey::EDGES_ON_RECORD_KEY);
            item.skipped = status_matches
                && priority_matches
                && assignees_match
                && marked
                && self.token_covers_a_copy(&item.source.source, item.carried.is_some())
                && as_written
                && skips(task, held, item.carried.is_some(), &item.to);
        }
        // What each task's comments come to at the destination, decided from the two reads
        // already in hand before anything is written: the record a task lands with is a digest
        // of the comments it will hold.
        for item in &mut planned {
            if let (Item::Task(task), Some(carried), false) =
                (&item.item, &item.carried, item.skipped)
            {
                let landing = match &item.target {
                    Target::Update { id, .. } => Some(GlobalId::new(item.to.clone(), id.clone())),
                    Target::Create => None,
                };
                let held = item
                    .held
                    .as_ref()
                    .and_then(|held| held.comments.clone())
                    .unwrap_or_default();
                item.comment_plan = Some(comments::plan(
                    &item.source,
                    origin_of(&task.metadata).as_ref(),
                    carried,
                    landing.as_ref(),
                    &held,
                ));
            }
        }
        // Once the content that lands is settled, and before anything — a member project
        // included — is written. A task skipped writes nothing, so it has nothing to hold.
        if !running.preflighted {
            let rewrite = self
                .preflight_copy(
                    planned.iter().filter(|item| !item.skipped).collect(),
                    running,
                )
                .await?;
            for item in &mut planned {
                if self.declared(&item.to) != crate::config::SourceVisibility::Private {
                    rewritten(item, &rewrite)?;
                }
            }
            // As the policy passed it, and before anything — an intent at the source included
            // — is written.
            self.size_bodies(request, &planned.iter().collect::<Vec<_>>(), running)?;
        }

        // Every create this call makes is recorded as an intent on the item it copies before
        // the first destination write any of them sends, so a retry finds what it made.
        if !request.dry_run && !request.no_link {
            for item in &mut planned {
                // A file on this machine is kept as it was, byte for byte, so a copy that takes
                // everything back leaves it exactly so.
                let snapshot = self.readable(&item.source.source).ok().and_then(|source| {
                    source
                        .source()
                        .item_snapshot(item.item.level().record(), &item.source.native)
                });
                if self.intend(item).await? {
                    if matches!(
                        item.target,
                        Target::Update {
                            found: Found::Intent,
                            ..
                        }
                    ) {
                        journal.owned.push((
                            item.item.level(),
                            item.source.clone(),
                            item.to.clone(),
                        ));
                    }
                    journal.intents.push((
                        item.item.level(),
                        item.source.clone(),
                        item.to.clone(),
                        snapshot,
                        item.intent.clone(),
                    ));
                }
            }
        }
        intent::stop_at("after-intent");

        if project.is_none() {
            self.pin_filing_projects(&planned, running).await;
        }
        // Resolved once per item, and used by both passes: the repair pass writes the
        // same item again, and re-deriving this there could file it somewhere else.
        let mut filed = Vec::new();
        for item in &planned {
            filed.push(match (project, &item.item) {
                (_, Item::Project(_)) => None,
                // A task skipped is written nowhere, so it is filed nowhere either.
                (None, _) if item.skipped => None,
                (Some(filing), _) => filing.get(&item.to).cloned(),
                (None, _) => self.filed(request, item, None, running, journal).await?,
            });
        }

        // A task or a document copied on its own joins whatever project it is filed under at
        // the destination, which says who may read it too: a private one is never filed under
        // a project held public there, and a document filed under a private project is private
        // — a task keeps its own classification, whatever project it is filed under. A dry
        // run reads that project too, so it reports the refusal its copy would meet.
        if project.is_none() {
            for (item, filed) in planned.iter_mut().zip(&filed) {
                if let Some(filed) = filed
                    && !item.skipped
                    && (self.boundary_active() || item.classification == Classification::Private)
                {
                    let destination = self.writable(&item.to)?;
                    item.classification = self
                        .filed_under(
                            destination,
                            filed,
                            &item.source.to_string(),
                            item.classification,
                            match item.item {
                                Item::Task(_) => super::boundary::Held::Task,
                                Item::Project(_) => super::boundary::Held::Project,
                                Item::Document(_) => super::boundary::Held::Document,
                            },
                            super::boundary::Absent::Carried,
                        )
                        .await?;
                    let declared = self.declared(&item.to);
                    if item.classification == Classification::Private
                        && declared != crate::config::SourceVisibility::Private
                    {
                        return Err(EngineError::NotPrivateDestination {
                            item: item.source.to_string(),
                            destination: item.to.to_string(),
                            declared,
                        });
                    }
                }
            }
        }

        for item in &planned {
            note_private(running, item);
        }

        // Tasks this call creates at a destination that creates several in one request are
        // created there together, before anything of this call lands — by a copy that cannot
        // skip a task, since a task created together with others lands without the record a
        // copy that can skip — `--comments`, or by query — leaves for the next one.
        let created = if request.dry_run
            || self.expects_copy()
            || self.holds_copy()
            || Stamping::of(request, false) == Stamping::Always
        {
            BTreeMap::new()
        } else {
            self.create_together(&planned, &filed, running, journal)
                .await?
        };

        let mut outcomes = Vec::new();
        let mut unresolved = Vec::new();
        let mut priors = Vec::new();
        for (index, item) in planned.iter().enumerate() {
            let destination = self.writable(&item.to)?;
            let edges = self.shown_edges(
                &item.to,
                &item.source,
                mapped_edges(
                    &item.edges,
                    &item.source.source,
                    &item.to,
                    &running.resolvable,
                    &running.counterparts,
                ),
                &running.private,
            );
            let delivers = self.shown_delivers(
                &item.to,
                delivers_of(item, &item.to, &running.resolvable, &running.counterparts),
                &running.private,
            );
            if !item.skipped
                && (edges.iter().any(Option::is_none) || delivers.iter().any(Option::is_none))
            {
                unresolved.push(index);
            }
            let resolvable_delivers = resolved_entries(&delivers);
            let landed = self
                .land(
                    destination,
                    request,
                    item,
                    filed[index].clone(),
                    Pointing {
                        edges: &edges,
                        delivers: &resolvable_delivers,
                    },
                    created.get(&index).cloned(),
                    journal,
                )
                .await;
            let (outcome, prior) = match landed {
                Ok(landed) => landed,
                Err(error) => {
                    return Err(self
                        .stale_filing(destination, item, filed[index].as_ref(), running, error)
                        .await);
                }
            };
            if let Some(id) = outcome.destination()
                && !request.dry_run
                && let Item::Task(_) = &item.item
            {
                if self.declared(&item.to) == crate::config::SourceVisibility::Private {
                    let held = match item.held.as_ref().map(|held| &held.item) {
                        Some(Item::Task(held)) => {
                            dependents::recorded(&held.metadata).map_err(|message| {
                                refused(
                                    destination,
                                    SourceError::Malformed {
                                        message: format!("task {id}: {message}"),
                                    },
                                )
                            })?
                        }
                        _ => Dependents::default(),
                    };
                    running.recorders.insert(id.to_string(), (id.clone(), held));
                } else {
                    // A task this copy created is never asked about: nothing earlier can have
                    // recorded it, and an earlier copy that kept an edge of a task on a private
                    // side took the mark off it in the same write.
                    let asks_recorders = match item.held.as_ref().map(|held| &held.item) {
                        Some(Item::Task(held)) => {
                            held.metadata
                                .contains_key(MetadataKey::COPIED_DESTINATION_VERSION_KEY)
                                && !held.metadata.contains_key(MetadataKey::EDGES_ON_RECORD_KEY)
                        }
                        Some(_) => true,
                        None => !matches!(outcome.action, CopyAction::Created { .. }),
                    };
                    running.dependents.push(Dependent {
                        destination: id.clone(),
                        origin: item.source.source.clone(),
                        edges: item.edges.clone(),
                        asks_recorders,
                    });
                }
            }
            if let Some(id) = outcome.destination() {
                running
                    .counterparts
                    .insert(item.source.to_string(), id.clone());
                if item.classification == Classification::Private && !running.private.contains(id) {
                    running.private.push(id.clone());
                }
                if !request.dry_run {
                    let metadata = described(&item.item).1;
                    running.linking.push(Linking {
                        item: item.source.clone(),
                        level: item.item.level(),
                        found: item.target.found().cloned(),
                        destination: id.clone(),
                        held: metadata.get(MetadataKey::COPIES_KEY).cloned(),
                        held_locations: metadata.get(MetadataKey::COPY_LOCATIONS_KEY).cloned(),
                        location: landed_location(destination, item, &id.native),
                        intent: item.intent.is_some()
                            || !matches!(intent::held(metadata, &item.to), Ok(None)),
                    });
                }
            }
            if !request.dry_run
                && !item.skipped
                && let (Item::Task(task), Some(landed)) = (&item.item, outcome.destination())
            {
                running.landed.push(LandedTask {
                    destination: landed.clone(),
                    origin: item.source.source.clone(),
                    delivers: task.delivers.clone(),
                    before: match prior.as_ref().map(|prior| &prior.item) {
                        Some(Item::Task(held)) => targets(&held.delivers, destination.name()),
                        _ => Vec::new(),
                    },
                    category: task.status.category,
                    classification: item.classification,
                });
            }
            outcomes.push(outcome);
            priors.push(prior);
        }

        if !request.dry_run {
            for (index, item) in planned.into_iter().enumerate() {
                if !unresolved.contains(&index) {
                    continue;
                }
                // Every item a copy that is not a dry run lands has a destination id: the
                // one outcome without one is a dry run that would have created, and this
                // block does not run for a dry run.
                let id = outcomes[index]
                    .destination()
                    .expect("a copy that writes lands every item it planned")
                    .clone();
                running.deferred.push(Deferred {
                    item,
                    filed: filed[index].clone(),
                    destination: id.native,
                    prior: priors[index].clone(),
                });
            }
        }
        // Each task left out answers where it was planned, among the rest.
        for (index, outcome) in left_out {
            outcomes.insert(index.min(outcomes.len()), outcome);
        }
        outcomes.sort_by_key(|outcome| {
            reported
                .iter()
                .position(|source| source == &outcome.source)
                .unwrap_or(usize::MAX)
        });
        Ok(outcomes)
    }

    /// Stamp `landing`, a task created together with others at a destination whose creates
    /// together carry the copy's record, exactly as [`Self::write`] stamps a create of it: the
    /// mark that its every edge is on its own record, and the record of this copy — so the write
    /// the copy then makes of it says what its create already did.
    fn stamp_created(
        &self,
        destination: &ResolvedSource,
        item: &Planned,
        suggested: &NativeId,
        edges: &[DependencyEdge],
        landing: &mut Item,
        journal: &mut Journal,
    ) {
        // Tasks are created together only by a copy that does not always record itself — one
        // that does creates each on its own — so this is the stamping its write of each uses.
        let stamping = Stamping::WithCarrier;
        let Item::Task(task) = landing else {
            return;
        };
        let stamped = stamping.stamps(task, None);
        if self.declared(destination.name()) != crate::config::SourceVisibility::Private {
            let recorded = stamped
                || task
                    .metadata
                    .contains_key(MetadataKey::COPIED_DESTINATION_VERSION_KEY);
            if edges.len() != item.edges.len() {
                task.metadata.remove(MetadataKey::EDGES_ON_RECORD_KEY);
            } else if recorded {
                task.metadata
                    .insert(MetadataKey::EDGES_ON_RECORD_KEY.to_owned(), Value::from(1));
            }
        }
        if stamped {
            let digested = self.digested_comments(destination, item, suggested, journal);
            let record = record_of(item, landing, edges, destination.name(), None, digested);
            record.stamp(metadata_of(landing));
        }
    }

    /// Create, together, the tasks of `planned` that land as new items at a destination creating
    /// several in one request — its [`create_batch_limit`](onetaskgraph_plugin_api::TaskSource::create_batch_limit)
    /// over one — answering each one's index in `planned` with the id it was created under.
    ///
    /// Each lands as `write` would land it, with the edges and the tasks it delivers already
    /// nameable and without a record of the copy, since its edges to the others cannot be named
    /// until they exist; every one is journalled as created, and every id is a counterpart from
    /// here on, so the write each one is landed with next names those edges. A task carrying
    /// assets, or skipped, is landed on its own as before.
    async fn create_together(
        &self,
        planned: &[Planned],
        filed: &[Option<NativeId>],
        running: &mut Running,
        journal: &mut Journal,
    ) -> Result<BTreeMap<usize, NativeId>, EngineError> {
        let mut together: Vec<(SourceName, Vec<usize>)> = Vec::new();
        for (index, item) in planned.iter().enumerate() {
            // At a destination whose creates together carry the copy's record, a task depending
            // on another of this copy, or linking to one, is created on its own, after what it
            // names, so its edges and links land with its create rather than in a second write.
            let whole = self
                .writable(&item.to)
                .is_ok_and(|destination| destination.source().records_created_together());
            if !(matches!(item.item, Item::Task(_))
                && matches!(item.target, Target::Create)
                && !item.skipped
                && item.assets.is_empty()
                && !(whole
                    && planned
                        .iter()
                        .any(|other| depends_on(item, other) || body_links_to(item, other))))
            {
                continue;
            }
            match together.iter_mut().find(|(name, _)| name == &item.to) {
                Some((_, indices)) => indices.push(index),
                None => together.push((item.to.clone(), vec![index])),
            }
        }
        let mut created = BTreeMap::new();
        for (name, indices) in together {
            let destination = self.writable(&name)?;
            let limit = destination.source().create_batch_limit();
            if limit < 2 || indices.len() < 2 {
                continue;
            }
            for chunk in indices.chunks(limit) {
                let mut writes = Vec::with_capacity(chunk.len());
                for index in chunk {
                    let item = &planned[*index];
                    let edges = resolved(&self.shown_edges(
                        &item.to,
                        &item.source,
                        mapped_edges(
                            &item.edges,
                            &item.source.source,
                            &item.to,
                            &running.resolvable,
                            &running.counterparts,
                        ),
                        &running.private,
                    ));
                    let delivers = resolved_entries(&self.shown_delivers(
                        &item.to,
                        delivers_of(item, &item.to, &running.resolvable, &running.counterparts),
                        &running.private,
                    ));
                    let project = filed[*index].clone();
                    let suggested = created_id(&item.item, project.as_ref());
                    let mut landing = as_written(
                        destination,
                        item,
                        outgoing(
                            item,
                            suggested.clone(),
                            project.clone(),
                            &recorded(item, None),
                            &delivers,
                            None,
                        ),
                    );
                    if destination.source().records_created_together() {
                        self.stamp_created(
                            destination,
                            item,
                            &suggested,
                            &edges,
                            &mut landing,
                            journal,
                        );
                    }
                    self.at_write(
                        destination,
                        &item.source.to_string(),
                        &WriteTarget::New {
                            repositories: repositories_of(&item.item),
                            project: project.as_ref(),
                        },
                    )
                    .await?;
                    check_status(destination, &landing, None).await?;
                    let Item::Task(task) = landing else {
                        unreachable!("only tasks are created together");
                    };
                    writes.push(ItemWrite {
                        target: None,
                        item: *task,
                        depends_on: edges,
                    });
                }
                for index in chunk {
                    let item = &planned[*index];
                    journal.sending.push(item.source.clone());
                    if item.intent.is_some() {
                        journal.owned.push((
                            item.item.level(),
                            item.source.clone(),
                            destination.name().clone(),
                        ));
                    }
                }
                let owned = Self::owned_at(journal, None, destination.name())
                    .into_iter()
                    .filter(|(_, owner, _)| {
                        chunk.iter().any(|index| &planned[*index].source == owner)
                    })
                    .collect();
                let ids = self
                    .guarded(owned, journal, destination.source().write_tasks(&writes))
                    .await?
                    .map_err(|error| create_refused(destination, Creating::Tasks, error))?;
                journal
                    .sending
                    .retain(|sending| chunk.iter().all(|index| &planned[*index].source != sending));
                let distinct: BTreeSet<&NativeId> = ids.iter().collect();
                if ids.len() != chunk.len() || distinct.len() != ids.len() {
                    return Err(create_refused(
                        destination,
                        Creating::Tasks,
                        SourceError::Malformed {
                            message: format!(
                                "the source answered {} ids, {} of them distinct, for the {} \
                                 tasks it was asked to create together",
                                ids.len(),
                                distinct.len(),
                                chunk.len()
                            ),
                        },
                    ));
                }
                for (index, id) in chunk.iter().zip(ids) {
                    journal.record(Undo::Created {
                        at: destination.name().clone(),
                        kind: Level::Task,
                        id: id.clone(),
                    });
                    running.counterparts.insert(
                        planned[*index].source.to_string(),
                        GlobalId::new(destination.name().clone(), id.clone()),
                    );
                    journal
                        .landed
                        .insert((Level::Task, planned[*index].source.clone()), id.clone());
                    created.insert(*index, id);
                }
            }
        }
        Ok(created)
    }

    /// Decide, for every task of `planned`, the assignees the copy writes, on the reads already
    /// in hand: none unless both its source and its destination hold assignees; the source's
    /// for a create; and for an update only while `--when-destination-assignees` names exactly
    /// the people the counterpart holds — recorded as its `assignee_condition` — the
    /// counterpart keeping its own otherwise. Answers, by index, every task whose source holds
    /// more than the one person a write names, where the copy would write them.
    fn decide_assignees(
        &self,
        request: &CopyRequest,
        planned: &mut [Planned],
    ) -> Result<HashMap<usize, EngineError>, EngineError> {
        let wanted = request.conditions.assignees.as_deref();
        let mut refused = HashMap::new();
        for (index, item) in planned.iter_mut().enumerate() {
            let Item::Task(task) = &item.item else {
                continue;
            };
            let held = match (&item.target, item.held.as_ref().map(|held| &held.item)) {
                (Target::Update { .. }, Some(Item::Task(held))) => Some(held),
                (Target::Update { .. }, _) => continue,
                (Target::Create, _) => None,
            };
            let holds = self
                .writable(&item.to)?
                .source()
                .capabilities()
                .assignees
                .is_native();
            if let (Some(held), Some(wanted)) = (held, wanted) {
                let held = held.assignees.clone().unwrap_or_default();
                let met = super::assignees::same_people(&held, wanted);
                item.assignee_condition = Some(super::AssigneeCondition { held, met });
            }
            let writes = match held {
                None => true,
                Some(_) => item
                    .assignee_condition
                    .as_ref()
                    .is_some_and(|found| found.met),
            };
            let (true, true, Some(people)) = (holds, writes, task.assignees.as_ref()) else {
                continue;
            };
            match super::assignees::one_person(&item.source.to_string(), people) {
                Ok(()) => item.assignees = Some(people.clone()),
                Err(error) => {
                    refused.insert(index, error);
                }
            }
        }
        Ok(refused)
    }

    /// Why the destination cannot represent `item` — a task whose write it refuses at
    /// [`TaskSource::check_task_write`](onetaskgraph_plugin_api::TaskSource::check_task_write) —
    /// or `None` when it can, or when `item` is no task.
    ///
    /// The write asked about is the one this copy would send first: the task as it lands, with
    /// the edges and the tasks it delivers that can already be named at the destination. Only a
    /// refusal leaves the task out; anything else that keeps the destination from answering
    /// fails the copy, as a failed write would.
    async fn unrepresentable(
        &self,
        item: &Planned,
        running: &Running,
    ) -> Result<Option<EngineError>, EngineError> {
        if item.skipped || !matches!(item.item, Item::Task(_)) {
            return Ok(None);
        }
        let destination = self.writable(&item.to)?;
        let target = match &item.target {
            Target::Update { id, .. } => Some(id.clone()),
            Target::Create => None,
        };
        let suggested = target
            .clone()
            .unwrap_or_else(|| created_id(&item.item, None));
        let delivers = resolved_entries(&self.shown_delivers(
            &item.to,
            delivers_of(item, &item.to, &running.resolvable, &running.counterparts),
            &running.private,
        ));
        let edges = resolved(&self.shown_edges(
            &item.to,
            &item.source,
            mapped_edges(
                &item.edges,
                &item.source.source,
                &item.to,
                &running.resolvable,
                &running.counterparts,
            ),
            &running.private,
        ));
        let Item::Task(task) = outgoing(
            item,
            suggested,
            None,
            &recorded(item, None),
            &delivers,
            None,
        ) else {
            return Ok(None);
        };
        match destination
            .source()
            .check_task_write(&ItemWrite {
                target,
                item: *task,
                depends_on: edges,
            })
            .await
        {
            Ok(()) => Ok(None),
            Err(error @ SourceError::Refused { .. }) => Ok(Some(refused(destination, error))),
            Err(error) => Err(refused(destination, error)),
        }
    }

    /// Place one item already read, read its forward edges, and decide where it is going.
    async fn plan_read(
        &self,
        request: &CopyRequest,
        read: Read,
        running: &mut Running,
    ) -> Result<Planned, EngineError> {
        let id = read.source.clone();
        let (classification, placement) = self.placement_of(request, &read, running).await?;
        let aimed = running.aimed.remove(&id.to_string());
        self.aim(
            request,
            read,
            placement,
            classification,
            aimed,
            &mut running.home_members,
        )
        .await
    }

    /// Who may read one item read for a copy, and where a copy into `request`'s destination
    /// places it: the one placement every copy and `task locate` make, so the two cannot
    /// disagree about where an item lands.
    async fn placement_of(
        &self,
        request: &CopyRequest,
        read: &Read,
        running: &mut Running,
    ) -> Result<(Classification, Placement), EngineError> {
        let classification = self.copied_classification(request, read, running).await?;
        let placement = match &read.item {
            Item::Task(task) => self.routes.place_classified(
                &request.destination,
                classification,
                &task.repositories,
            ),
            Item::Document(document) => {
                self.document_placement(request, &read.source, document, classification, running)
                    .await?
            }
            Item::Project(project) => self.routes.place_classified(
                &request.destination,
                classification,
                &project.repositories,
            ),
        };
        Ok((classification, placement))
    }

    /// Who may read one item read for a copy: its own declaration, tightened by its
    /// repositories and — for a document filed under a project, while the boundary is active —
    /// by everything that project holds at its source. A task is its own: it inherits nothing
    /// from its project.
    async fn copied_classification(
        &self,
        request: &CopyRequest,
        read: &Read,
        running: &mut Running,
    ) -> Result<Classification, EngineError> {
        let (declared, repositories, filed) = match &read.item {
            Item::Task(task) => (task.classification, &task.repositories, None),
            Item::Document(document) => (
                document.classification,
                &document.repositories,
                document.project.as_ref(),
            ),
            Item::Project(project) => (project.classification, &project.repositories, None),
        };
        let own = self.classify(declared, repositories).await;
        let filed = match filed {
            Some(filed) if self.boundary_active() => {
                match self
                    .source_project_class_noting(
                        &GlobalId::new(read.source.source.clone(), filed.clone()),
                        &mut running.project_classes,
                        Some(&mut running.home_members),
                    )
                    .await
                {
                    // Unclassified reaches a destination declared private as private, and
                    // nowhere else at all.
                    Err(EngineError::ProjectUnclassified { .. })
                        if self.declared(&request.destination)
                            == crate::config::SourceVisibility::Private =>
                    {
                        Classification::Private
                    }
                    other => other?,
                }
            }
            _ => Classification::Public,
        };
        Ok(own.strictest(filed))
    }

    /// A source project's classification, worked out once per command: see
    /// [`Engine::project_class`].
    ///
    /// While the boundary is active a project its source does not hold, or cannot read
    /// projects at all, is [`EngineError::ProjectUnclassified`], and a read that fails is
    /// refused: neither is ever answered public. A public answer worked out earlier in the
    /// command is trusted only once the project is read again and is still there, because it
    /// may have gone or become unreadable since, and a private one stays private whatever
    /// became of the project, so it is trusted without a read.
    pub(super) async fn source_project_class(
        &self,
        project: &GlobalId,
        known: &mut HashMap<GlobalId, Classification>,
    ) -> Result<Classification, EngineError> {
        self.source_project_class_noting(project, known, None).await
    }

    /// [`source_project_class`](Self::source_project_class), noting in `members` the member
    /// projects the record it reads names, so a copy that needs them reads it no second time.
    async fn source_project_class_noting(
        &self,
        project: &GlobalId,
        known: &mut HashMap<GlobalId, Classification>,
        members: Option<&mut HashMap<String, Vec<GlobalId>>>,
    ) -> Result<Classification, EngineError> {
        let cached = known.get(project).copied();
        if let Some(class) = cached
            && (class == Classification::Private || !self.boundary_active())
        {
            return Ok(class);
        }
        let source = self.readable(&project.source)?;
        // Classified afresh, the project's members are read before the project is: first by
        // the source's own classification-only read, when it holds one for this command, and
        // otherwise by listing them, a listing that may carry the project issue itself.
        let afresh = cached.is_none()
            && self.boundary_active()
            && source.source().capabilities().projects.is_native();
        let classified = if afresh {
            source
                .source()
                .member_classifications(&project.native)
                .await
                .map_err(|error| refused(source, error))?
        } else {
            None
        };
        let listed = if afresh && classified.is_none() {
            Some(self.project_member_tasks(project, FOR_A_COPY).await?)
        } else {
            None
        };
        let held = if source.source().capabilities().projects.is_native() {
            source
                .source()
                .get_project(&project.native)
                .await
                .map_err(|error| refused(source, error))?
        } else {
            None
        };
        let Some(held) = held else {
            // Nothing is inherited from a project that is not there, but while the boundary is
            // active that is not the same as inheriting nothing: what is filed under it is
            // unclassified, and that is never written down as public for a later read to trust.
            if self.boundary_active() {
                known.remove(project);
                return Err(EngineError::ProjectUnclassified {
                    project: project.to_string(),
                    why: if source.source().capabilities().projects.is_native() {
                        "its source does not hold it".to_owned()
                    } else {
                        "its source cannot read projects".to_owned()
                    },
                });
            }
            return Ok(Classification::Public);
        };
        if let (Some(members), Ok(named)) = (members, members_of(project, &held.metadata)) {
            members.insert(project.to_string(), named);
        }
        if let Some(class) = cached {
            // Read again, its record may have tightened since; nothing loosens it.
            let class =
                class.strictest(self.classify(held.classification, &held.repositories).await);
            known.insert(project.clone(), class);
            return Ok(class);
        }
        if let Some(members) = classified {
            let mut class = self.classify(held.classification, &held.repositories).await;
            for (declared, repositories) in &members {
                class = class.strictest(self.classify(*declared, repositories).await);
            }
            known.insert(project.clone(), class);
            return Ok(class);
        }
        let tasks = match listed {
            Some(tasks) => tasks,
            None => self.project_member_tasks(project, FOR_A_COPY).await?,
        };
        let class = self
            .project_class(source, project, &held, tasks.iter().map(|task| &task.item))
            .await?;
        known.insert(project.clone(), class);
        Ok(class)
    }

    /// Who may read the item `far` names, as a write naming it has to know: its own record and
    /// repositories — a task's alone, and a project's tightened by every task and document it
    /// holds. `known` holds the project classifications this command has
    /// already worked out, each confirmed by [`Engine::source_project_class`] before it is
    /// trusted. `Err` says why it could not be established: its source is not configured, a
    /// read failed, or no such item is held.
    pub(super) async fn reference_class(
        &self,
        far: &GlobalId,
        kind: ItemKind,
        known: &mut HashMap<GlobalId, Classification>,
    ) -> Result<Classification, String> {
        let Ok(source) = self.readable(&far.source) else {
            return Err("no source of that name is configured to read it from".to_owned());
        };
        let unread = |error: SourceError| format!("its source could not be read: {error}");
        let missing = || "its source holds no such item".to_owned();
        match kind {
            ItemKind::Task => {
                let task = source
                    .source()
                    .get_task(&far.native)
                    .await
                    .map_err(unread)?
                    .ok_or_else(missing)?;
                // A task is as private as its own record and repositories say, whatever the
                // project it is filed under is.
                Ok(self.classify(task.classification, &task.repositories).await)
            }
            ItemKind::Project => match self.source_project_class(far, known).await {
                Ok(class) => Ok(class),
                Err(EngineError::ProjectUnclassified { why, .. }) => Err(why),
                Err(error) => Err(error.to_string()),
            },
        }
    }

    /// A project's classification: the strictest of its own declaration, its repositories,
    /// every task in `tasks` and — while the boundary is active, at a source with documents —
    /// every document filed under it. Nothing loosens it: a project once private stays
    /// private, because what this writes it as is what the next copy reads.
    async fn project_class<'a>(
        &self,
        source: &ResolvedSource,
        project: &GlobalId,
        held: &Project,
        tasks: impl Iterator<Item = &'a Task>,
    ) -> Result<Classification, EngineError> {
        let mut class = self.classify(held.classification, &held.repositories).await;
        for task in tasks {
            class = class.strictest(self.classify(task.classification, &task.repositories).await);
        }
        if self.boundary_active() && source.source().capabilities().documents.is_native() {
            for document in self.project_documents(project).await? {
                class = class.strictest(
                    self.classify(document.item.classification, &document.item.repositories)
                        .await,
                );
            }
        }
        Ok(class)
    }

    /// `edges` as a write to `destination` carries them: one whose far end that write withholds
    /// (see [`Engine::withholds`]) — an item of a source declared private, or one of `private`,
    /// the items this copy carries classified private — written in the opaque form, its
    /// source's designator and its native id, when [`preflight_copy`](Self::preflight_copy)
    /// learned that source's designator and its id is the item's native id (see
    /// [`Engine::opaque_designator`]), and left out otherwise. One not resolvable yet stays
    /// `None`.
    ///
    /// Every edge left out is noted against `near`, the item it is an edge of, for
    /// [`Engine::take_withheld_edges`].
    fn shown_edges(
        &self,
        destination: &SourceName,
        near: &GlobalId,
        edges: Vec<Option<DependencyEdge>>,
        private: &[GlobalId],
    ) -> Vec<Option<DependencyEdge>> {
        edges
            .into_iter()
            .filter_map(|edge| {
                let Some(mut edge) = edge else {
                    return Some(None);
                };
                let Some(far) = far_end(&edge.to, destination)
                    .filter(|far| self.withholds(destination, far, private))
                else {
                    return Some(Some(edge));
                };
                let Some(designator) = self.opaque_designator(&far) else {
                    let reason =
                        if self.declared(&far.source) == crate::config::SourceVisibility::Private {
                            format!("its source {} is declared private", far.source)
                        } else {
                            "it is classified private".to_owned()
                        };
                    self.boundary.withheld(format!(
                        "withheld {near}'s dependency on {far} from {destination}: {reason}, and \
                         its source named no tracker an opaque reference to it could point into"
                    ));
                    return None;
                };
                edge.to = DependencyEndpoint::opaque(designator, far.native.0, edge.to.kind)
                    .expect("a far end's native id is not empty");
                Some(Some(edge))
            })
            .collect()
    }

    /// `delivers` as a write to `destination` carries them: without an entry naming a task
    /// that write withholds. One not resolvable yet stays `None`.
    fn shown_delivers(
        &self,
        destination: &SourceName,
        delivers: Vec<Option<TaskRef>>,
        private: &[GlobalId],
    ) -> Vec<Option<TaskRef>> {
        delivers
            .into_iter()
            .filter(|entry| {
                !entry.as_ref().is_some_and(|entry| {
                    entry
                        .in_source(destination)
                        .as_str()
                        .parse::<GlobalId>()
                        .is_ok_and(|far| self.withholds(destination, &far, private))
                })
            })
            .collect()
    }

    /// Hold every item a copy lands to the public boundary before any is written: a private
    /// one refused anywhere not declared private, and every one landing anywhere not declared
    /// private put to the caller's check. Every destination declared private is read here,
    /// once for the copy, so a dry run reports the refusal its copy would meet and the copy's
    /// writes read nothing more.
    ///
    /// While the boundary is active it first learns who may read every item outside the copy
    /// that a carried dependency or delivery names, so a write anywhere not declared private
    /// withholds the id of a private one — and refuses, before anything is written, a
    /// reference whose classification it cannot establish.
    async fn preflight_copy(
        &self,
        planned: Vec<&Planned>,
        running: &mut Running,
    ) -> Result<Rewrite, EngineError> {
        for item in &planned {
            note_private(running, item);
        }
        self.learn_designators(&planned).await?;
        if self.boundary_active() {
            for item in &planned {
                self.classify_references(item, running).await?;
            }
        }
        // Every item is placed first, and what each lands somewhere not verified private is then
        // put to the check together, before anything is written.
        let mut checked = Vec::new();
        for item in &planned {
            let destination = self.writable(&item.to)?;
            let target = match &item.target {
                Target::Update { id, .. } => WriteTarget::Existing(id),
                Target::Create => WriteTarget::New {
                    repositories: repositories_of(&item.item),
                    project: None,
                },
            };
            let exposure = exposure_of(item, |far| self.withholds(&item.to, far, &running.private));
            let outbound = Outbound {
                item: item.source.to_string(),
                classification: item.classification.strictest(
                    item.held
                        .as_ref()
                        .map_or(Classification::Public, |held| classification_of(&held.item)),
                ),
                target,
                exposure: &exposure,
            };
            if self.placed(destination, &outbound)? {
                checked.push((destination, outbound.item.clone(), exposure.clone()));
            }
            // Every item, not one per destination: who can read a write depends on where in
            // the destination it lands — an issue's own repository on a board. Read here,
            // before anything is written, and held for the copy, so its writes read nothing
            // more and a dry run reports the refusal its copy would meet.
            let parent = match &item.item {
                Item::Task(task) => task.project.as_ref(),
                Item::Document(document) => document.project.as_ref(),
                Item::Project(_) => None,
            };
            let project = parent
                .and_then(|parent| {
                    planned.iter().find(|candidate| {
                        matches!(candidate.item, Item::Project(_))
                            && candidate.source.source == item.source.source
                            && candidate.source.native == *parent
                            && candidate.to == item.to
                    })
                })
                .map(|project| match &project.target {
                    Target::Update { id, .. } => WriteTarget::Existing(id),
                    Target::Create => WriteTarget::New {
                        repositories: repositories_of(&project.item),
                        project: None,
                    },
                });
            self.verify_private_with_project(
                destination,
                &outbound.item,
                &outbound.target,
                project.as_ref(),
            )
            .await?;
        }
        self.check_all(&checked).await
    }

    /// Size the body every item of `planned` would write — each as [`Engine::write`] composes it
    /// for its destination, its copy's record and every asset reference included — before any
    /// of it is written, and refuse the whole copy, writing nothing, when any is larger than its
    /// destination holds: every one over its limit named, as [`EngineError::Oversize`].
    ///
    /// A dry run keeps each size for its report instead, and refuses nothing. An item skipped
    /// writes nothing and is not sized; a destination whose plugin cannot say is not either.
    fn size_bodies(
        &self,
        request: &CopyRequest,
        planned: &[&Planned],
        running: &mut Running,
    ) -> Result<(), EngineError> {
        let mut over = Vec::new();
        for item in planned.iter().filter(|item| !item.skipped) {
            let destination = self.writable(&item.to)?;
            let Some(size) = self.composed_size(destination, item, !request.no_link, running)
            else {
                continue;
            };
            if let (true, Some(limit)) = (size.exceeds(), size.limit) {
                over.push(Oversized {
                    source: item.source.clone(),
                    destination: item.to.clone(),
                    body_size: size.size,
                    limit,
                });
            }
            running.sizes.insert(item.source.clone(), size);
        }
        if over.is_empty() || request.dry_run {
            return Ok(());
        }
        Err(EngineError::Oversize { items: over })
    }

    /// The body one planned item's write would store at `destination`, as its plugin sizes it:
    /// the item [`outgoing`] lands, stamped with the record a copy leaves on it, with the edges
    /// and delivered tasks this copy can already name and the assets it carries.
    fn composed_size(
        &self,
        destination: &ResolvedSource,
        item: &Planned,
        linking: bool,
        running: &Running,
    ) -> Option<BodySize> {
        let held = item.held.as_ref();
        let target = match &item.target {
            Target::Update { id, .. } => Some(id.clone()),
            Target::Create => None,
        };
        let edges = resolved(&self.shown_edges(
            &item.to,
            &item.source,
            mapped_edges(
                &item.edges,
                &item.source.source,
                &item.to,
                &running.resolvable,
                &running.counterparts,
            ),
            &running.private,
        ));
        let delivers = resolved_entries(&self.shown_delivers(
            &item.to,
            delivers_of(item, &item.to, &running.resolvable, &running.counterparts),
            &running.private,
        ));
        let suggested = target
            .clone()
            .unwrap_or_else(|| created_id(&item.item, None));
        let mut landing = as_written(
            destination,
            item,
            outgoing(
                item,
                suggested,
                None,
                &recorded(item, held),
                &delivers,
                held,
            ),
        );
        // A create carries the key of the intent the copy records before it, as
        // [`Engine::intend`] decides one is recorded: its UUID, as long as every one.
        if matches!(item.target, Target::Create)
            && linking
            && !item.withheld
            && self.writable(&item.source.source).is_ok_and(|source| {
                source.source().writes().is_supported()
                    && (self.declared(&item.to) != crate::config::SourceVisibility::Private
                        || self.declared(source.name()) == crate::config::SourceVisibility::Private)
            })
        {
            metadata_of(&mut landing).insert(
                MetadataKey::CREATE_KEY.to_owned(),
                Value::String(uuid::Uuid::nil().hyphenated().to_string()),
            );
        }
        if let Item::Task(task) = &mut landing
            && self.declared(destination.name()) != crate::config::SourceVisibility::Private
            && edges.len() == item.edges.len()
        {
            task.metadata
                .insert(MetadataKey::EDGES_ON_RECORD_KEY.to_owned(), Value::from(1));
        }
        record_of(
            item,
            &landing,
            &edges,
            destination.name(),
            held.and_then(|prior| prior.recorded.as_ref()),
            None,
        )
        .stamp(metadata_of(&mut landing));
        let carrying = !item.assets.is_empty() || held.is_some_and(|prior| prior.assets.is_some());
        let assets = carrying.then(|| {
            assets::write_of(
                item.assets.clone(),
                held.and_then(|prior| prior.recorded.clone()),
            )
        });
        let source = destination.source();
        match landing {
            Item::Task(task) => source.task_body_size(
                &ItemWrite {
                    target,
                    item: *task,
                    depends_on: edges,
                },
                assets.as_ref(),
            ),
            Item::Project(project) => source.project_body_size(&ItemWrite {
                target,
                item: *project,
                depends_on: edges,
            }),
            Item::Document(document) => source.document_body_size(
                &ItemWrite {
                    target,
                    item: *document,
                    depends_on: Vec::new(),
                },
                assets.as_ref(),
            ),
        }
    }

    /// Ask, once, which tracker each source declared private is that an edge of `planned`,
    /// landing somewhere not declared private, is withheld into: the source its far end is in,
    /// or — for a far end this copy carries — the source that far end lands in. No other
    /// source is asked, so a copy no such edge crosses sends no request for one. A source that
    /// cannot say is left unknown, and an edge to it is withheld as it is for a source with no
    /// designator.
    ///
    /// A far end of a source that can say, and that the copy does not carry, is read once there
    /// to confirm its recorded id is the item's native id; one that is not — a prefixed human
    /// key, a URL, a slug, or an id the source holds no item under — is refused before anything
    /// is written, exactly as a direct write refuses it (see [`Engine::publicly_named`]), naming
    /// its form and never its value.
    ///
    /// # Errors
    ///
    /// [`EngineError::PrivateReferenceContent`], and a far end's source failing that read.
    async fn learn_designators(&self, planned: &[&Planned]) -> Result<(), EngineError> {
        let private =
            |name: &SourceName| self.declared(name) == crate::config::SourceVisibility::Private;
        let mut asked: BTreeSet<SourceName> = BTreeSet::new();
        for item in planned.iter().filter(|item| !private(&item.to)) {
            for far in item
                .edges
                .iter()
                .filter_map(|edge| far_end(&edge.to, &item.source.source))
            {
                let carried = planned.iter().find(|other| other.source == far);
                let lands = carried.map_or(far.source.clone(), |other| other.to.clone());
                if !private(&lands) {
                    continue;
                }
                if asked.insert(lands.clone()) {
                    let _ = self.designator(&lands).await;
                }
                // A far end the copy does not carry is named by the id its near item recorded,
                // which is read once at its source to confirm it is that item's native id; one
                // the copy carries is named by the id its own write is answered with.
                if carried.is_none() && self.known_designator(&lands).is_some() {
                    let kind = item
                        .edges
                        .iter()
                        .find(|edge| far_end(&edge.to, &item.source.source).as_ref() == Some(&far))
                        .map_or(ItemKind::Task, |edge| edge.to.kind);
                    if let Some(form) = self.native_form(&far, kind).await? {
                        return Err(EngineError::PrivateReferenceContent {
                            item: item.source.to_string(),
                            destination: item.to.to_string(),
                            form,
                        });
                    }
                }
            }
        }
        Ok(())
    }

    /// Learn who may read each item outside the copy that `item`'s dependencies and deliveries
    /// name, when `item` lands anywhere not declared private, recording each private one in
    /// `running.private`.
    ///
    /// An item of a source declared private, one of the destination itself, and one this copy
    /// carries need no read: the first is withheld by its source, the second names nothing
    /// elsewhere, and the third's classification is already known. Every other is read once
    /// per command, with what its classification inherits — a task's project, and a project's
    /// tasks and documents. One whose source is not configured, or where any of those reads
    /// fails or finds no such item, is refused, because an id that cannot be shown public is
    /// never written where it would name a private item.
    async fn classify_references(
        &self,
        item: &Planned,
        running: &mut Running,
    ) -> Result<(), EngineError> {
        if self.declared(&item.to) == crate::config::SourceVisibility::Private {
            return Ok(());
        }
        let origin = &item.source.source;
        let mut named: Vec<(GlobalId, ItemKind)> = item
            .edges
            .iter()
            .filter_map(|edge| far_end(&edge.to, origin).map(|far| (far, edge.to.kind)))
            .collect();
        if let Item::Task(task) = &item.item {
            named.extend(task.delivers.iter().filter_map(|entry| {
                entry
                    .in_source(origin)
                    .as_str()
                    .parse::<GlobalId>()
                    .ok()
                    .map(|far| (far, ItemKind::Task))
            }));
        }
        for (far, kind) in named {
            if far.source == item.to
                || self.declared(&far.source) == crate::config::SourceVisibility::Private
                || running.resolvable.contains(&far)
                || running.private.contains(&far)
                || running.public_references.contains(&far)
            {
                continue;
            }
            let class = self
                .reference_class(&far, kind, &mut running.project_classes)
                .await
                .map_err(|why| EngineError::ReferenceUnclassified {
                    item: item.source.to_string(),
                    reference: far.to_string(),
                    destination: item.to.to_string(),
                    why,
                })?;
            if class == Classification::Private {
                running.private.push(far);
            } else {
                running.public_references.push(far);
            }
        }
        Ok(())
    }

    /// Read one item out of its own source.
    async fn read(&self, kind: Level, id: &GlobalId) -> Result<Read, EngineError> {
        let source = self.readable(&id.source)?;
        if kind == Level::Document {
            documentary(source)?;
        }
        let item = match kind {
            Level::Task => source
                .source()
                .get_task(&id.native)
                .await
                .map_err(|error| refused(source, error))?
                .map(|task| Item::Task(Box::new(task))),
            Level::Project => source
                .source()
                .get_project(&id.native)
                .await
                .map_err(|error| refused(source, error))?
                .map(|project| Item::Project(Box::new(project))),
            Level::Document => source
                .source()
                .get_document(&id.native)
                .await
                .map_err(|error| refused(source, error))?
                .map(|document| Item::Document(Box::new(document))),
        }
        .ok_or_else(|| EngineError::NoSuchItem { id: id.to_string() })?;
        Ok(Read {
            source: id.clone(),
            item,
            comments: None,
        })
    }

    /// One item read for a copy.
    ///
    /// A task a `--comments` copy carries is read with every page of its comments as one
    /// revision of it — see [`snapshot`] — starting from the detail read made for many tasks of
    /// its source at once, when there was one, and read again from its first page once when its
    /// requests disagreed, and so is a task a copy by query narrowed by comment activity reads
    /// to confirm its match. Every other item is read as [`Engine::read`] reads it, so a copy
    /// with neither costs exactly what it did before comments could be carried.
    ///
    /// # Errors
    ///
    /// [`EngineError::ChangedWhileCopied`] for a task whose requests disagreed on both reads,
    /// and every error [`Engine::read`] returns.
    async fn read_for_copy(
        &self,
        request: &CopyRequest,
        kind: Level,
        id: &GlobalId,
        running: &mut Running,
    ) -> Result<Read, EngineError> {
        if kind != Level::Task || !reads_comments(request) {
            return self.read(kind, id).await;
        }
        let source = self.readable(&id.source)?;
        if !source.source().capabilities().comments.is_native() {
            // A source whose tasks hold no comments has none to carry.
            let mut read = self.read(kind, id).await?;
            read.comments = Some(Vec::new());
            return Ok(read);
        }
        let mut prefetched = running.prefetched.remove(&id.to_string());
        for _ in 0..2 {
            let answer = match prefetched.take() {
                Some(answer) => answer,
                None => snapshot::first(source, &id.native).await,
            };
            let read = answer
                .map_err(|error| refused(source, error))?
                .ok_or_else(|| EngineError::NoSuchItem { id: id.to_string() })?;
            match snapshot::take(source, &id.native, read)
                .await
                .map_err(|error| refused(source, error))?
            {
                snapshot::Taken::Whole(whole) => {
                    return Ok(Read {
                        source: id.clone(),
                        item: Item::Task(Box::new(whole.task)),
                        comments: Some(whole.comments),
                    });
                }
                snapshot::Taken::Gone => {
                    return Err(EngineError::NoSuchItem { id: id.to_string() });
                }
                // Already read in full more than once, so not read yet again.
                snapshot::Taken::Unsettled => {
                    return Err(EngineError::ChangedWhileCopied { item: id.clone() });
                }
                snapshot::Taken::Changed => {}
            }
        }
        Err(EngineError::ChangedWhileCopied { item: id.clone() })
    }

    /// Read the destination counterparts of every item of `reads` ahead of aiming any of
    /// them, in each source this copy can reach: what each item's own link names there, what
    /// its own origin names there, and — for a document filed under a project this command
    /// has not landed — what that project's link names, which the filing lookup reads.
    ///
    /// One [`TaskSource::read_ahead_for_copy`] per source, so a source that can batch its
    /// reads answers every item's read-before-write together, and each read by id that
    /// follows is answered from it. Nothing is decided here: a failure holds nothing, and every
    /// read it would have answered is made as it always was and reports its own failure.
    ///
    /// A copy told there is nothing to find reads nothing, a copy expecting a version of its
    /// one target leaves that read to the cache, a task read with its comments is read by the
    /// detail read that carries them, and a source with one counterpart to read reads it by
    /// itself — so none of those is read ahead.
    async fn read_ahead(&self, request: &CopyRequest, reads: &[Read], running: &mut Running) {
        if request.create || self.expects_copy() || self.holds_copy() {
            return;
        }
        let reachable = self.routes.reachable(&request.destination);
        let mut wanted: Vec<(SourceName, Vec<(MetadataRecord, NativeId)>)> = Vec::new();
        let mut origins: Vec<(SourceName, Vec<String>)> = Vec::new();
        let mut want = |there: &SourceName, record: MetadataRecord, native: NativeId| {
            let native = (record, native);
            match wanted.iter_mut().find(|(name, _)| name == there) {
                Some((_, natives)) if natives.contains(&native) => {}
                Some((_, natives)) => natives.push(native),
                None => wanted.push((there.clone(), vec![native])),
            }
        };
        // Each destination project this copy names, with the origin it records there and why
        // the copy names it: what a cache standing in front of that destination is pinned
        // from, below.
        let mut homes: Vec<(SourceName, NativeId, GlobalId, ProjectNamed)> = Vec::new();
        // The origin each document's project records at a destination it is linked to: asked
        // beside the document's own when that destination is asked about origins anyway, so the
        // project a new document is filed under is answered by that one request rather than
        // read on its own before the document is created there.
        let mut project_origins: Vec<(SourceName, GlobalId)> = Vec::new();
        // Each destination a document's project is read at for a screened write's classification.
        let mut screened_homes: Vec<SourceName> = Vec::new();
        // A copy of documents alone reads each document and its project by the one read that
        // carries them, as it does where no cache stands in front of the destination: pinning
        // every member of the project first would spend a listing of them, and a lookup of the
        // project by its origin, on reads this copy never makes.
        let documents_alone = !matches!(request.scope, CopyScope::Projects { .. })
            && reads
                .iter()
                .all(|read| read.item.level() == Level::Document);
        for read in reads {
            let metadata = described(&read.item).1;
            if read.item.level() == Level::Project {
                for there in &reachable {
                    if let Some(link) = link_of(metadata, there) {
                        homes.push((
                            there.clone(),
                            link.native,
                            read.source.clone(),
                            ProjectNamed::Copied,
                        ));
                    }
                }
            }
            if !(request.comments && read.item.level() == Level::Task) {
                for there in &reachable {
                    if let Some(link) = link_of(metadata, there) {
                        want(there, read.item.level().record(), link.native);
                    }
                    if let Some(origin) =
                        origin_of(metadata).filter(|origin| &origin.source == there)
                    {
                        want(there, read.item.level().record(), origin.native);
                    }
                }
            }
            // An item naming nothing at a destination is looked for there by its own origin, so
            // every such question is asked together, before the first of them would be — but for
            // a task whose priority that destination cannot hold, which is refused before the
            // destination is asked anything. An item of a source declared private is asked about
            // at the copy's own destination too: its own lookup in [`Engine::target`] asks that
            // destination for the same origin, since a public item there may already carry it (by
            // hand, or from a release before origins were withheld), and asking it here sends the
            // same id in one request rather than one each. It is never asked of a source that is
            // not private and that the copy reaches only by a route: what a private copy sends
            // there is a neutral view, which nothing there may name the private item to.
            let private_source =
                self.declared(&read.source.source) == crate::config::SourceVisibility::Private;
            for there in &reachable {
                if private_source
                    && there != &request.destination
                    && self.declared(there) != crate::config::SourceVisibility::Private
                {
                    continue;
                }
                let refused = match (&read.item, self.writable(there)) {
                    (Item::Task(task), Ok(destination)) => {
                        holds_priority(destination, &read.source.to_string(), task.priority)
                            .is_err()
                    }
                    _ => false,
                };
                if !refused
                    && link_of(metadata, there).is_none()
                    && origin_of(metadata).is_none_or(|origin| &origin.source != there)
                {
                    let asked = read.source.to_string();
                    match origins.iter_mut().find(|(name, _)| name == there) {
                        Some((_, asked_there)) => asked_there.push(asked),
                        None => origins.push((there.clone(), vec![asked])),
                    }
                }
            }
            // A document's own project is read at its source for its references anyway, so
            // reading it here for the filing lookup moves that read rather than adding one. A
            // task's is left to the lookup, in the order it always made it.
            let Item::Document(document) = &read.item else {
                continue;
            };
            let Some(project) = document.project.clone() else {
                continue;
            };
            let qualified = GlobalId::new(read.source.source.clone(), project);
            if running.counterparts.contains_key(&qualified.to_string())
                || reads.iter().any(|other| other.source == qualified)
            {
                continue;
            }
            // A source whose cache stands in front of it has its projects pinned by a listing
            // before the filing lookup reads them, which this read would come before.
            if self.caches(&qualified.source) {
                continue;
            }
            if !running.source_projects.contains_key(&qualified) {
                let Ok(source) = self.readable(&qualified.source) else {
                    continue;
                };
                if !source.source().capabilities().projects.is_native() {
                    continue;
                }
                // A record answered for another id is left to the filing lookup's own read.
                let Ok(Some(held)) = source.source().get_project(&qualified.native).await else {
                    continue;
                };
                if held.id != qualified.native {
                    continue;
                }
                running.source_projects.insert(qualified.clone(), held);
            }
            for there in &reachable {
                // A document with its own link there has its counterpart read anyway, and is
                // taken as filed under that project when that is where it sits — see
                // [`Engine::filed_as_linked`] — so its project is not read with it; one a person
                // moved has the project read by the filing lookup instead. Where a cache stands
                // in front of the destination and the copy writes more than documents, the
                // project is still named, because pinning it is what answers the document's own
                // read from that cache.
                if link_of(&document.metadata, there).is_some()
                    && (!self.caches(there) || documents_alone)
                {
                    continue;
                }
                if let Some(link) = link_of(&running.source_projects[&qualified].metadata, there) {
                    project_origins.push((there.clone(), qualified.clone()));
                    // A write the boundary screens there reads who may read that project, so
                    // the project is named to the read ahead even alone: a source may carry its
                    // members' classification beside the origins it is asked.
                    if self.boundary_active()
                        && self.declared(there) != crate::config::SourceVisibility::Private
                    {
                        screened_homes.push(there.clone());
                    }
                    want(there, MetadataRecord::Project, link.native.clone());
                    homes.push((
                        there.clone(),
                        link.native,
                        qualified.clone(),
                        ProjectNamed::FiledUnder,
                    ));
                }
            }
        }
        for (there, project, origin, named) in homes {
            if self.caches(&there)
                && !documents_alone
                && let Ok(destination) = self.writable(&there)
            {
                // A project the copy writes is read with its forward edges, which only a token
                // covering them could answer from the cache; where it does not, pinning the
                // project would spend a request on a read that is made anyway.
                let covered = destination
                    .source()
                    .project_version_coverage()
                    .forward_edges
                    .reason()
                    .is_none();
                let pinned = named == ProjectNamed::FiledUnder || covered;
                self.pin_members(destination, &project, pinned.then_some(&origin))
                    .await;
            }
        }
        let asked_there: Vec<SourceName> = origins.iter().map(|(name, _)| name.clone()).collect();
        let asked_origins_at = asked_there.clone();
        for (name, mut asked) in origins {
            for (there, origin) in &project_origins {
                let spelled = origin.to_string();
                if there == &name && !asked.contains(&spelled) {
                    asked.push(spelled);
                    running.asked_origins.push((there.clone(), origin.clone()));
                }
            }
            if let Ok(there) = self.writable(&name) {
                // Nothing is decided by it: each lookup it would have answered is made as before.
                let _ = there.source().read_ahead_origins_for_copy(&asked).await;
            }
        }
        // One counterpart of a task or a document copied on its own is read by its own read,
        // which costs what a batch of one does and carries what a write of it needs besides; a
        // project copy reads what its cache could not answer together, however few, and so is a
        // document's project a screened write there classifies the document by. Every
        // destination asked about origins above is handed its read ahead, empty or not, since a
        // source may hold those origins back to send with it.
        let projects = matches!(request.scope, CopyScope::Projects { .. });
        for (name, natives) in &mut wanted {
            let kept = natives.len() > 1
                || (projects && !natives.is_empty())
                || screened_homes.contains(name);
            if !kept {
                natives.clear();
            }
        }
        for name in asked_there {
            if !wanted.iter().any(|(there, _)| there == &name) {
                wanted.push((name, Vec::new()));
            }
        }
        for (name, natives) in wanted {
            if natives.is_empty() && !asked_origins_at.contains(&name) {
                continue;
            }
            if let Ok(there) = self.writable(&name) {
                // Nothing is decided by it: each read it would have answered is made as before.
                let _ = there.source().read_ahead_records_for_copy(&natives).await;
            }
        }
    }

    /// Pin, in the cache standing in front of `destination`, every member of its project
    /// `project` — and, given the `origin` its counterpart records, that project itself — by
    /// the listings that carry their change tokens: the project's own tasks and documents, the
    /// members the orphan check lists anyway, and the projects recording `origin`.
    ///
    /// So the read-before-write of every item those tokens vouch for is answered by the cache
    /// at no request, and only an item changed since its entry — or with none — is read. A
    /// listing that fails pins nothing, and every read it would have answered is made.
    async fn pin_members(
        &self,
        destination: &ResolvedSource,
        project: &NativeId,
        origin: Option<&GlobalId>,
    ) {
        let source = destination.source();
        let members = TaskQuery {
            project: ProjectFilter::Is(project.clone()),
            ..TaskQuery::default()
        };
        let mut cursor: Option<Cursor> = None;
        // A cursor already sent ends the pinning: it only pins, and every read it would have
        // answered is made anyway.
        let mut asked_before: BTreeSet<String> = BTreeSet::new();
        loop {
            let asked = cursor.clone();
            let request = request_for(destination, cursor);
            let Ok(page) = source.query_tasks(&members, &request).await else {
                break;
            };
            // A page the walk's own guards refuse ends the pinning, as a failed one does.
            if fits(page.items.len(), request.limit).is_err()
                || unrepeated(
                    page.next.as_ref(),
                    asked.as_ref(),
                    "pinning a project's members",
                )
                .is_err()
            {
                break;
            }
            match page.next {
                Some(next) if asked_before.insert(next.0.clone()) => cursor = Some(next),
                _ => break,
            }
        }
        if source.capabilities().documents.is_native() {
            let documents = DocumentQuery {
                project: ProjectFilter::Is(project.clone()),
                ..DocumentQuery::default()
            };
            let _ = source
                .query_documents(&documents, &request_for(destination, None))
                .await;
        }
        if let Some(origin) = origin {
            let _ = source
                .query_projects_by_origin(&origin.to_string(), &request_for(destination, None))
                .await;
        }
    }

    /// Read the first detail page of every task in `ids` whose comments the copy reads — to
    /// carry them, or to confirm a comment-activity query's match — as few requests as each
    /// source's own batched detail read takes, for [`Engine::read_for_copy`] to start from: each
    /// task with the first page of its comments beside it.
    ///
    /// Nothing for a copy that reads no comment, which reads each task by itself as it always
    /// did.
    async fn prefetch(&self, request: &CopyRequest, ids: &[GlobalId], running: &mut Running) {
        if !reads_comments(request) {
            return;
        }
        let mut by_source: Vec<(SourceName, Vec<NativeId>)> = Vec::new();
        for id in ids {
            if running.prefetched.contains_key(&id.to_string()) {
                continue;
            }
            match by_source.iter_mut().find(|(name, _)| name == &id.source) {
                Some((_, natives)) => {
                    if !natives.contains(&id.native) {
                        natives.push(id.native.clone());
                    }
                }
                None => by_source.push((id.source.clone(), vec![id.native.clone()])),
            }
        }
        for (name, natives) in by_source {
            let Ok(source) = self.readable(&name) else {
                continue;
            };
            if !source.source().capabilities().comments.is_native() {
                continue;
            }
            let mut answers = source
                .source()
                .get_task_details_for_copy(&natives, &snapshot::comment_page(source))
                .await;
            // One answer per id, in order, or none of them is anybody's: a short or a long batch
            // cannot say which of its answers is about which task.
            if answers.len() != natives.len() {
                let miscounted = SourceError::Malformed {
                    message: format!(
                        "the source answered {} task details for the {} tasks a copy read",
                        answers.len(),
                        natives.len()
                    ),
                };
                answers = natives.iter().map(|_| Err(miscounted.clone())).collect();
            }
            for (native, answer) in natives.into_iter().zip(answers) {
                running
                    .prefetched
                    .insert(GlobalId::new(name.clone(), native).to_string(), answer);
            }
        }
    }

    /// The edges from `id`, a task read for a copy, that a task of a source declared private
    /// records of it (see [`dependents`]): each to that task, qualified, so the copy carries
    /// them as it carries the task's own edges — remapped to the counterpart a member of the
    /// copied set lands as, and withheld from, and recorded on the private side of, a write
    /// anywhere not declared private.
    ///
    /// Asked only of the private sources the task's home names a member project in: a task
    /// filed under no such home holds no edge the private side could record, so a copy of one
    /// asks no source it did not before. The home's member list comes from the read that
    /// classified it — a store with a source declared private has its boundary active — and
    /// is read here only when that read was not made.
    ///
    /// # Errors
    ///
    /// The home's source failing its read or holding a member list it cannot read, and
    /// whatever [`Engine::recorded_prerequisites_in`] refuses.
    async fn recorded_for_copy(
        &self,
        id: &GlobalId,
        task: &Task,
        home_members: &mut HashMap<String, Vec<GlobalId>>,
    ) -> Result<Vec<DependencyEdge>, EngineError> {
        let recording = self.recording_sources(&id.source);
        let Some(project) = task.project.as_ref().filter(|_| !recording.is_empty()) else {
            return Ok(Vec::new());
        };
        let home = GlobalId::new(id.source.clone(), project.clone());
        if let std::collections::hash_map::Entry::Vacant(unread) =
            home_members.entry(home.to_string())
        {
            let source = self.readable(&id.source)?;
            let held = source
                .source()
                .get_project(project)
                .await
                .map_err(|error| refused(source, error))?;
            unread.insert(match held {
                Some(held) => members_of(&home, &held.metadata)
                    .map_err(|message| refused(source, SourceError::Malformed { message }))?,
                None => Vec::new(),
            });
        }
        let mut sources: Vec<SourceName> = Vec::new();
        for member in &home_members[&home.to_string()] {
            if recording.contains(&member.source) && !sources.contains(&member.source) {
                sources.push(member.source.clone());
            }
        }
        if sources.is_empty() {
            return Ok(Vec::new());
        }
        Ok(self
            .recorded_prerequisites_in(id, &sources)
            .await?
            .into_iter()
            .map(|prerequisite| DependencyEdge {
                from: DependencyEndpoint::from_native(id.native.clone(), ItemKind::Task),
                to: DependencyEndpoint::new(prerequisite.to_string(), ItemKind::Task)
                    .expect("a qualified id is a valid endpoint"),
                kind: DependencyKind::Blocks,
            })
            .collect())
    }

    /// Decide where one read item is going, once it is known which source it lands in.
    async fn aim(
        &self,
        request: &CopyRequest,
        read: Read,
        placement: Placement,
        classification: Classification,
        aimed: Option<(Target, Option<Prior>)>,
        home_members: &mut HashMap<String, Vec<GlobalId>>,
    ) -> Result<Planned, EngineError> {
        let Read {
            source: id,
            item,
            comments,
        } = read;
        let withheld = self.withholds_provenance(&id.source, &placement.destination);
        let lineage = (!withheld)
            .then(|| lineage_of(&item))
            .flatten()
            .filter(|root| {
                root.as_str() != Some(id.to_string().as_str())
                    && root
                        .as_str()
                        .and_then(|root| root.parse::<GlobalId>().ok())
                        .is_none_or(|root| {
                            !self.withholds_provenance(&root.source, &placement.destination)
                        })
            });
        let destination = self.writable(&placement.destination)?;
        if item.level() == Level::Document {
            documentary(destination)?;
        }
        // Before the destination is read, and so before it is written: a destination that
        // holds no priority has nowhere to put one, and every item of a copy is planned before
        // any of them lands. A task carrying `none` passes and writes exactly as it always did.
        if let Item::Task(task) = &item {
            holds_priority(destination, &id.to_string(), task.priority)?;
        }
        let routed = self.routes.routes(&request.destination);
        // A task or a document whose counterpart already sits somewhere a route could have
        // sent it, other than where it routes now, was moved by a repository changed after
        // the copy. That is a person's decision rather than a silent move, so it is refused
        // before anything is written. A project's home is exempt: it stays where it landed.
        if routed && item.level() != Level::Project {
            self.misrouted(request, &id, &item, &placement.destination)
                .await?;
        }
        let source = self.readable(&id.source)?;
        // Read, and refused, before the destination is read and before anything is written:
        // an asset its source does not hold, and a destination that stores none.
        let assets = carried_assets(source, &id, &item).await?;
        if let Some(first) = assets.first() {
            assets::stores(destination, &id.to_string(), &first.name)?;
        }
        // `read_for_copy` reads every page of a task's comments whenever the copy carries them,
        // so a carried task without them is a read that went wrong, never a task with none.
        let carried = match (request.comments && item.level() == Level::Task, comments) {
            (false, _) => None,
            (true, Some(comments)) => Some(comments),
            (true, None) => {
                return Err(refused(
                    source,
                    SourceError::Malformed {
                        message: format!("the read of task {id} carried no comments"),
                    },
                ));
            }
        };
        let mut edges = forward_edges(source, &id.native, item.level()).await?;
        if let Item::Task(task) = &item {
            edges.extend(self.recorded_for_copy(&id, task, home_members).await?);
        }
        // A task copied by query was already aimed from its listing, when the copy asked
        // whether it could skip it.
        let (mut target, mut held) = match aimed {
            Some(aimed) => aimed,
            None => {
                self.target(destination, request, &id, &item, withheld, None)
                    .await?
            }
        };
        // A project the destination already holds a member of the same plan for is that
        // member, never a second project beside it.
        if matches!(target, Target::Create)
            && item.level() == Level::Project
            && !request.create
            && let Some((member, home, prior)) = self
                .plan_project_in(destination, &id, &item, request.comments)
                .await?
        {
            target = Target::Update {
                id: member,
                found: Found::Member { home },
            };
            held = prior;
        }
        // A create the destination's own policy on where it creates refuses is refused here,
        // before anything of the copy is written and before any intent to create is recorded —
        // a dry run's included, so it reports the refusal its copy would meet. An update is
        // never refused by it.
        if matches!(target, Target::Create)
            && let Some(reason) = destination
                .source()
                .creation_refusal(&WriteTarget::New {
                    repositories: repositories_of(&item),
                    project: None,
                })
                .await
                .map_err(|error| refused(destination, error))?
        {
            return Err(EngineError::CreationOwner {
                item: id.clone(),
                destination: destination.name().clone(),
                reason,
            });
        }
        Ok(Planned {
            status_policy: request.status_policy,
            source: id,
            source_status: item.status().cloned(),
            item,
            edges,
            target,
            held,
            to: placement.destination.clone(),
            placed: routed.then_some(placement),
            assets,
            classification,
            withheld,
            lineage,
            carried,
            comment_plan: None,
            skipped: false,
            status_condition: None,
            priority_condition: None,
            assignee_condition: None,
            assignees: None,
            intent: None,
            relist: false,
            links: None,
            pending: Vec::new(),
            comment_links: Vec::new(),
            comment_sections: sections::SectionCounts::default(),
            rewrite: Rewrite::default(),
        })
    }

    /// Whether an item of `source` landing in `to` leaves its provenance behind: provenance
    /// naming a private source stays out of anywhere not declared private.
    fn withholds_provenance(&self, source: &SourceName, to: &SourceName) -> bool {
        self.declared(source) == crate::config::SourceVisibility::Private
            && self.declared(to) != crate::config::SourceVisibility::Private
    }

    /// The project `destination` already holds of the plan the project `id` is the source of,
    /// with the plan's counterpart home elsewhere and what the destination holds there: the
    /// member that home names in `destination`, or else the project there naming that home at
    /// `onetaskgraph.member_of`.
    ///
    /// The counterpart home is a project `id`'s own `onetaskgraph.copies` links to in another
    /// source, which names `id` back as its origin — or names none, where that copy withheld it
    /// (see [`Engine::linked_project`]) — and is no member itself. `None` when there is no
    /// such home, or `destination` holds neither.
    async fn plan_project_in(
        &self,
        destination: &ResolvedSource,
        id: &GlobalId,
        item: &Item,
        comments: bool,
    ) -> Result<Option<(NativeId, GlobalId, Option<Prior>)>, EngineError> {
        let links = well_formed(described(item).1.get(MetadataKey::COPIES_KEY));
        for linked in links
            .values()
            .filter_map(|linked| linked.as_str()?.parse::<GlobalId>().ok())
        {
            if &linked.source == destination.name() {
                continue;
            }
            let Ok(at) = self.readable(&linked.source) else {
                continue;
            };
            if !at.source().capabilities().projects.is_native() {
                continue;
            }
            let Some(home) = at
                .source()
                .get_project(&linked.native)
                .await
                .map_err(|error| refused(at, error))?
            else {
                continue;
            };
            let names_back = match origin_of(&home.metadata) {
                Some(origin) => &origin == id,
                None => {
                    self.declared(&id.source) == crate::config::SourceVisibility::Private
                        && self.declared(&linked.source) != crate::config::SourceVisibility::Private
                }
            };
            if !names_back || home.metadata.contains_key(MetadataKey::MEMBER_OF_KEY) {
                continue;
            }
            let named = Home::of(linked.clone(), &home)?
                .members
                .into_iter()
                .find(|member| &member.source == destination.name());
            let held = match named {
                Some(member) => destination
                    .source()
                    .get_project(&member.native)
                    .await
                    .map_err(|error| refused(destination, error))?
                    .map(|_| member.native),
                None => None,
            };
            let found = match held {
                Some(member) => Some(member),
                None => {
                    self.scan(
                        destination,
                        Level::Project,
                        &Wanted::Metadata(
                            MetadataKey::MEMBER_OF_KEY.to_owned(),
                            Value::String(linked.to_string()),
                        ),
                    )
                    .await?
                }
            };
            if let Some(member) = found {
                let prior = self
                    .prior_reading(destination, Level::Project, &member, comments)
                    .await?;
                return Ok(Some((member, linked, prior)));
            }
        }
        Ok(None)
    }

    /// Refuse an item whose existing counterpart, by its link or its origin, sits in a source
    /// its copy could reach other than `to`, the one it routes to now.
    async fn misrouted(
        &self,
        request: &CopyRequest,
        id: &GlobalId,
        item: &Item,
        to: &SourceName,
    ) -> Result<(), EngineError> {
        let metadata = described(item).1;
        for elsewhere in self.routes.reachable(&request.destination) {
            if &elsewhere == to {
                continue;
            }
            let there = self.writable(&elsewhere)?;
            let linked = match link_of(metadata, &elsewhere) {
                Some(link) => self
                    .prior(there, item.level(), &link.native)
                    .await?
                    .filter(|held| origin_of(described(&held.item).1).as_ref() == Some(id))
                    .map(|_| link),
                None => None,
            };
            let originated = match origin_of(metadata).filter(|origin| origin.source == elsewhere) {
                Some(origin) => self
                    .prior(there, item.level(), &origin.native)
                    .await?
                    .map(|_| origin),
                None => None,
            };
            if let Some(counterpart) = linked.or(originated) {
                return Err(EngineError::Misrouted {
                    item: id.clone(),
                    counterpart,
                    route: to.clone(),
                });
            }
        }
        Ok(())
    }

    /// Where a document lands: with its project's home, wherever that is; by its own
    /// repositories when its project has none yet, or it is in no project.
    async fn document_placement(
        &self,
        request: &CopyRequest,
        id: &GlobalId,
        document: &Document,
        classification: Classification,
        running: &mut Running,
    ) -> Result<Placement, EngineError> {
        if self.routes.routes(&request.destination)
            && let Some(project) = &document.project
        {
            let qualified = GlobalId::new(id.source.clone(), project.clone());
            if let Some(home) = self.find_home(request, &qualified, running).await? {
                return Ok(self.placed_at(request, &home.source));
            }
        }
        Ok(self.routes.place_classified(
            &request.destination,
            classification,
            &document.repositories,
        ))
    }

    /// What the report says placed an item in `at`: the first route entry sending there, or
    /// none for the copy's own destination.
    fn placed_at(&self, request: &CopyRequest, at: &SourceName) -> Placement {
        Placement {
            destination: at.clone(),
            route: self
                .routes
                .of(&request.destination)
                .iter()
                .position(|route| route.to() == at)
                .map(|index| u32::try_from(index).unwrap_or(u32::MAX)),
        }
    }

    /// Which destination item this one corresponds to, by the link the item records, the
    /// two origin rules and the caller's escape, and what the destination holds there.
    ///
    /// Given `carriers`, the origin query is read to its last page and every item it finds
    /// carrying this one's origin is put there in listing order — the counterpart still the
    /// first, as a copy picks it. That is what `task locate` reports; a copy passes `None`
    /// and stops at the first.
    async fn target(
        &self,
        destination: &ResolvedSource,
        request: &CopyRequest,
        id: &GlobalId,
        item: &Item,
        withheld: bool,
        carriers: Option<&mut Vec<NativeId>>,
    ) -> Result<(Target, Option<Prior>), EngineError> {
        let (title, metadata) = described(item);
        // An intent an earlier attempt recorded is resolved before every other rule, and before
        // `--create` is believed: the item its create made, when the destination holds it,
        // is this item's counterpart rather than a reason to create a second.
        if let Some(intent) = intent::held(metadata, destination.name()).map_err(|problem| {
            EngineError::SourceRefused {
                name: id.source.to_string(),
                error: SourceError::Malformed {
                    message: format!(
                        "{id}: {problem}; next: remove that entry once no copy of {id} is \
                             running"
                    ),
                },
            }
        })? && let Some(native) = self
            .resolve_intent(
                item.level(),
                destination,
                id,
                &intent,
                repositories_of(item),
                if request.dry_run {
                    intent::Resolving::Reads
                } else if request.no_link {
                    intent::Resolving::Waits
                } else {
                    intent::Resolving::WaitsLeased
                },
            )
            .await?
            && let Some(held) = self
                .prior_reading(destination, item.level(), &native, request.comments)
                .await?
        {
            return Ok((
                Target::Update {
                    id: native,
                    found: Found::Intent,
                },
                Some(held),
            ));
        }
        // A caller asserting there is nothing to find is believed, and nothing is looked for —
        // unless the item itself says otherwise, by naming a counterpart at the destination.
        if request.create {
            let carrier = link_of(metadata, destination.name()).or_else(|| {
                origin_of(metadata).filter(|origin| &origin.source == destination.name())
            });
            if let Some(carrier) = carrier {
                return Err(EngineError::CreateCarried {
                    item: id.clone(),
                    carrier,
                });
            }
            return Ok((Target::Create, None));
        }
        if let Some(target) = &request.target_task {
            if &target.source != destination.name() {
                return Err(EngineError::TargetTaskSource {
                    target: target.clone(),
                    destination: destination.name().clone(),
                });
            }
            let held = self
                .prior_reading(destination, item.level(), &target.native, request.comments)
                .await?
                .ok_or_else(|| EngineError::TargetTaskMissing {
                    target: target.clone(),
                })?;
            if origin_of(described(&held.item).1).as_ref() != Some(id) {
                return Err(EngineError::TargetTaskOrigin {
                    target: target.clone(),
                    item: id.clone(),
                    held: Some(Box::new(item_result(&held.item))),
                });
            }
            return Ok((
                Target::Update {
                    id: target.native.clone(),
                    found: Found::TargetTask,
                },
                Some(held),
            ));
        }
        // The link first, and it is believed only when the item it names still names this
        // one back: a person who re-pointed that item's origin has said it is not this
        // one's counterpart any more, so the link is ignored and rewritten to whatever the
        // rules below find.
        let comments = request.comments;
        if let Some(link) = link_of(metadata, destination.name()) {
            match self
                .prior_reading(destination, item.level(), &link.native, comments)
                .await?
            {
                // A copy whose provenance was withheld left no origin there to name this
                // item back, so a link it recorded on its own private source is believed
                // while the item it names holds none.
                Some(held)
                    if origin_of(described(&held.item).1).as_ref() == Some(id)
                        || (withheld && origin_of(described(&held.item).1).is_none()) =>
                {
                    return Ok((
                        Target::Update {
                            id: link.native,
                            found: Found::Link,
                        },
                        Some(held),
                    ));
                }
                Some(_) => {}
                None if request.recreate => {}
                None => {
                    return Err(EngineError::StaleLink {
                        item: id.to_string(),
                        link: link.to_string(),
                    });
                }
            }
        }
        if let Some(origin) = origin_of(metadata)
            && &origin.source == destination.name()
        {
            // The read that says the origin still names something is the read of what it
            // holds, so rule 1 costs one round trip rather than two.
            if let Some(held) = self
                .prior_reading(destination, item.level(), &origin.native, comments)
                .await?
            {
                return Ok((
                    Target::Update {
                        id: origin.native,
                        found: Found::Origin,
                    },
                    Some(held),
                ));
            }
            if !request.recreate {
                return Err(EngineError::StaleOrigin {
                    item: id.to_string(),
                    origin: origin.to_string(),
                });
            }
        }
        let wanted = Wanted::Origin(id.to_string());
        let scanned = match carriers {
            Some(carriers) => {
                *carriers = self
                    .walk(destination, item.level(), &wanted, Walk::Every)
                    .await?;
                carriers.first().cloned()
            }
            None => self.scan(destination, item.level(), &wanted).await?,
        };
        // A copy into a destination its policy rewrites wrote the origin as that policy passed
        // it: an id carrying a replaced term is found by its replaced spelling, asked of the
        // policy only once the id as it stands has found nothing.
        let scanned = match scanned {
            Some(found) => Some(found),
            None => match self.origin_as_written(destination, id).await {
                Some(replaced) => {
                    self.scan(destination, item.level(), &Wanted::Origin(replaced))
                        .await?
                }
                None => None,
            },
        };
        if let Some(found) = scanned {
            let held = self
                .prior_reading(destination, item.level(), &found, comments)
                .await?;
            return Ok((
                Target::Update {
                    id: found,
                    found: Found::Scan,
                },
                held,
            ));
        }
        let wanted = match &request.match_by {
            Some(MatchBy::Title) => Some(Wanted::Title(title.to_owned())),
            Some(MatchBy::Metadata(key)) => metadata
                .get(key)
                .map(|value| Wanted::Metadata(key.clone(), value.clone())),
            None => None,
        };
        if let Some(wanted) = wanted
            && let Some(found) = self.scan(destination, item.level(), &wanted).await?
        {
            let held = self
                .prior_reading(destination, item.level(), &found, comments)
                .await?;
            return Ok((
                Target::Update {
                    id: found,
                    found: Found::Match,
                },
                held,
            ));
        }
        Ok((Target::Create, None))
    }

    /// Walk the destination one page at a time, looking for `wanted`.
    ///
    /// One page is held at a time and nothing is written down, which is the same bound
    /// every other compensation in this engine works under.
    async fn scan(
        &self,
        destination: &ResolvedSource,
        kind: Level,
        wanted: &Wanted,
    ) -> Result<Option<NativeId>, EngineError> {
        Ok(self
            .walk(destination, kind, wanted, Walk::First)
            .await?
            .into_iter()
            .next())
    }

    /// Walk the destination one page at a time for the items `wanted` finds, in listing order,
    /// as far as `walk` says: to the first, or to the last page for every one.
    async fn walk(
        &self,
        destination: &ResolvedSource,
        kind: Level,
        wanted: &Wanted,
        walk: Walk,
    ) -> Result<Vec<NativeId>, EngineError> {
        let mut found = Vec::new();
        let mut cursor: Option<Cursor> = None;
        let tasks = wanted.task_query(&destination.source().capabilities());
        loop {
            let asked = cursor.clone();
            let request = request_for(destination, cursor);
            let next = match kind {
                Level::Task => {
                    let page = destination
                        .source()
                        .query_tasks(&tasks, &request)
                        .await
                        .map_err(|error| refused(destination, error))?;
                    fits(page.items.len(), request.limit)
                        .map_err(|error| refused(destination, error))?;
                    for task in &page.items {
                        if wanted.found(&task.title, &task.metadata) {
                            found.push(task.id.clone());
                            if walk == Walk::First {
                                return Ok(found);
                            }
                        }
                    }
                    page.next
                }
                // An origin is asked for as itself, so a source that can find the items one
                // origin names answers without listing everything it holds.
                Level::Project => {
                    let page = match wanted {
                        Wanted::Origin(origin) => {
                            destination
                                .source()
                                .query_projects_by_origin(origin, &request)
                                .await
                        }
                        _ => {
                            destination
                                .source()
                                .query_projects(&ProjectQuery::default(), &request)
                                .await
                        }
                    }
                    .map_err(|error| refused(destination, error))?;
                    fits(page.items.len(), request.limit)
                        .map_err(|error| refused(destination, error))?;
                    for project in &page.items {
                        if wanted.found(&project.title, &project.metadata) {
                            found.push(project.id.clone());
                            if walk == Walk::First {
                                return Ok(found);
                            }
                        }
                    }
                    page.next
                }
                Level::Document => {
                    let page = match wanted {
                        Wanted::Origin(origin) => {
                            destination
                                .source()
                                .query_documents_by_origin(origin, &request)
                                .await
                        }
                        _ => {
                            destination
                                .source()
                                .query_documents(&DocumentQuery::default(), &request)
                                .await
                        }
                    }
                    .map_err(|error| refused(destination, error))?;
                    fits(page.items.len(), request.limit)
                        .map_err(|error| refused(destination, error))?;
                    for document in &page.items {
                        if wanted.found(&document.title, &document.metadata) {
                            found.push(document.id.clone());
                            if walk == Walk::First {
                                return Ok(found);
                            }
                        }
                    }
                    page.next
                }
            };
            unrepeated(
                next.as_ref(),
                asked.as_ref(),
                "the destination was being scanned for the item to update",
            )
            .map_err(|error| refused(destination, error))?;
            match next {
                Some(next) => cursor = Some(next),
                None => return Ok(found),
            }
        }
    }

    /// Whether `held`, the counterpart of `item` at `destination`, is a task an earlier copy
    /// recorded that lacks the mark a copy now leaves on one whose every edge — `edges`, as this
    /// copy writes them — is on its own record.
    fn unmarked(
        &self,
        destination: &ResolvedSource,
        item: &Planned,
        held: &Prior,
        edges: &[DependencyEdge],
    ) -> bool {
        let Item::Task(held) = &held.item else {
            return false;
        };
        self.declared(destination.name()) != crate::config::SourceVisibility::Private
            && edges.len() == item.edges.len()
            && held
                .metadata
                .contains_key(MetadataKey::COPIED_DESTINATION_VERSION_KEY)
            && !held.metadata.contains_key(MetadataKey::EDGES_ON_RECORD_KEY)
    }

    /// Write one planned item, or say what a dry run would have done.
    ///
    /// Answers with what the destination held there beforehand as well, which is what
    /// makes an item written twice restorable to what it was rather than to what this
    /// copy's first pass left.
    // llmlint: ignore[suppressions_justified] `created` is the one argument this change adds: the id a task created together with others was created under, which only `copy_items` holds and which decides how this one write records itself. The rest are what `land` always took, each a distinct decision of the copy; grouping them would only move the list to a constructor, as at `write`.
    #[allow(clippy::too_many_arguments)]
    async fn land(
        &self,
        destination: &ResolvedSource,
        request: &CopyRequest,
        item: &Planned,
        project: Option<NativeId>,
        pointing: Pointing<'_>,
        created: Option<NativeId>,
        journal: &mut Journal,
    ) -> Result<(CopyOutcome, Option<Prior>), EngineError> {
        let Pointing { edges, delivers } = pointing;
        let (target, found) = match &item.target {
            Target::Update { id, found } => (Some(id.clone()), Some(found.clone())),
            Target::Create => (None, None),
        };
        // Every link is settled once the whole copy has landed, by `Engine::link`, so none
        // is known here.
        let comments = if item.skipped {
            item.carried.is_some().then(|| {
                skipped_counts(
                    item.held.as_ref(),
                    &item.source,
                    origin_of(described(&item.item).1).as_ref(),
                )
            })
        } else {
            item.comment_plan.as_ref().map(|plan| plan.counts)
        };
        let reached = |destination: Option<GlobalId>, changed: bool| match (found, destination) {
            (Some(via), Some(destination)) if changed => CopyAction::Updated {
                destination,
                via: via.via(),
                link: None,
                comments,
            },
            (Some(via), Some(destination)) => CopyAction::Unchanged {
                destination,
                via: via.via(),
                link: None,
                skipped: item.skipped,
                comments,
            },
            (_, destination) => CopyAction::Created {
                destination,
                via: NoCounterpart::Created,
                link: None,
                comments,
            },
        };
        let outcome = |action: CopyAction| CopyOutcome {
            body_size: None,
            sections: SectionLinks::default(),
            source: item.source.clone(),
            action,
            placed: item.placed.clone(),
            expected: ExpectedWrite::default(),
            status_condition: item.status_condition,
            priority_condition: item.priority_condition,
            assignee_condition: item.assignee_condition.clone(),
            held: None,
        };
        // The one read of the destination item, made where its target was found, used to
        // decide whether the write changes anything and — if the copy cannot finish — to
        // put that item back.
        let prior = item.held.clone();
        let qualified = |native: NativeId| GlobalId::new(destination.name().clone(), native);
        // What an item the copy found already reading as it does holds: what the read before
        // the copy held, since nothing was written to it.
        let as_held = |outcome: CopyOutcome| match &prior {
            Some(held) if !request.dry_run => with_result(outcome, item_result(&held.item)),
            _ => outcome,
        };
        if item.skipped
            && let Some(id) = &target
        {
            return Ok((
                as_held(outcome(reached(Some(qualified(id.clone())), false))),
                prior,
            ));
        }
        let edges = resolved(edges);
        let comments_move = item
            .comment_plan
            .as_ref()
            .is_some_and(|plan| !plan.steps.is_empty());
        let covers = self.version_covers_a_copy(item);
        let changed = comments_move
            || target.as_ref().is_none_or(|id| {
                changes(
                    prior.as_ref(),
                    item,
                    id,
                    &project,
                    &edges,
                    delivers,
                    destination,
                    covers,
                )
            });
        // An item that already reads as the source does may still hold a stale record of the
        // copy that left it so, which this copy brings up to date without calling it changed.
        let rerecord = !changed
            && match (&target, prior.as_ref()) {
                (Some(id), Some(held)) => {
                    stale_record(
                        item,
                        held,
                        id,
                        &project,
                        &edges,
                        delivers,
                        destination,
                        covers,
                    ) || self.unmarked(destination, item, held, &edges)
                }
                _ => false,
            };
        if let Some(id) = &target
            && !changed
            && !rerecord
        {
            return Ok((
                as_held(outcome(reached(Some(qualified(id.clone())), false))),
                prior,
            ));
        }
        if request.dry_run {
            // Null only for a create here: nothing was created, so there is no id to report.
            return Ok((outcome(reached(target.map(qualified), changed)), prior));
        }
        // A task created together with others is written once more, as the item its create
        // made, now that it can name its edges to them: it was journalled when it was created.
        let stamping = match created {
            Some(_) if !destination.source().records_created_together() => Stamping::Never,
            _ => Stamping::of(request, rerecord),
        };
        // What the item holds once written, as the copy sent it and the destination reads a
        // status back: refined from the destination's own answers once the whole copy is done.
        let mut sent = item_result(&as_written(
            destination,
            item,
            outgoing(
                item,
                target.clone().unwrap_or_else(|| item.item.id().clone()),
                project.clone(),
                &recorded(item, prior.as_ref()),
                delivers,
                prior.as_ref(),
            ),
        ));
        // The token `outgoing` carries is the source's; the destination's is not known yet.
        sent.version = None;
        let written = self
            .write(
                destination,
                item,
                created.or(target.clone()),
                project,
                &edges,
                delivers,
                prior.clone(),
                stamping,
                journal,
            )
            .await?;
        let written = qualified(written);
        let expected = self.take_expected(&written).unwrap_or_default();
        // A create whose source's intent another attempt could share, or that an earlier
        // attempt's request may yet land beside, is listed again by its key: one more item
        // carrying it is a duplicate, reported and never linked.
        if target.is_none() {
            intent::pause_at("after-create");
        }
        if target.is_none()
            && item.relist
            && let Some(intent) = &item.intent
        {
            let listed = self
                .relisted(
                    item.item.level(),
                    destination,
                    intent,
                    repositories_of(&item.item),
                )
                .await?;
            if listed.len() > 1 {
                let error = EngineError::CreateDuplicated {
                    item: item.source.clone(),
                    destination: destination.name().clone(),
                    found: listed
                        .iter()
                        .map(|native| qualified(native.clone()))
                        .collect(),
                };
                return Ok((query::failed(item.source.clone(), &error), prior));
            }
        }
        if let Some(plan) = &item.comment_plan {
            journal
                .comment_sections
                .entry(item.source.clone())
                .or_default()
                .add(item.comment_sections);
            let (plan, pending, finished_counts) =
                self.finished_comments(destination, item, &written.native, plan, journal);
            Self::restamp_comments(
                journal,
                destination.name(),
                &written.native,
                plan.after.clone(),
            );
            journal
                .comment_sections
                .entry(item.source.clone())
                .or_default()
                .add(finished_counts);
            let landed = self
                .write_comments(destination, &item.source, &written.native, &plan, journal)
                .await?;
            // A comment whose link names a record not landed yet is finished once every write
            // of the copy has: see `Engine::finish_sections`.
            for (origin, links, still) in pending {
                let written_copy = plan.steps.iter().find_map(|step| match step {
                    comments::Step::Add(copy) | comments::Step::Edit { copy, .. }
                        if comments::key(&copy.origin) == origin =>
                    {
                        Some(copy.clone())
                    }
                    _ => None,
                });
                if let (Some((_, comment)), Some(copy)) = (
                    landed.iter().find(|(held, _)| *held == origin),
                    written_copy,
                ) {
                    journal.comments_pending.push(CommentPending {
                        at: destination.name().clone(),
                        source: item.source.clone(),
                        task: written.native.clone(),
                        comment: comment.clone(),
                        copy,
                        links,
                        pending: still,
                    });
                }
            }
        }
        let mut landed = outcome(reached(Some(written), changed));
        landed.expected = expected;
        Ok((with_result(landed, sent), prior))
    }

    /// `plan` with each comment it writes whose links were not addressable when it was planned
    /// rendered again, now that its task `task` has landed, and each whose links still are not,
    /// by its origin id, with its links and what is still pending.
    fn finished_comments(
        &self,
        destination: &ResolvedSource,
        item: &Planned,
        task: &NativeId,
        plan: &comments::CommentPlan,
        journal: &Journal,
    ) -> (
        comments::CommentPlan,
        Vec<CommentLinks>,
        sections::SectionCounts,
    ) {
        let mut plan = plan.clone();
        let mut pending_after = Vec::new();
        let mut finished_counts = sections::SectionCounts::default();
        if item.comment_links.is_empty() {
            return (plan, pending_after, finished_counts);
        }
        let knows = Writing {
            destination,
            landed: &journal.landed,
            own: located(
                destination
                    .source()
                    .written_location(MetadataRecord::Task, task)
                    .as_ref(),
            )
            .map(|location| (task.clone(), location)),
        };
        let mut reworded = Vec::new();
        for step in &mut plan.steps {
            let (comments::Step::Add(copy) | comments::Step::Edit { copy, .. }) = step else {
                continue;
            };
            let Some((_, links, pending)) = item
                .comment_links
                .iter()
                .find(|(origin, _, _)| *origin == comments::key(&copy.origin))
            else {
                continue;
            };
            let (finished, still) = links.finished(
                copy.body.as_str(),
                pending,
                &knows,
                sections::Pass::Provisional,
            );
            if let Ok(body) = onetaskgraph_plugin_api::CommentBody::new(finished) {
                copy.body = body;
            }
            reworded.push((copy.origin.clone(), copy.body.as_str().to_owned()));
            if still.is_empty() {
                finished_counts.add(links.render(&knows, sections::Pass::Final).sections);
            } else {
                pending_after.push((comments::key(&copy.origin), links.clone(), still));
            }
        }
        for (origin, body) in &reworded {
            comments::reword(&mut plan.after, origin, body);
        }
        (plan, pending_after, finished_counts)
    }

    /// The comments the record of `item`'s write as `task` digests: as the copy leaves them,
    /// each carried one with its section links finished as far as they can be before the task
    /// has landed. `None` for a copy that carries none.
    fn digested_comments(
        &self,
        destination: &ResolvedSource,
        item: &Planned,
        task: &NativeId,
        journal: &Journal,
    ) -> Option<Vec<Value>> {
        match &item.comment_plan {
            Some(plan) if item.carried.is_some() => Some(
                self.finished_comments(destination, item, task, plan, journal)
                    .0
                    .after,
            ),
            _ => record_comments(item),
        }
    }

    /// Bring the record of the task `id` of `at` a `--comments` write stamped into line with
    /// `comments`, the comments as its carried ones were finished after that write: in the
    /// write the item is already owed when it is owed one, and otherwise in one more, counted
    /// as a write that finished its section links.
    fn restamp_comments(
        journal: &mut Journal,
        at: &SourceName,
        id: &NativeId,
        comments: Vec<Value>,
    ) {
        let Some(stamped) = journal.stamped.get_mut(&(at.clone(), id.clone())) else {
            return;
        };
        if stamped.comments == comments {
            return;
        }
        stamped.comments.clone_from(&comments);
        if let Some(held) = journal
            .sections_pending
            .iter_mut()
            .find(|held| &held.at == at && &held.id == id)
        {
            held.comments = Some(comments);
            return;
        }
        if let Some(follow) = journal
            .follow_ups
            .iter_mut()
            .find(|follow| &follow.at == at && &follow.id == id)
        {
            follow.record.get_or_insert_with(|| stamped.record.clone());
            follow.comments = Some(comments);
            return;
        }
        let source = stamped.source.clone();
        let follow = FollowUp {
            at: at.clone(),
            id: id.clone(),
            source: source.clone(),
            item: stamped.item.clone(),
            edges: stamped.edges.clone(),
            record: Some(stamped.record.clone()),
            comments: Some(comments),
            stored: content_of(&stamped.item).unwrap_or_default().to_owned(),
            answer: FollowUpAnswer::Exact,
        };
        journal.follow_ups.push(follow);
        journal
            .sections
            .entry(source)
            .or_default()
            .section_link_updates += 1;
    }

    /// Make one destination task's comments what `plan` decided, recording how to take each
    /// write back — renewing the lease on `source`'s intent before each, while this copy owns
    /// its create. Answers each comment it added or edited, by the origin its copy carries,
    /// with its id at the destination.
    async fn write_comments(
        &self,
        destination: &ResolvedSource,
        source: &GlobalId,
        task: &NativeId,
        plan: &comments::CommentPlan,
        journal: &mut Journal,
    ) -> Result<Vec<(comments::OriginKey, NativeId)>, EngineError> {
        let mut landed = Vec::new();
        let at = destination.name().clone();
        let gone = || EngineError::NoSuchTask {
            id: GlobalId::new(at.clone(), task.clone()).to_string(),
        };
        for (index, step) in plan.steps.iter().enumerate() {
            intent::pause_at(&format!("before-comment-{index}"));
            self.renew_owned(source, destination.name(), journal)
                .await?;
            match step {
                comments::Step::Delete(before) => {
                    let removed = destination
                        .source()
                        .delete_comment(task, &before.id)
                        .await
                        .map_err(|error| refused(destination, error))?;
                    if removed.is_some()
                        && let (Some(origin), Ok(body)) = (
                            before.origin.clone(),
                            onetaskgraph_plugin_api::CommentBody::new(before.body.clone()),
                        )
                    {
                        journal.comments.push(CommentUndo {
                            at: at.clone(),
                            task: task.clone(),
                            undo: CommentUndoing::Deleted(CommentCopy { body, origin }),
                        });
                    }
                }
                comments::Step::Edit { id, copy, before } => {
                    let edited = destination
                        .source()
                        .write_comment_copy(task, Some(id), copy)
                        .await
                        .map_err(|error| refused(destination, error))?;
                    match edited {
                        Some(_) => {
                            landed.push((comments::key(&copy.origin), id.clone()));
                            if let (Some(origin), Ok(body)) = (
                                before.origin.clone(),
                                onetaskgraph_plugin_api::CommentBody::new(before.body.clone()),
                            ) {
                                journal.comments.push(CommentUndo {
                                    at: at.clone(),
                                    task: task.clone(),
                                    undo: CommentUndoing::Edited {
                                        id: id.clone(),
                                        before: CommentCopy { body, origin },
                                    },
                                });
                            }
                        }
                        // Removed since it was read: the copy is written afresh.
                        None => {
                            let added = destination
                                .source()
                                .write_comment_copy(task, None, copy)
                                .await
                                .map_err(|error| refused(destination, error))?
                                .ok_or_else(gone)?;
                            landed.push((comments::key(&copy.origin), added.id.clone()));
                            journal.comments.push(CommentUndo {
                                at: at.clone(),
                                task: task.clone(),
                                undo: CommentUndoing::Added(added.id),
                            });
                        }
                    }
                }
                comments::Step::Add(copy) => {
                    let added = destination
                        .source()
                        .write_comment_copy(task, None, copy)
                        .await
                        .map_err(|error| refused(destination, error))?
                        .ok_or_else(gone)?;
                    landed.push((comments::key(&copy.origin), added.id.clone()));
                    journal.comments.push(CommentUndo {
                        at: at.clone(),
                        task: task.clone(),
                        undo: CommentUndoing::Added(added.id),
                    });
                }
            }
        }
        Ok(landed)
    }

    /// Which destination project this item is filed under, when it is filed at all.
    ///
    /// A task copied as part of a project copy is filed under that project's counterpart,
    /// which the copy has just established. A task copied on its own has to find it.
    async fn filed(
        &self,
        request: &CopyRequest,
        item: &Planned,
        project: Option<NativeId>,
        running: &mut Running,
        journal: &mut Journal,
    ) -> Result<Option<NativeId>, EngineError> {
        match (&item.item, project) {
            (Item::Task(task), None) => {
                self.counterpart(request, item, task.project.as_ref(), running, journal)
                    .await
            }
            (Item::Document(document), None) => {
                self.counterpart(request, item, document.project.as_ref(), running, journal)
                    .await
            }
            (Item::Task(_) | Item::Document(_), filed) => Ok(filed),
            (Item::Project(_), _) => Ok(None),
        }
    }

    /// The destination project this task's own project corresponds to, when there is one.
    ///
    /// A task copied on its own keeps its source's project id when the destination holds
    /// no counterpart: the field is opaque to this engine, and dropping it would lose
    /// what the source said — unless that project is private, or cannot be classified, and the
    /// destination is not declared private (see [`Engine::withholds_project`]).
    ///
    /// Looked for once per source project and landing source per command. A project this
    /// command itself copied is already known, and one an earlier task of this command was
    /// filed under was already looked for; walking the destination again for either would
    /// spend a scan per task on an answer the command holds. A project whose own link names
    /// its counterpart there is found by that link — see [`Engine::linked_project`] — and
    /// the destination is walked only when it does not.
    ///
    /// A routed item whose project's home landed in another source than it does is filed
    /// under that home's member project in its own source — see [`Engine::member_in`].
    async fn counterpart(
        &self,
        request: &CopyRequest,
        item: &Planned,
        project: Option<&NativeId>,
        running: &mut Running,
        journal: &mut Journal,
    ) -> Result<Option<NativeId>, EngineError> {
        let Some(project) = project else {
            return Ok(None);
        };
        let qualified = GlobalId::new(item.source.source.clone(), project.clone());
        let key = (qualified.to_string(), item.to.clone());
        if let Some(looked) = running.filings.get(&key) {
            return Ok(looked.clone());
        }
        if let Some(filed) = self.filed_as_linked(item, &qualified, running).await {
            running.filings.insert(key, Some(filed.clone()));
            return Ok(Some(filed));
        }
        // A document is filed under the project its own project's link names where it lands,
        // without that project being read: a link a person left stale is told by the write the
        // destination refuses — see [`Engine::stale_filing`] — and `--recreate` walks instead.
        let document = matches!(item.item, Item::Document(_));
        if document
            && !request.recreate
            && let Some(link) = self.project_link(&qualified, &item.to, running).await?
        {
            running.trusted.insert(link.clone(), qualified.clone());
            running.filings.insert(key, Some(link.native.clone()));
            return Ok(Some(link.native));
        }
        let home = self.find_home(request, &qualified, running).await?;
        if document
            && request.recreate
            && !request.dry_run
            && let Some(home) = &home
        {
            self.relink_project(&qualified, home, running, journal)
                .await?;
        }
        let filed = match home {
            Some(home) if home.source == item.to => Some(home.native),
            Some(home) => {
                self.member_in(request.dry_run, &home, &item.to, None, running, journal)
                    .await?
            }
            None if self.withholds_project(&qualified, &item.to, running).await => None,
            None => Some(project.clone()),
        };
        running.filings.insert(key, filed.clone());
        Ok(filed)
    }

    /// The source project `project`, read once per command.
    async fn source_project(
        &self,
        project: &GlobalId,
        running: &mut Running,
    ) -> Result<Option<Project>, EngineError> {
        if let Some(held) = running.source_projects.get(project) {
            return Ok(Some(held.clone()));
        }
        let source = self.readable(&project.source)?;
        if !source.source().capabilities().projects.is_native() {
            return Ok(None);
        }
        let Some(held) = source
            .source()
            .get_project(&project.native)
            .await
            .map_err(|error| refused(source, error))?
        else {
            return Ok(None);
        };
        running
            .source_projects
            .insert(project.clone(), held.clone());
        Ok(Some(held))
    }

    /// The project `project`'s own `onetaskgraph.copies` link names in `to`, when it names one.
    async fn project_link(
        &self,
        project: &GlobalId,
        to: &SourceName,
        running: &mut Running,
    ) -> Result<Option<GlobalId>, EngineError> {
        Ok(self
            .source_project(project, running)
            .await?
            .and_then(|held| link_of(&held.metadata, to)))
    }

    /// Point `project`'s own link where a `--recreate` copy of a document found its counterpart,
    /// `home`, when it names somewhere else there: the link a stale one is recovered to.
    async fn relink_project(
        &self,
        project: &GlobalId,
        home: &GlobalId,
        running: &mut Running,
        journal: &mut Journal,
    ) -> Result<(), EngineError> {
        let Some(held) = self.source_project(project, running).await? else {
            return Ok(());
        };
        if link_of(&held.metadata, &home.source).as_ref() == Some(home) {
            return Ok(());
        }
        let source = self.readable(&project.source)?;
        if !source.source().writes().is_supported() {
            return Ok(());
        }
        let before = held.metadata.get(MetadataKey::COPIES_KEY).cloned();
        let mut links = well_formed(before.as_ref());
        links.insert(home.source.to_string(), Value::String(home.to_string()));
        journal.links.push(Unlink {
            item: project.clone(),
            level: Level::Project,
            key: MetadataKey::copies(),
            before,
        });
        match set_link(
            source,
            Level::Project,
            &project.native,
            &MetadataKey::copies(),
            &Value::Object(links.clone()),
        )
        .await
        {
            Ok(true) => {
                if let Some(cached) = running.source_projects.get_mut(project) {
                    cached
                        .metadata
                        .insert(MetadataKey::COPIES_KEY.to_owned(), Value::Object(links));
                }
                Ok(())
            }
            Ok(false) | Err(SourceError::Refused { .. } | SourceError::Malformed { .. }) => {
                journal.links.pop();
                Ok(())
            }
            Err(error) => Err(refused(source, error)),
        }
    }

    /// The refusal a write of `item` owes, refused with `error` while filed under `filed`: a
    /// stale link, when `filed` is a project this copy trusted its source project's link for
    /// unread and the destination no longer holds it — which one read, made only now the write
    /// was refused, says. Every other failure is answered as it came.
    async fn stale_filing(
        &self,
        destination: &ResolvedSource,
        item: &Planned,
        filed: Option<&NativeId>,
        running: &Running,
        error: EngineError,
    ) -> EngineError {
        let Some(filed) = filed else {
            return error;
        };
        let link = GlobalId::new(item.to.clone(), filed.clone());
        let Some(project) = running.trusted.get(&link) else {
            return error;
        };
        match destination.source().get_project(filed).await {
            Ok(None) => EngineError::StaleLink {
                item: project.to_string(),
                link: link.to_string(),
            },
            _ => error,
        }
    }

    /// Whether an item copied on its own to `destination` leaves out the id of its source
    /// project `project`, which the destination holds no counterpart of: while the boundary is
    /// active and `destination` is not declared private, when that project is private — or
    /// cannot be classified at all — since a public task under a private project is public by its
    /// own record, and the id of the project it is filed under is not.
    async fn withholds_project(
        &self,
        project: &GlobalId,
        destination: &SourceName,
        running: &mut Running,
    ) -> bool {
        if !self.boundary_active()
            || self.declared(destination) == crate::config::SourceVisibility::Private
        {
            return false;
        }
        if self.declared(&project.source) == crate::config::SourceVisibility::Private {
            return true;
        }
        !matches!(
            self.source_project_class(project, &mut running.project_classes)
                .await,
            Ok(Classification::Public)
        )
    }

    /// The project a document's counterpart already sits under, when that is exactly the project
    /// its own project's `onetaskgraph.copies` link names where it lands — so it is taken as
    /// filed there without the linked project being read to confirm the link.
    ///
    /// Only ever the item the link names: a document a person moved under another project sits
    /// under that one, which is not what the link names, so the lookup below answers it and the
    /// copy puts it back. Its one blind spot is stated rather than covered: a person re-pointing
    /// that project's own origin while leaving the document under it keeps the document there
    /// until the project itself is next copied. Not for a source whose provenance is withheld,
    /// whose link a withheld origin is confirmed by differently.
    async fn filed_as_linked(
        &self,
        item: &Planned,
        project: &GlobalId,
        running: &mut Running,
    ) -> Option<NativeId> {
        if item.withheld {
            return None;
        }
        let Some(Item::Document(held)) = item.held.as_ref().map(|held| &held.item) else {
            return None;
        };
        let under = held.project.as_ref()?;
        if !running.source_projects.contains_key(project) {
            let source = self.readable(&project.source).ok()?;
            if !source.source().capabilities().projects.is_native() {
                return None;
            }
            let read = source.source().get_project(&project.native).await.ok()??;
            if read.id != project.native {
                return None;
            }
            running.source_projects.insert(project.clone(), read);
        }
        let link = link_of(&running.source_projects[project].metadata, &item.to)?;
        (&link.native == under).then_some(link.native)
    }

    /// Pin the projects the items of `planned` are filed under, so the filing lookup's read of
    /// each — [`Engine::linked_project`] — is answered from the source's cache while unchanged.
    ///
    /// Only for a source whose cache stands in front of it, only when the items name at least
    /// two of its projects this command has not already looked up, and only one page: a
    /// listing pins every project it carries, and the rest are read by id as they always were.
    /// So it never costs more than one request over reading each project, and saves one per
    /// project the page carried beyond the first. A page that fails costs only that request:
    /// every read it would have pinned is made as before, and reports its own failure.
    async fn pin_filing_projects(&self, planned: &[Planned], running: &mut Running) {
        let mut named: BTreeMap<SourceName, BTreeSet<NativeId>> = BTreeMap::new();
        for item in planned.iter().filter(|item| !item.skipped) {
            let project = match &item.item {
                Item::Task(task) => task.project.as_ref(),
                Item::Document(document) => document.project.as_ref(),
                Item::Project(_) => None,
            };
            let Some(project) = project else { continue };
            let qualified = GlobalId::new(item.source.source.clone(), project.clone());
            if running.counterparts.contains_key(&qualified.to_string())
                || running
                    .filings
                    .contains_key(&(qualified.to_string(), item.to.clone()))
            {
                continue;
            }
            named
                .entry(item.source.source.clone())
                .or_default()
                .insert(project.clone());
        }
        for (name, projects) in named {
            if projects.len() < 2 || !self.caches(&name) || running.projects_paged.contains(&name) {
                continue;
            }
            let Ok(source) = self.readable(&name) else {
                continue;
            };
            let capabilities = source.source().capabilities();
            if !capabilities.projects.is_native() {
                continue;
            }
            running.projects_paged.insert(name);
            let page = PageRequest {
                cursor: None,
                limit: capabilities.max_page_size.max(1),
            };
            let _ = source
                .source()
                .query_projects(&ProjectQuery::default(), &page)
                .await;
        }
    }

    /// The counterpart a source project has in some source this copy can reach — its home —
    /// when it has one: the one this command landed, the one its own link names, or the one a
    /// walk of each reachable source finds recording it as its origin.
    async fn find_home(
        &self,
        request: &CopyRequest,
        project: &GlobalId,
        running: &Running,
    ) -> Result<Option<GlobalId>, EngineError> {
        if let Some(landed) = running.counterparts.get(&project.to_string()) {
            return Ok(Some(landed.clone()));
        }
        if let Some(linked) = self
            .linked_project(
                request,
                project,
                running.source_projects.get(project),
                &running.asked_origins,
            )
            .await?
        {
            return Ok(Some(linked));
        }
        for reachable in self.routes.reachable(&request.destination) {
            let there = self.writable(&reachable)?;
            if let Some(found) = self
                .scan(there, Level::Project, &Wanted::Origin(project.to_string()))
                .await?
            {
                return Ok(Some(GlobalId::new(reachable, found)));
            }
        }
        Ok(None)
    }

    /// The member project `home` names in `to`, creating it when it names none there.
    ///
    /// Found through the home's `onetaskgraph.members`, read once per command — or, when the
    /// home names none `to` still holds, as the project in `to` whose `onetaskgraph.member_of`
    /// names the home, which is recorded on the home rather than created beside. A member it
    /// names that `to` no longer holds is otherwise replaced rather than refused: the member
    /// exists only to file the home's routed tasks, and nothing a person wrote lives on it. A
    /// dry run creates nothing, so a member it would create files its tasks under nothing.
    ///
    /// A member it creates is as private as its home and `classification`, the tasks it is
    /// created to file, together — or as `classification` alone where the home is in a source
    /// declared private and `to` is not. There it holds only the public work filed under it,
    /// and its home's id is withheld from it as any other provenance naming a private source
    /// is: the home's own list is what ties the two. Everywhere else it names its home at
    /// `onetaskgraph.member_of`.
    async fn member_in(
        &self,
        dry_run: bool,
        home: &GlobalId,
        to: &SourceName,
        filing: Option<&MemberFiling>,
        running: &mut Running,
        journal: &mut Journal,
    ) -> Result<Option<NativeId>, EngineError> {
        let key = home.to_string();
        if !running.homes.contains_key(&key) {
            let at = self.writable(&home.source)?;
            let Some(held) = at
                .source()
                .get_project(&home.native)
                .await
                .map_err(|error| refused(at, error))?
            else {
                return Ok(None);
            };
            running
                .homes
                .insert(key.clone(), Home::of(home.clone(), &held)?);
        }
        let named = running.homes[&key]
            .members
            .iter()
            .find(|member| &member.source == to)
            .cloned();
        let there = self.writable(to)?;
        if let Some(member) = named
            && there
                .source()
                .get_project(&member.native)
                .await
                .map_err(|error| refused(there, error))?
                .is_some()
        {
            return Ok(Some(member.native));
        }
        // A home this copy created has no member anywhere to find.
        let created = running
            .linking
            .iter()
            .any(|landed| &landed.destination == home && landed.found.is_none());
        if !created
            && let Some(found) = self
                .scan(
                    there,
                    Level::Project,
                    &Wanted::Metadata(
                        MetadataKey::MEMBER_OF_KEY.to_owned(),
                        Value::String(home.to_string()),
                    ),
                )
                .await?
        {
            let entry = running
                .homes
                .get_mut(&key)
                .expect("the home was recorded above");
            entry.members.retain(|member| &member.source != to);
            entry.members.push(GlobalId::new(to.clone(), found.clone()));
            entry.changed = true;
            return Ok(Some(found));
        }
        if dry_run {
            return Ok(None);
        }
        let held = &running.homes[&key];
        // A home in a source declared private, with a member somewhere that is not: the member
        // holds only the public work filed under it, and names nothing of where its home is.
        let apart = self.withholds(to, home, &[]);
        let mut metadata = BTreeMap::new();
        if !apart {
            metadata.insert(
                MetadataKey::MEMBER_OF_KEY.to_owned(),
                Value::String(home.to_string()),
            );
        }
        let classification = match filing {
            Some(filing) if apart => filing.classification,
            Some(filing) => filing.classification.strictest(held.classification),
            None => held.classification,
        };
        let item = match filing {
            Some(filing) => format!(
                "the member project of {home} in {to}, for {}",
                filing
                    .tasks
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            None => format!("the member project of {home}"),
        };
        let mut member = Project {
            version: None,
            id: home.native.clone(),
            title: held.title.clone(),
            content: None,
            status: held.status.clone(),
            labels: Vec::new(),
            url: None,
            location: None,
            created_at: None,
            updated_at: None,
            metadata,
            repositories: Vec::new(),
            classification,
        };
        let rewrite = self
            .admit(
                there,
                &Outbound {
                    item,
                    classification: member.classification,
                    target: WriteTarget::New {
                        repositories: &[],
                        project: None,
                    },
                    exposure: &Exposure::of_project(&member),
                },
            )
            .await?;
        rewrite.project(&mut member);
        let created = there
            .source()
            .write_project(&ItemWrite {
                target: None,
                item: member,
                depends_on: Vec::new(),
            })
            .await
            .map_err(|error| create_refused(there, Creating::Project, error))?;
        journal.record(Undo::Created {
            at: to.clone(),
            kind: Level::Project,
            id: created.clone(),
        });
        let entry = running
            .homes
            .get_mut(&key)
            .expect("the home was recorded above");
        entry.members.retain(|member| &member.source != to);
        entry
            .members
            .push(GlobalId::new(to.clone(), created.clone()));
        entry.changed = true;
        Ok(Some(created))
    }

    /// What every task of a source declared private that this copy touches is to record of the
    /// tasks depending on it from somewhere not declared private, once every destination id is
    /// known.
    ///
    /// An edge from a task this copy landed somewhere not declared private, to a task of a
    /// source declared private, is withheld from the first (see [`Engine::withholds`]); it is
    /// recorded on the second instead, under the first's destination id, by the second's own
    /// source. A dependent this copy landed whose edges no longer name it is taken off, because
    /// this copy wrote those edges; one this copy did not land is kept as it was.
    ///
    /// Only a far end that is a member of the copied set is recorded on — its counterpart, which
    /// this copy landed or resolved as already landed — because that is a task in a source this
    /// copy writes to. An id an edge names directly, of a task the copy does not carry, is
    /// withheld exactly as it always was: recording on it would write to a source the copy was
    /// never asked to write to, and may only read.
    fn recordings(&self, running: &Running) -> Recordings {
        let decided: Dependents = running
            .dependents
            .iter()
            .map(|dependent| dependent.destination.clone())
            .collect();
        let mut named: BTreeMap<String, (GlobalId, Dependents)> = BTreeMap::new();
        for dependent in &running.dependents {
            let near = &dependent.destination.source;
            let ends = mapped_edges(
                &dependent.edges,
                &dependent.origin,
                near,
                &running.resolvable,
                &running.counterparts,
            );
            for (read, end) in dependent.edges.iter().zip(ends) {
                let member = far_end(&read.to, &dependent.origin)
                    .is_some_and(|far| running.resolvable.contains(&far));
                let (true, Some(end)) = (member, end) else {
                    continue;
                };
                let Some(far) = far_end(&end.to, near) else {
                    continue;
                };
                // A far end written on the dependent itself, opaquely, is kept of nothing on the
                // private side — and the entry an earlier copy kept there is taken off below,
                // the dependent being decided.
                if end.kind != DependencyKind::Blocks
                    || end.to.kind != ItemKind::Task
                    || self.declared(&far.source) != crate::config::SourceVisibility::Private
                    || !self.withholds(near, &far, &[])
                    || self.opaque_designator(&far).is_some()
                {
                    continue;
                }
                named
                    .entry(far.to_string())
                    .or_insert_with(|| (far.clone(), Dependents::default()))
                    .1
                    .insert(dependent.destination.clone());
            }
        }
        let mut each = Vec::new();
        for (key, (at, held)) in &running.recorders {
            each.push(Recording {
                at: at.clone(),
                named: named
                    .remove(key)
                    .map(|(_, named)| named)
                    .unwrap_or_default(),
                held: Some(held.clone()),
            });
        }
        each.extend(named.into_values().map(|(at, named)| Recording {
            at,
            named,
            held: None,
        }));
        let asked = running
            .dependents
            .iter()
            .filter(|dependent| dependent.asks_recorders)
            .map(|dependent| dependent.destination.clone())
            .collect();
        Recordings {
            decided,
            asked,
            each,
        }
    }

    /// Write each of `recordings` whose task does not already record what it is to: one read
    /// of the task and one write of it, through its own source, every other member as the
    /// source holds it.
    ///
    /// A task of a source declared private that an earlier copy recorded a dependent this copy
    /// landed on, and that this copy neither landed nor found named, is found by asking each
    /// source that may record one, and has that dependent taken off: the dependent's edges no
    /// longer name it, and this copy decides that dependent's entries wherever they are kept —
    /// so a dependent copied on its own, without the task it used to depend on, does not leave
    /// the removed edge readable from the far end.
    ///
    /// # Errors
    ///
    /// [`EngineError::NoSuchTask`] for a task its source no longer holds that a dependent this
    /// copy landed depends on, a record that does not say what it names, and whatever its
    /// source refuses.
    async fn record_dependents(
        &self,
        mut recordings: Recordings,
        journal: &mut Journal,
    ) -> Result<(), EngineError> {
        for dependent in &recordings.asked {
            for at in self.recorded_prerequisites(dependent).await? {
                if !recordings.each.iter().any(|recording| recording.at == at) {
                    recordings.each.push(Recording {
                        at,
                        named: Dependents::default(),
                        held: None,
                    });
                }
            }
        }
        for recording in &recordings.each {
            // A task this copy landed was read as it landed, so one already recording what it
            // is to is not read again.
            if let Some(held) = &recording.held
                && held.applied(&recordings.decided, &recording.named) == *held
            {
                continue;
            }
            let at = self.writable(&recording.at.source)?;
            // A task gone from its source has nothing left to take off, but one a dependent
            // this copy landed still names is refused rather than leaving that edge nowhere.
            let Some(prior) = self.prior(at, Level::Task, &recording.at.native).await? else {
                if recording.named.iter().next().is_none() {
                    continue;
                }
                return Err(EngineError::NoSuchTask {
                    id: recording.at.to_string(),
                });
            };
            let Item::Task(mut task) = prior.item.clone() else {
                continue;
            };
            let current = dependents::recorded(&task.metadata).map_err(|message| {
                refused(
                    at,
                    SourceError::Malformed {
                        message: format!("task {}: {message}", recording.at),
                    },
                )
            })?;
            let dependents = current.applied(&recordings.decided, &recording.named);
            if dependents == current {
                continue;
            }
            dependents::record(&mut task.metadata, &dependents);
            // A follow-up of this task is sent after this write, and carries what it records.
            journal.carry_into_follow_up(
                &recording.at.source,
                &recording.at.native,
                described(&prior.item).1,
                &task.metadata,
            );
            at.source()
                .check_status_write(
                    ItemKind::Task,
                    task.status.category,
                    Some(&recording.at.native),
                )
                .await
                .map_err(|error| refused(at, error))?;
            self.at_write(
                at,
                &recording.at.to_string(),
                &WriteTarget::Existing(&recording.at.native),
            )
            .await?;
            let edges = prior.edges.clone();
            journal.record(Undo::Updated {
                at: recording.at.source.clone(),
                id: recording.at.native.clone(),
                prior,
            });
            at.source()
                .write_task(&ItemWrite {
                    target: Some(recording.at.native.clone()),
                    item: *task,
                    depends_on: edges,
                })
                .await
                .map_err(|error| refused(at, error))?;
        }
        Ok(())
    }

    /// Write each home's changed `onetaskgraph.members` through its own project write, once the
    /// whole copy has landed.
    ///
    /// The home is read as it now stands and written back with that one entry changed, and
    /// the read is what the journal restores when the copy cannot finish — unless the copy
    /// already holds an earlier entry for it, which is what it was before this copy began.
    async fn record_members(
        &self,
        homes: BTreeMap<String, Home>,
        journal: &mut Journal,
    ) -> Result<(), EngineError> {
        for home in homes.into_values().filter(|home| home.changed) {
            let at = self.writable(&home.id.source)?;
            let Some(prior) = self.prior(at, Level::Project, &home.id.native).await? else {
                continue;
            };
            let Item::Project(mut project) = prior.item.clone() else {
                continue;
            };
            // A home left naming no member records no list, rather than an empty one.
            if home.members.is_empty() {
                project.metadata.remove(MetadataKey::MEMBERS_KEY);
            } else {
                project.metadata.insert(
                    MetadataKey::MEMBERS_KEY.to_owned(),
                    Value::Array(
                        home.members
                            .iter()
                            .map(|member| Value::String(member.to_string()))
                            .collect(),
                    ),
                );
            }
            // A follow-up of this home is sent after this write, and carries the same list.
            journal.carry_into_follow_up(
                &home.id.source,
                &home.id.native,
                described(&prior.item).1,
                &project.metadata,
            );
            let edges = prior.edges.clone();
            at.source()
                .check_status_write(
                    ItemKind::Project,
                    project.status.category,
                    Some(&home.id.native),
                )
                .await
                .map_err(|error| refused(at, error))?;
            self.at_write(
                at,
                &home.id.to_string(),
                &WriteTarget::Existing(&home.id.native),
            )
            .await?;
            journal.record(Undo::Updated {
                at: home.id.source.clone(),
                id: home.id.native.clone(),
                prior,
            });
            at.source()
                .write_project(&ItemWrite {
                    target: Some(home.id.native.clone()),
                    item: *project,
                    depends_on: edges,
                })
                .await
                .map_err(|error| refused(at, error))?;
        }
        Ok(())
    }

    /// The home project a source project's own `onetaskgraph.copies` names in a source this
    /// copy can reach, when the project it names there still records that source project as
    /// its origin — or records none, where the source is declared private and the source
    /// it names is not, because that copy withheld the origin (see [`Engine::target`]).
    ///
    /// The filing lookup's half of following a link: one read of the project at its source
    /// and one of its counterpart by id, in place of a walk of the destination's projects.
    /// A link that names nothing there, or names a project somebody re-pointed, is not a
    /// refusal here as it is for a copy's own target — this only decides where an item is
    /// filed — so the walk answers instead, exactly as it did before there were links. A
    /// withheld link naming nothing is the exception: no origin there can name a private
    /// project, so the walk would only file the item under an id the destination never held,
    /// and it is refused as a copy's own stale link is, unless the caller asked to recreate.
    ///
    /// A destination this command already asked which of its projects record `project` as
    /// their origin — the read ahead does, beside a document's own origin — confirms the link
    /// by that answer: the project its link names, recording `project`, is the one the
    /// destination answered it with, so it is not read again. Any other answer is left to the
    /// read below.
    async fn linked_project(
        &self,
        request: &CopyRequest,
        project: &GlobalId,
        read: Option<&Project>,
        asked_origins: &[(SourceName, GlobalId)],
    ) -> Result<Option<GlobalId>, EngineError> {
        let source = self.readable(&project.source)?;
        if !source.source().capabilities().projects.is_native() {
            return Ok(None);
        }
        let held = match read {
            Some(read) => read.clone(),
            None => {
                let Some(held) = source
                    .source()
                    .get_project(&project.native)
                    .await
                    .map_err(|error| refused(source, error))?
                else {
                    return Ok(None);
                };
                held
            }
        };
        for reachable in self.routes.reachable(&request.destination) {
            let Some(link) = link_of(&held.metadata, &reachable) else {
                continue;
            };
            let there = self.writable(&reachable)?;
            let withheld = self.declared(&project.source)
                == crate::config::SourceVisibility::Private
                && self.declared(&reachable) != crate::config::SourceVisibility::Private;
            let origin = project.to_string();
            if !withheld
                && asked_origins
                    .iter()
                    .any(|(asked, named)| asked == &reachable && named == project)
                && let Ok(answered) = there
                    .source()
                    .query_projects_by_origin(&origin, &request_for(there, None))
                    .await
                && answered.items.iter().any(|held| {
                    held.id == link.native && origin_of(&held.metadata).as_ref() == Some(project)
                })
            {
                return Ok(Some(link));
            }
            let named = there
                .source()
                .get_project(&link.native)
                .await
                .map_err(|error| refused(there, error))?;
            match named {
                Some(there)
                    if origin_of(&there.metadata).as_ref() == Some(project)
                        || (withheld && origin_of(&there.metadata).is_none()) =>
                {
                    return Ok(Some(link));
                }
                None if withheld && !request.recreate => {
                    return Err(EngineError::StaleLink {
                        item: project.to_string(),
                        link: link.to_string(),
                    });
                }
                _ => {}
            }
        }
        Ok(None)
    }

    /// What the destination holds at one id, item and forward edges together.
    ///
    /// One read for both purposes it serves — deciding whether a write changes anything,
    /// and putting the item back if the copy cannot finish — because a second read of the
    /// same item is a second round trip against a hosted destination for nothing.
    async fn prior(
        &self,
        destination: &ResolvedSource,
        kind: Level,
        id: &NativeId,
    ) -> Result<Option<Prior>, EngineError> {
        self.prior_reading(destination, kind, id, false).await
    }

    /// [`Engine::prior`], with every page of a task's comments there when `comments` asks and
    /// the destination has comments: the first page in the same detail read as the task.
    async fn prior_reading(
        &self,
        destination: &ResolvedSource,
        kind: Level,
        id: &NativeId,
        comments: bool,
    ) -> Result<Option<Prior>, EngineError> {
        let mut held_comments = None;
        let held = match kind {
            Level::Task if comments && destination.source().capabilities().comments.is_native() => {
                self.expect_copied(destination.name(), id);
                self.hold_copied(destination.name(), id);
                let mut whole = None;
                for _ in 0..2 {
                    let Some(read) = snapshot::first(destination, id)
                        .await
                        .map_err(|error| refused(destination, error))?
                    else {
                        return Ok(None);
                    };
                    match snapshot::take(destination, id, read)
                        .await
                        .map_err(|error| refused(destination, error))?
                    {
                        snapshot::Taken::Whole(taken) => {
                            whole = Some(taken);
                            break;
                        }
                        snapshot::Taken::Gone => return Ok(None),
                        snapshot::Taken::Unsettled => break,
                        snapshot::Taken::Changed => {}
                    }
                }
                let whole = whole.ok_or_else(|| {
                    refused(
                        destination,
                        SourceError::Unavailable {
                            message: format!(
                                "task {id} changed while the copy read it, twice; copy again \
                                 once it has settled"
                            ),
                        },
                    )
                })?;
                held_comments = Some(whole.comments);
                Some(Item::Task(Box::new(whole.task)))
            }
            Level::Task => {
                self.expect_copied(destination.name(), id);
                self.hold_copied(destination.name(), id);
                destination
                    .source()
                    .get_task(id)
                    .await
                    .map_err(|error| refused(destination, error))?
                    .map(|task| Item::Task(Box::new(task)))
            }
            Level::Project => destination
                .source()
                .get_project(id)
                .await
                .map_err(|error| refused(destination, error))?
                .map(|project| Item::Project(Box::new(project))),
            Level::Document => destination
                .source()
                .get_document(id)
                .await
                .map_err(|error| refused(destination, error))?
                .map(|document| Item::Document(Box::new(document))),
        };
        let Some(item) = held else {
            return Ok(None);
        };
        let edges = forward_edges(destination, id, kind).await?;
        let recorded =
            assets::recorded(described(&item).1).map_err(|error| refused(destination, error))?;
        let assets = held_assets(destination, kind, id, recorded.as_ref()).await?;
        Ok(Some(Prior {
            item,
            edges,
            assets,
            recorded,
            comments: held_comments,
            derived: Vec::new(),
        }))
    }

    /// Hand one item to the destination's own write interface, recording how to take it
    /// back.
    // llmlint: ignore[suppressions_justified] A write is the item, where it is going, what
    // it is filed under, its edges, what was there before and the journal that records how
    // to put it back. Each is a distinct decision made by a different part of the copy, and
    // grouping them would only move the argument list to a constructor.
    #[allow(clippy::too_many_arguments)]
    async fn write(
        &self,
        destination: &ResolvedSource,
        item: &Planned,
        target: Option<NativeId>,
        project: Option<NativeId>,
        edges: &[DependencyEdge],
        delivers: &[TaskRef],
        prior: Option<Prior>,
        stamping: Stamping,
        journal: &mut Journal,
    ) -> Result<NativeId, EngineError> {
        // A further write of an item whose create this copy sent renews its lease first.
        self.renew_owned(&item.source, destination.name(), journal)
            .await?;
        let created_kind = item.item.level();
        let suggested = target
            .clone()
            .unwrap_or_else(|| created_id(&item.item, project.as_ref()));
        // Settled before the journal takes `prior`, and from that same read: what the
        // destination holds at the origin key is what a copy-back leaves there, and what it
        // holds as `delivered_by` is what the item keeps.
        let origin = recorded(item, prior.as_ref());
        let mut landing = as_written(
            destination,
            item,
            outgoing(
                item,
                suggested.clone(),
                project.clone(),
                &origin,
                delivers,
                prior.as_ref(),
            ),
        );
        // Its content's links, rendered once more against everything this copy has landed
        // before it: a record written earlier in the copy is addressable now.
        let prior_location = prior
            .as_ref()
            .and_then(|prior| located(location_of(&prior.item)));
        let mut pending = Vec::new();
        if let Some(links) = &item.links {
            let knows = Writing {
                destination,
                landed: &journal.landed,
                own: target.clone().zip(prior_location.clone()),
            };
            if let Some(content) = content_of(&landing).map(str::to_owned) {
                let (finished, still) =
                    links.finished(&content, &item.pending, &knows, sections::Pass::Provisional);
                if finished != content {
                    stored::restamp(metadata_of(&mut landing), &[&content], &finished);
                    set_content(&mut landing, finished);
                }
                pending = still;
            }
            journal.sections.insert(
                item.source.clone(),
                SectionLinks::of(links.render(&knows, sections::Pass::Final).sections, 0),
            );
        }
        // A task lands with the record of this copy — the source's token it read, whether it
        // carried the comments, and the digest of what it leaves here — when `stamping` says
        // this write records it; otherwise with whatever record the destination already holds,
        // which `outgoing` carried.
        // Onto a destination not declared private, a task whose every edge is on its own record
        // — none withheld and kept on a private source's side — says so, so a dependency read
        // of it never asks a private source what it records of it.
        if let Item::Task(task) = &mut landing
            && self.declared(destination.name()) != crate::config::SourceVisibility::Private
        {
            // Only a task carrying a copy's record is ever read for what a private source keeps
            // of it, so only one is marked.
            let recorded = stamping.stamps(task, prior.as_ref())
                || task
                    .metadata
                    .contains_key(MetadataKey::COPIED_DESTINATION_VERSION_KEY);
            if edges.len() != item.edges.len() {
                task.metadata.remove(MetadataKey::EDGES_ON_RECORD_KEY);
            } else if recorded {
                task.metadata
                    .insert(MetadataKey::EDGES_ON_RECORD_KEY.to_owned(), Value::from(1));
            }
        }
        // A project and a document carry the record on every write that records one: a write of
        // either sends its whole long form, so there is no member to write on its own instead.
        let stamped = match &landing {
            Item::Task(task) => stamping.stamps(task, prior.as_ref()),
            Item::Project(_) | Item::Document(_) => stamping != Stamping::Never,
        };
        let digested = self.digested_comments(destination, item, &suggested, journal);
        let record = stamped.then(|| {
            record_of(
                item,
                &landing,
                edges,
                destination.name(),
                prior.as_ref().and_then(|prior| prior.recorded.as_ref()),
                digested.clone(),
            )
        });
        if let Some(record) = &record {
            record.stamp(metadata_of(&mut landing));
        }
        // Before anything is sent: a destination declared private is held to what this copy
        // read of it, before its first write, for this write's destination — except the write
        // that completes a task created together with others, which is the same write its
        // create was checked for a moment before, finished once the others it names exist.
        if stamping != Stamping::Never {
            self.at_write(
                destination,
                &item.source.to_string(),
                &match &target {
                    Some(id) => WriteTarget::Existing(id),
                    None => WriteTarget::New {
                        repositories: repositories_of(&item.item),
                        project: project.as_ref(),
                    },
                },
            )
            .await?;
        }
        // Asked before the journal records anything or the destination is sent anything: a
        // status the destination has no name for refuses this write while there is nothing to
        // put back, where refused inside the write it would leave the journal restoring an
        // item the write never touched.
        check_status(destination, &landing, target.as_ref()).await?;
        // Recorded *before* the write rather than after it. A destination's own write is
        // several calls — `docs/plugin-protocol.md` §4.9 — and one of them failing leaves
        // the ones before it applied. No source can put those back, because only this
        // journal holds what was there; recorded after a successful write, an update that
        // stopped part way was the one way a copy could end and leave the destination
        // altered. A restore of an item the write never reached rewrites what is already
        // there, which costs one mutation and is what "either complete or it never
        // happened" is worth.
        // A write carries assets when the item's content references any, and when the item it
        // overwrites holds any: what lands then holds exactly the assets this copy carries.
        let held_assets = prior.as_ref().and_then(|prior| prior.assets.clone());
        let carrying = !item.assets.is_empty() || held_assets.is_some();
        let recorded = prior.as_ref().and_then(|prior| prior.recorded.clone());
        if let (Some(id), Some(mut prior)) = (target.clone(), prior) {
            if carrying && prior.assets.is_none() {
                // Putting this item back then has to take away the assets this write adds.
                prior.assets = Some(Vec::new());
            }
            journal.record(Undo::Updated {
                at: destination.name().clone(),
                id,
                prior,
            });
        }
        // A create whose answer is malformed may have stored what it was sent under an id this
        // copy never learns, so it cannot be journalled; that is said rather than reported as
        // a source to fix and copy again. An update was journalled before it was sent.
        let failed = |operation: Creating, error| match &target {
            None => create_refused(destination, operation, error),
            Some(_) => refused(destination, error),
        };
        // What this write's own answer says the destination stores is what is compared with
        // what it sends, so nothing an earlier write of the item left behind is.
        if let Some(id) = &target {
            stored::forget(destination, created_kind.record(), id);
        }
        if target.is_none() {
            journal.sending.push(item.source.clone());
            if item.intent.is_some() {
                journal.owned.push((
                    item.item.level(),
                    item.source.clone(),
                    destination.name().clone(),
                ));
            }
        }
        let mut sent = match &landing {
            Item::Task(task) => task.content.clone(),
            Item::Project(project) => project.content.clone(),
            Item::Document(document) => document.content.clone(),
        };
        // What a follow-up write starts from, should this write's answer say the destination
        // stores other content than it was sent.
        let composed = landing.clone();
        let owned = Self::owned_at(journal, Some(&item.source), destination.name());
        let landed = self
            .guarded(owned, journal, async {
                Ok::<NativeId, EngineError>(match landing {
                    Item::Task(task) if carrying => destination
                        .source()
                        .write_task_with_assets(
                            &ItemWrite {
                                target: target.clone(),
                                item: *task,
                                depends_on: edges.to_vec(),
                            },
                            None,
                            &assets::write_of(item.assets.clone(), recorded.clone()),
                        )
                        .await
                        .map(|written| {
                            sent = written.content;
                            written.id
                        })
                        .map_err(|error| failed(Creating::Task, error))?,
                    Item::Document(document) if carrying => destination
                        .source()
                        .write_document_with_assets(
                            &ItemWrite {
                                target: target.clone(),
                                item: *document,
                                depends_on: Vec::new(),
                            },
                            None,
                            &assets::write_of(item.assets.clone(), recorded.clone()),
                        )
                        .await
                        .map(|written| {
                            sent = written.content;
                            written.id
                        })
                        .map_err(|error| failed(Creating::Document, error))?,
                    Item::Task(task) => destination
                        .source()
                        .write_task(&ItemWrite {
                            target: target.clone(),
                            item: *task,
                            depends_on: edges.to_vec(),
                        })
                        .await
                        .map_err(|error| failed(Creating::Task, error))?,
                    Item::Project(project) => destination
                        .source()
                        .write_project(&ItemWrite {
                            target: target.clone(),
                            item: *project,
                            depends_on: edges.to_vec(),
                        })
                        .await
                        .map_err(|error| failed(Creating::Project, error))?,
                    // No edges, and that is the contract: a document takes part in no dependency
                    // graph, so there is nothing here for `depends_on` to carry.
                    Item::Document(document) => destination
                        .source()
                        .write_document(&ItemWrite {
                            target: target.clone(),
                            item: *document,
                            depends_on: Vec::new(),
                        })
                        .await
                        .map_err(|error| failed(Creating::Document, error))?,
                })
            })
            .await??;
        // A created item can only be journalled here: its id is what the write answers
        // with. A create that fails leaves nothing behind — §4.9 makes taking the item
        // back the source's own duty, because a write that refused must not leave an item
        // nobody asked for. One that answered malformed is the exception no source can be
        // held to, which is why `failed` reports it as a destination this copy cannot vouch for.
        if target.is_none() {
            journal.sending.retain(|sending| sending != &item.source);
            journal.record(Undo::Created {
                at: destination.name().clone(),
                kind: created_kind,
                id: landed.clone(),
            });
        }
        // Only this item's latest write is the one its follow-up answers: a repair that writes it
        // again supersedes what the first write left to do, and one stored as sent needs none.
        journal
            .follow_ups
            .retain(|follow| !(&follow.at == destination.name() && follow.id == landed));
        journal
            .landed
            .insert((created_kind, item.source.clone()), landed.clone());
        if created_kind != Level::Task
            && let Some(address) = located(
                destination
                    .source()
                    .written_location(created_kind.record(), &landed)
                    .as_ref(),
            )
            .or_else(|| prior_location.clone())
        {
            journal
                .titles
                .entry(destination.name().clone())
                .or_default()
                .insert(address, described(&composed).0.to_owned());
        }
        journal
            .sections_pending
            .retain(|held| !(&held.at == destination.name() && held.id == landed));
        if let Some(links) = &item.links
            && !pending.is_empty()
        {
            let own = located(
                destination
                    .source()
                    .written_location(created_kind.record(), &landed)
                    .as_ref(),
            )
            .or(prior_location)
            .map(|location| (landed.clone(), location));
            journal.sections_pending.push(SectionsPending {
                at: destination.name().clone(),
                id: landed.clone(),
                source: item.source.clone(),
                links: links.clone(),
                pending,
                own,
                item: composed.clone(),
                edges: edges.to_vec(),
                record: record.clone(),
                comments: digested.clone(),
            });
        }
        if let (Some(record), Some(comments)) = (&record, &digested) {
            journal.stamped.insert(
                (destination.name().clone(), landed.clone()),
                Stamped {
                    source: item.source.clone(),
                    item: composed.clone(),
                    edges: edges.to_vec(),
                    record: record.clone(),
                    comments: comments.clone(),
                },
            );
        }
        if let Some(stored) =
            stored::rewritten(destination, created_kind.record(), &landed, sent.as_deref())
            && let Some(follow) = follow_up_of(
                item,
                composed,
                &landed,
                edges,
                destination.name(),
                sent.as_deref(),
                stored,
                record,
                digested,
            )
        {
            journal.follow_ups.push(follow);
        }
        Ok(landed)
    }

    /// Note, on each task this call updates on a destination that relates records by itself, every
    /// `related` edge it holds to another task of this call when the body this call writes for
    /// either of the two links the other. Such an edge is the destination's, not a person's or a
    /// copy's, so it is no change there. Any other `related` edge still is, and so is one to a task
    /// outside this call, whose body it does not hold.
    fn note_derived_relations(&self, planned: &mut [Planned]) -> Result<(), EngineError> {
        let mut ends: Vec<(SourceName, NativeId, String, String)> = Vec::new();
        for item in planned.iter() {
            let (Target::Update { id, .. }, Some(held), Item::Task(task)) =
                (&item.target, &item.held, &item.item)
            else {
                continue;
            };
            if !SectionForm::for_kind(self.writable(&item.to)?.kind())
                .is_some_and(SectionForm::derives_related_edges)
            {
                continue;
            }
            if let Some(location) = located(location_of(&held.item)) {
                ends.push((
                    item.to.clone(),
                    id.clone(),
                    location,
                    task.content.clone().unwrap_or_default(),
                ));
            }
        }
        for item in planned.iter_mut() {
            let Target::Update { id, .. } = &item.target else {
                continue;
            };
            let Some(near) = ends.iter().find(|end| end.0 == item.to && end.1 == *id) else {
                continue;
            };
            let Some(held) = item.held.as_mut() else {
                continue;
            };
            held.derived = held
                .edges
                .iter()
                .filter(|edge| edge.kind == DependencyKind::Related)
                .map(|edge| edge.to.clone())
                .filter(|far| {
                    let far_id = far.to_string();
                    ends.iter().any(|end| {
                        let named = far_id == end.1.as_str()
                            || far_id == GlobalId::new(end.0.clone(), end.1.clone()).to_string();
                        end.0 == near.0
                            && named
                            && (links_to(&near.3, &end.2) || links_to(&end.3, &near.2))
                    })
                })
                .collect();
        }
        Ok(())
    }

    /// Send each follow-up write `journal` holds, as its own request, and answer every item whose
    /// follow-up was answered with other content than it sent back.
    ///
    /// Run once every link of the copy is written, so a record whose version is its source's
    /// change token records the token the source answered that link write with — a source that
    /// tokens its whole item moves it when the link lands — and a later copy of an unchanged
    /// source finds it unchanged. A follow-up that fails fails the copy, which then undoes every
    /// write it made; one answered with other content is landed and reported `failed`.
    async fn finish_sections(&self, journal: &mut Journal) -> Result<(), EngineError> {
        for held in std::mem::take(&mut journal.comments_pending) {
            let destination = self.writable(&held.at)?;
            let knows = Writing {
                destination,
                landed: &journal.landed,
                own: located(
                    destination
                        .source()
                        .written_location(MetadataRecord::Task, &held.task)
                        .as_ref(),
                )
                .map(|location| (held.task.clone(), location)),
            };
            let (finished, _) = held.links.finished(
                held.copy.body.as_str(),
                &held.pending,
                &knows,
                sections::Pass::Final,
            );
            journal
                .comment_sections
                .entry(held.source.clone())
                .or_default()
                .add(held.links.render(&knows, sections::Pass::Final).sections);
            if finished == held.copy.body.as_str() {
                continue;
            }
            let Ok(body) = onetaskgraph_plugin_api::CommentBody::new(finished) else {
                continue;
            };
            let copy = CommentCopy {
                body,
                origin: held.copy.origin.clone(),
            };
            let edited = destination
                .source()
                .write_comment_copy(&held.task, Some(&held.comment), &copy)
                .await
                .map_err(|error| refused(destination, error))?;
            // The task's record digests its comments as they now read.
            if let Some(stamped) = journal.stamped.get(&(held.at.clone(), held.task.clone())) {
                let mut comments = stamped.comments.clone();
                comments::reword(&mut comments, &copy.origin, copy.body.as_str());
                Self::restamp_comments(journal, &held.at, &held.task, comments);
            }
            match edited {
                Some(_) => journal.comments.push(CommentUndo {
                    at: held.at.clone(),
                    task: held.task.clone(),
                    undo: CommentUndoing::Edited {
                        id: held.comment.clone(),
                        before: held.copy,
                    },
                }),
                // Removed since it was written: the copy is written afresh, as a comment edit of
                // the copy's plan is when its comment has gone.
                None => {
                    let added = destination
                        .source()
                        .write_comment_copy(&held.task, None, &copy)
                        .await
                        .map_err(|error| refused(destination, error))?
                        .ok_or_else(|| EngineError::NoSuchTask {
                            id: GlobalId::new(held.at.clone(), held.task.clone()).to_string(),
                        })?;
                    journal.comments.push(CommentUndo {
                        at: held.at.clone(),
                        task: held.task.clone(),
                        undo: CommentUndoing::Added(added.id),
                    });
                }
            }
            journal
                .sections
                .entry(held.source)
                .or_default()
                .section_link_updates += 1;
        }
        for held in std::mem::take(&mut journal.sections_pending) {
            let destination = self.writable(&held.at)?;
            let knows = Writing {
                destination,
                landed: &journal.landed,
                own: held.own.clone(),
            };
            let figures = held.links.render(&knows, sections::Pass::Final).sections;
            // A destination that stores other content than it was sent is already written once
            // more with what it stored: its links are finished in that write.
            if let Some(follow) = journal
                .follow_ups
                .iter_mut()
                .find(|follow| follow.at == held.at && follow.id == held.id)
            {
                let stored = content_of(&follow.item).unwrap_or_default().to_owned();
                let (finished, _) =
                    held.links
                        .finished(&stored, &held.pending, &knows, sections::Pass::Final);
                if finished != stored {
                    stored::restamp(metadata_of(&mut follow.item), &[&stored], &finished);
                    set_content(&mut follow.item, finished.clone());
                    follow.stored = finished;
                    follow.answer = FollowUpAnswer::MayNormalize;
                }
                settle(journal, &held.source, figures, 0);
                continue;
            }
            let sent = content_of(&held.item).unwrap_or_default().to_owned();
            let (finished, _) =
                held.links
                    .finished(&sent, &held.pending, &knows, sections::Pass::Final);
            let updates = u64::from(finished != sent);
            settle(journal, &held.source, figures, updates);
            if updates == 0 {
                continue;
            }
            let mut item = held.item;
            stored::restamp(metadata_of(&mut item), &[&sent], &finished);
            set_content(&mut item, finished.clone());
            journal.follow_ups.push(FollowUp {
                at: held.at,
                id: held.id,
                source: held.source,
                item,
                edges: held.edges,
                record: held.record,
                comments: held.comments,
                stored: finished,
                answer: FollowUpAnswer::MayNormalize,
            });
        }
        Ok(())
    }

    async fn follow_up(
        &self,
        journal: &mut Journal,
    ) -> Result<Vec<(GlobalId, EngineError)>, EngineError> {
        let mut diverged = Vec::new();
        let mut owed = std::collections::VecDeque::from(std::mem::take(&mut journal.follow_ups));
        while let Some(mut follow) = owed.pop_front() {
            let destination = self.writable(&follow.at)?;
            let record_again = follow.record.clone();
            if let Some(mut record) = follow.record.take() {
                if let record::SourceVersion::Token(token) = &mut record.version
                    && let Some(version) = journal.linked_versions.get(&follow.source)
                {
                    token.clone_from(version);
                }
                record.digest = record::digest_of(
                    &follow.item,
                    &follow.edges,
                    follow.comments.clone(),
                    &follow.at,
                );
                record.stamp(metadata_of(&mut follow.item));
            }
            self.at_write(
                destination,
                &follow.source.to_string(),
                &WriteTarget::Existing(&follow.id),
            )
            .await?;
            let kind = follow.item.level().record();
            stored::forget(destination, kind, &follow.id);
            let target = Some(follow.id.clone());
            let written = match follow.item.clone() {
                Item::Task(task) => {
                    destination
                        .source()
                        .write_task(&ItemWrite {
                            target,
                            item: *task,
                            depends_on: follow.edges.clone(),
                        })
                        .await
                }
                Item::Project(project) => {
                    destination
                        .source()
                        .write_project(&ItemWrite {
                            target,
                            item: *project,
                            depends_on: follow.edges.clone(),
                        })
                        .await
                }
                Item::Document(document) => {
                    destination
                        .source()
                        .write_document(&ItemWrite {
                            target,
                            item: *document,
                            depends_on: Vec::new(),
                        })
                        .await
                }
            }
            .map_err(|error| refused(destination, error))?;
            // A follow-up is an update of the item it names, and an answer naming another is one
            // this copy cannot say what was written to.
            if written != follow.id {
                return Err(refused(
                    destination,
                    SourceError::Malformed {
                        message: format!(
                            "the follow-up write of {} answered with the item {written}",
                            follow.id
                        ),
                    },
                ));
            }
            if follow.answer == FollowUpAnswer::Exact {
                if let Some(error) = stored::unstable(destination, kind, &follow.id, &follow.stored)
                {
                    diverged.push((follow.source, error));
                }
                continue;
            }
            let titles = journal.titles.get(&follow.at).cloned().unwrap_or_default();
            match stored::normalized(destination, kind, &follow.id, &follow.stored, &titles) {
                stored::Normalized::Kept => {}
                stored::Normalized::Diverged(error) => diverged.push((follow.source, error)),
                // What the destination made of text of this copy's own, which a read now reports:
                // written once more beside the fingerprints of it, so a repeat copy finds them
                // true. Its answer has to report it byte for byte, as any stored content's does.
                stored::Normalized::Stored(held) => {
                    stored::restamp(metadata_of(&mut follow.item), &[&follow.stored], &held);
                    set_content(&mut follow.item, held.clone());
                    follow.stored = held;
                    follow.record = record_again;
                    follow.answer = FollowUpAnswer::Exact;
                    owed.push_back(follow);
                }
            }
        }
        Ok(diverged)
    }

    /// A configured source that built, for reading an item out of.
    pub(super) fn readable(&self, name: &SourceName) -> Result<&ResolvedSource, EngineError> {
        let name = self.known(name)?;
        if let Some(unavailable) = self.unavailable().find(|source| source.name() == &name) {
            return Err(EngineError::SourceRefused {
                name: name.to_string(),
                error: unavailable.error().clone(),
            });
        }
        self.ready()
            .find(|source| source.name() == &name)
            .ok_or(EngineError::NoSources)
    }
}

/// What a copy's walk of a project's members says it was for, when a source refuses it.
const FOR_A_COPY: &str = "for a copy";

/// How many tasks of a project are read at once while walking it.
pub(super) const PROJECT_PAGE: std::num::NonZeroU32 =
    std::num::NonZeroU32::new(50).expect("50 is not zero");

/// Each source's own running totals, in the order the sources were given.
///
/// A reading a source could not take is read as that source not metering: what a command
/// spent is a report about the work, and a failed reading must not become a failure of the
/// work itself. That holds when only one of a source's two readings failed as well, since
/// a difference needs both ends.
pub(super) async fn readings(sources: &[&ResolvedSource]) -> Vec<Option<Metering>> {
    let mut read = Vec::with_capacity(sources.len());
    for source in sources {
        read.push(source.source().metering().await.ok().flatten());
    }
    read
}

/// What the sources spent between two readings of them, or `None` when none of them meters.
///
/// A source that does not meter contributes nothing and does not make the total zero: a
/// command whose sources all declined reports that it cannot say, not that it spent nothing.
/// A budget is keyed by its name and unit together, so two sources naming one budget add up
/// and two units of one budget stay apart.
///
/// A source's two readings are a plugin's word, so they are held to [`Metering`]'s contract
/// before either is believed, and a pair that breaks it is that source not metering — see
/// [`difference`].
pub(crate) fn spent_between(
    before: &[Option<Metering>],
    after: &[Option<Metering>],
) -> Option<Spent> {
    let mut metered = false;
    let mut requests = 0_u64;
    let mut budgets: BTreeMap<(String, String), (u64, u64)> = BTreeMap::new();
    for (before, after) in before.iter().zip(after) {
        let (Some(before), Some(after)) = (before, after) else {
            continue;
        };
        let Some((sent, spent)) = difference(before, after) else {
            continue;
        };
        metered = true;
        requests = requests.saturating_add(sent);
        for (key, measured, modelled) in spent {
            let total = budgets.entry(key).or_default();
            total.0 = total.0.saturating_add(measured).saturating_add(modelled);
            total.1 = total.1.saturating_add(modelled);
        }
    }
    metered.then(|| Spent {
        requests,
        budgets: budgets
            .into_iter()
            .map(|((budget, unit), (amount, modelled))| BudgetSpent {
                budget,
                unit,
                amount,
                lower_bound: modelled > 0,
            })
            .collect(),
    })
}

/// One budget's name and unit, and the measured and modelled amounts spent against it.
type BudgetDifference = ((String, String), u64, u64);

/// What one source sent and spent between two of its readings, or `None` when the pair
/// cannot be a running total's.
///
/// A running total never falls, never drops a budget it has named, and names every budget
/// it keeps, once; a pair that breaks any of that is a source that reset its figures or
/// reported something else, and a difference taken over it would be a number that measures
/// nothing. Such a source is reported as not metering rather than as having spent a clamped
/// zero.
fn difference(before: &Metering, after: &Metering) -> Option<(u64, Vec<BudgetDifference>)> {
    let sent = after.requests.checked_sub(before.requests)?;
    for reading in [before, after] {
        let mut named = BTreeSet::new();
        for budget in &reading.budgets {
            if budget.budget.is_empty()
                || budget.unit.is_empty()
                || !named.insert((&budget.budget, &budget.unit))
            {
                return None;
            }
        }
    }
    let held = |from: &Metering, budget: &onetaskgraph_plugin_api::Metered| {
        from.budgets
            .iter()
            .find(|held| held.budget == budget.budget && held.unit == budget.unit)
            .cloned()
    };
    if before
        .budgets
        .iter()
        .any(|budget| held(after, budget).is_none())
    {
        return None;
    }
    let mut spent = Vec::with_capacity(after.budgets.len());
    for budget in &after.budgets {
        let earlier = held(before, budget);
        let measured = budget
            .measured
            .checked_sub(earlier.as_ref().map_or(0, |held| held.measured))?;
        let modelled = budget
            .modelled
            .checked_sub(earlier.as_ref().map_or(0, |held| held.modelled))?;
        spent.push((
            (budget.budget.clone(), budget.unit.clone()),
            measured,
            modelled,
        ));
    }
    Some((sent, spent))
}

/// One page request against `source`, at the largest page it will serve.
fn request_for(source: &ResolvedSource, cursor: Option<Cursor>) -> PageRequest {
    PageRequest {
        cursor,
        limit: source.source().capabilities().max_page_size.max(1),
    }
}

/// Whether writing this item would change what the destination already holds.
///
/// A free function over the state already read rather than a method that reads it again:
/// the same answer is wanted where the item is landed and where a repeat copy of a project
/// decides whether it settled, and a second read there is a second round trip for nothing.
#[expect(
    clippy::too_many_arguments,
    reason = "each is one part of what the write would send, and whether the source's token is \
              trusted to say it is unchanged"
)]
fn changes(
    held: Option<&Prior>,
    item: &Planned,
    target: &NativeId,
    project: &Option<NativeId>,
    edges: &[DependencyEdge],
    delivers: &[TaskRef],
    destination: &ResolvedSource,
    covers: bool,
) -> bool {
    let Some(held) = held else {
        return true;
    };
    let mut outgoing = as_written(
        destination,
        item,
        outgoing(
            item,
            target.clone(),
            project.clone(),
            &recorded(item, Some(held)),
            delivers,
            Some(held),
        ),
    );
    if (!item.assets.is_empty() || held.assets.is_some())
        && !lands_as_held(item, held, &mut outgoing)
    {
        return true;
    }
    // An assignee the copy writes is decided against the destination as read, whatever the
    // record says: a guard met over an assignee somebody else set is a change the record
    // never saw.
    if let (Item::Task(held), Item::Task(outgoing)) = (&held.item, &outgoing)
        && !writes_held_assignees(held, outgoing)
    {
        return true;
    }
    if recorded_unchanged(item, held, &outgoing, edges, destination.name(), covers) {
        return false;
    }
    !same(&held.item, &outgoing, destination.name())
        || !same_edges(&held.counted_edges(edges), edges)
}

/// `item` with its status as `destination` reads it back once written — see
/// [`TaskSource::status_as_written`](onetaskgraph_plugin_api::TaskSource::status_as_written).
///
/// What a copy compares with the destination, and what the record it leaves there digests, is
/// what the destination will hold: a board that writes a category to an option of its own
/// reads `todo` back as `Todo`, and comparing the spelling the source gave would call every
/// such item changed on every copy, and record a digest no later read of it could match.
fn as_written(destination: &ResolvedSource, planned: &Planned, mut item: Item) -> Item {
    if planned.status_policy == StatusPolicy::PreserveDestination && planned.held.is_some() {
        return item;
    }
    let source = destination.source();
    match &mut item {
        Item::Task(task) => task.status = source.status_as_written(ItemKind::Task, &task.status),
        Item::Project(project) => {
            project.status = source.status_as_written(ItemKind::Project, &project.status);
        }
        Item::Document(_) => {}
    }
    item
}

/// Whether `held` already holds exactly the assets `item` carries, by name and digest — and,
/// when it does, `outgoing` made what the destination would store of it.
///
/// A destination that serves its assets at a URL says what it holds in its own record,
/// `onetaskgraph.assets`, and would rewrite the references to the URLs recorded there; one
/// that keeps them beside the record says so in the assets it lists, and stores the references
/// as written.
fn lands_as_held(item: &Planned, held: &Prior, outgoing: &mut Item) -> bool {
    let Some(recorded) = &held.recorded else {
        let holds: Vec<onetaskgraph_plugin_api::Asset> = held
            .assets
            .iter()
            .flatten()
            .map(|payload| onetaskgraph_plugin_api::Asset {
                name: payload.name.clone(),
                sha256: payload.sha256.clone(),
                content_type: payload.content_type,
                path: None,
            })
            .collect();
        return assets::same_set(&holds, &item.assets);
    };
    if recorded.0.len() != item.assets.len() {
        return false;
    }
    let mut served = AssetUploads::default();
    for payload in &item.assets {
        let Some(url) = recorded.reusable(&payload.name, &payload.sha256) else {
            return false;
        };
        served.0.insert(
            payload.name.clone(),
            onetaskgraph_plugin_api::AssetUpload {
                sha256: payload.sha256.clone(),
                url: url.to_owned(),
            },
        );
    }
    let (content, metadata) = match outgoing {
        Item::Task(task) => (&mut task.content, &mut task.metadata),
        Item::Document(document) => (&mut document.content, &mut document.metadata),
        Item::Project(_) => return true,
    };
    if let Some(text) = content {
        *text = onetaskgraph_plugin_api::serve_asset_references(text, metadata, &served);
    }
    true
}

/// The image assets `item`'s content references, each with its bytes as `source` holds them,
/// in the order the content first references them — none for a project.
///
/// # Errors
///
/// [`EngineError::AssetNotHeld`] naming the record and the asset for a reference `source` holds
/// no asset for, and [`EngineError::SourceRefused`] when it cannot answer.
async fn carried_assets(
    source: &ResolvedSource,
    id: &GlobalId,
    item: &Item,
) -> Result<Vec<AssetPayload>, EngineError> {
    let (content, owner) = match item {
        Item::Task(task) => (task.content.as_deref(), assets::Owner::Task),
        Item::Document(document) => (document.content.as_deref(), assets::Owner::Document),
        Item::Project(_) => return Ok(Vec::new()),
    };
    let referenced = onetaskgraph_plugin_api::asset_references(content.unwrap_or_default());
    if referenced.is_empty() {
        return Ok(Vec::new());
    }
    let listed = match owner {
        assets::Owner::Document => source.source().document_assets(&id.native).await,
        assets::Owner::Task => source.source().task_assets(&id.native).await,
    }
    .map_err(|error| refused(source, error))?;
    let mut carried = Vec::with_capacity(referenced.len());
    for name in referenced {
        let not_held = || EngineError::AssetNotHeld {
            record: id.to_string(),
            asset: name.to_string(),
        };
        if !listed.iter().any(|asset| asset.name == name) {
            return Err(not_held());
        }
        let bytes = match owner {
            assets::Owner::Document => source.source().document_asset(&id.native, &name).await,
            assets::Owner::Task => source.source().task_asset(&id.native, &name).await,
        }
        .map_err(|error| refused(source, error))?
        .ok_or_else(not_held)?;
        carried.push(AssetPayload::of(name, bytes));
    }
    Ok(carried)
}

/// The image assets the item `id` of kind `kind` holds at `destination`, as [`Prior::assets`]
/// keeps them: `None` for a destination that stores none, for a project, and for an item that
/// holds none.
async fn held_assets(
    destination: &ResolvedSource,
    kind: Level,
    id: &NativeId,
    recorded: Option<&AssetUploads>,
) -> Result<Option<Vec<AssetPayload>>, EngineError> {
    let owner = match kind {
        Level::Task => assets::Owner::Task,
        Level::Document => assets::Owner::Document,
        Level::Project => return Ok(None),
    };
    if !destination.source().capabilities().assets.is_native() {
        return Ok(None);
    }
    let listed = match owner {
        assets::Owner::Document => destination.source().document_assets(id).await,
        assets::Owner::Task => destination.source().task_assets(id).await,
    }
    .map_err(|error| refused(destination, error))?;
    if listed.is_empty() {
        // A destination that lists none may still serve some — one reached over the stdio
        // protocol lists nothing — and its own record of what it uploaded is then what it holds,
        // every one of them reusable by its digest without reading a byte back.
        return Ok(recorded
            .filter(|recorded| !recorded.0.is_empty())
            .map(|recorded| {
                recorded
                    .0
                    .iter()
                    .map(|(name, upload)| AssetPayload {
                        name: name.clone(),
                        sha256: upload.sha256.clone(),
                        content_type: name.content_type(),
                        bytes: None,
                    })
                    .collect()
            }));
    }
    let mut held = Vec::with_capacity(listed.len());
    for asset in listed {
        // Bytes the destination's own record says it serves are not read back: putting them
        // back reuses what it serves.
        let bytes = if recorded
            .is_some_and(|recorded| recorded.reusable(&asset.name, &asset.sha256).is_some())
        {
            None
        } else {
            match owner {
                assets::Owner::Document => {
                    destination.source().document_asset(id, &asset.name).await
                }
                assets::Owner::Task => destination.source().task_asset(id, &asset.name).await,
            }
            .map_err(|error| refused(destination, error))?
        };
        held.push(AssetPayload {
            name: asset.name,
            sha256: asset.sha256,
            content_type: asset.content_type,
            bytes,
        });
    }
    Ok(Some(held))
}

/// Remove one item this copy created, through the destination's own write interface.
async fn remove(
    destination: &ResolvedSource,
    kind: Level,
    id: &NativeId,
) -> Result<(), SourceError> {
    match kind {
        Level::Task => destination.source().delete_task(id).await,
        Level::Project => destination.source().delete_project(id).await,
        Level::Document => destination.source().delete_document(id).await,
    }
}

/// Write one item back exactly as the destination held it before this copy.
async fn restore(
    destination: &ResolvedSource,
    id: &NativeId,
    prior: &Prior,
) -> Result<(), SourceError> {
    if let Some(held) = &prior.assets {
        let assets = AssetWrite {
            assets: held.clone(),
            recorded_assets: prior.recorded.clone(),
        };
        match &prior.item {
            Item::Task(task) => {
                return destination
                    .source()
                    .write_task_with_assets(
                        &ItemWrite {
                            target: Some(id.clone()),
                            item: (**task).clone(),
                            depends_on: prior.edges.clone(),
                        },
                        None,
                        &assets,
                    )
                    .await
                    .map(|_| ());
            }
            Item::Document(document) => {
                return destination
                    .source()
                    .write_document_with_assets(
                        &ItemWrite {
                            target: Some(id.clone()),
                            item: (**document).clone(),
                            depends_on: Vec::new(),
                        },
                        None,
                        &assets,
                    )
                    .await
                    .map(|_| ());
            }
            Item::Project(_) => {}
        }
    }
    match &prior.item {
        Item::Task(task) => destination
            .source()
            .write_task(&ItemWrite {
                target: Some(id.clone()),
                item: (**task).clone(),
                depends_on: prior.edges.clone(),
            })
            .await
            .map(|_| ()),
        Item::Project(project) => destination
            .source()
            .write_project(&ItemWrite {
                target: Some(id.clone()),
                item: (**project).clone(),
                depends_on: prior.edges.clone(),
            })
            .await
            .map(|_| ()),
        Item::Document(document) => destination
            .source()
            .write_document(&ItemWrite {
                target: Some(id.clone()),
                item: (**document).clone(),
                depends_on: Vec::new(),
            })
            .await
            .map(|_| ()),
    }
}

/// Hold `value` at `key` — `onetaskgraph.copies` or `onetaskgraph.copy_locations` — on one
/// item, through its source's narrow metadata write: `false` when the source holds no such
/// item.
async fn set_link(
    source: &ResolvedSource,
    level: Level,
    id: &NativeId,
    key: &MetadataKey,
    value: &Value,
) -> Result<bool, SourceError> {
    Ok(set_link_versioned(source, level, id, key, value)
        .await?
        .is_some())
}

/// [`set_link`], answering — when the source holds the item — the change token it answered the
/// write with, `None` inside for a source that gives none.
async fn set_link_versioned(
    source: &ResolvedSource,
    level: Level,
    id: &NativeId,
    key: &MetadataKey,
    value: &Value,
) -> Result<Option<Option<String>>, SourceError> {
    Ok(match level {
        Level::Task => source
            .source()
            .set_task_metadata(id, key, value)
            .await?
            .map(|task| task.version),
        Level::Project => source
            .source()
            .set_project_metadata(id, key, value)
            .await?
            .map(|project| project.version),
        Level::Document => source
            .source()
            .set_document_metadata(id, key, value)
            .await?
            .map(|document| document.version),
    })
}

/// The metadata of either kind of item, to edit.
fn metadata_of(item: &mut Item) -> &mut BTreeMap<String, Value> {
    match item {
        Item::Task(task) => &mut task.metadata,
        Item::Project(project) => &mut project.metadata,
        Item::Document(document) => &mut document.metadata,
    }
}

/// Refuse a document copy addressed to a source that declares it has none.
///
/// Read off the declaration rather than by asking, which is what "not asked" means: the
/// engine learned at the handshake that this source holds no documents, so it refuses
/// naming the source and its plugin instead of sending a read that would be refused there.
/// Applied at both ends of a copy — a source with no documents holds nothing to copy out,
/// and a destination with none has nowhere to put one.
fn documentary(source: &ResolvedSource) -> Result<(), EngineError> {
    if source.source().capabilities().documents.is_native() {
        return Ok(());
    }
    Err(EngineError::NoDocuments {
        name: source.name().to_string(),
        kind: source.kind().to_owned(),
    })
}

/// One source failing while a copy was mid-flight.
fn refused(source: &ResolvedSource, error: SourceError) -> EngineError {
    EngineError::SourceRefused {
        name: source.name().to_string(),
        error,
    }
}

/// The plugin method a create is sent as — the operation an uncertain destination names.
#[derive(Clone, Copy)]
enum Creating {
    Task,
    /// Several tasks created together, by one `write_tasks`.
    Tasks,
    Document,
    Project,
}

impl Creating {
    /// The method's name in `docs/plugin-protocol.md`, which an asset write shares with the
    /// item it is on.
    fn method(self) -> &'static str {
        match self {
            Self::Task => "write_task",
            Self::Tasks => "write_tasks",
            Self::Document => "write_document",
            Self::Project => "write_project",
        }
    }
}

/// A destination's refusal of a create sent as `operation`, where a malformed answer is not a
/// refusal at all: the plugin may have stored the item before answering in a shape the engine
/// refuses, so the destination may hold an item this copy has no id to take back.
fn create_refused(
    destination: &ResolvedSource,
    operation: Creating,
    error: SourceError,
) -> EngineError {
    if matches!(error, SourceError::Malformed { .. }) {
        EngineError::CopyDestinationUncertain {
            error: Box::new(refused(destination, error)),
            destination: destination.name().to_string(),
            operation: operation.method().to_owned(),
        }
    } else {
        refused(destination, error)
    }
}

/// Every forward edge at one item, walked to exhaustion one page at a time.
pub(super) async fn forward_edges(
    source: &ResolvedSource,
    id: &NativeId,
    kind: Level,
) -> Result<Vec<DependencyEdge>, EngineError> {
    // A document has no edges to walk, and asking for them would mean asking a source for
    // a graph the contract says nothing may point into.
    if kind == Level::Document {
        return Ok(Vec::new());
    }
    let mut edges = Vec::new();
    let mut cursor: Option<Cursor> = None;
    loop {
        let asked = cursor.clone();
        let request = request_for(source, cursor);
        let page = match kind {
            Level::Task | Level::Document => {
                source
                    .source()
                    .task_dependencies(id, Direction::DependsOn, &request)
                    .await
            }
            Level::Project => {
                source
                    .source()
                    .project_dependencies(id, Direction::DependsOn, &request)
                    .await
            }
        }
        .map_err(|error| refused(source, error))?;
        fits(page.items.len(), request.limit).map_err(|error| refused(source, error))?;
        edges.extend(page.items);
        unrepeated(
            page.next.as_ref(),
            asked.as_ref(),
            "an item's dependencies were being read for a copy",
        )
        .map_err(|error| refused(source, error))?;
        match page.next {
            Some(next) => cursor = Some(next),
            None => return Ok(edges),
        }
    }
}

/// The location string one record reports, when it reports a usable one.
///
/// Either variant's own `String`, and `None` for a record the source gave no location for
/// or gave an empty string for: there is nothing to look for in a document's content and
/// nothing to point a reader at.
fn located(location: Option<&Location>) -> Option<String> {
    let (Location::Path(held) | Location::Url(held)) = location?;
    (!held.is_empty()).then(|| held.clone())
}

/// Whether a destination record a referent's link names still names it back: its own
/// origin is one of the referent's two keys. A record somebody re-pointed is no counterpart
/// by its link, and the referent is looked for by its origin instead.
fn confirms(referent: &Referent, metadata: &BTreeMap<String, Value>) -> bool {
    origin_of(metadata).is_some_and(|origin| referent.keys().contains(&origin.to_string()))
}

/// What a counterpart lookup keeps of each record it is answered with: its id, the location
/// its source reports and its metadata.
type HeldRecord = (NativeId, Option<Location>, BTreeMap<String, Value>);

/// Record one candidate referent, when its source said where it is.
fn note(
    into: &mut Vec<Referent>,
    source: &ResolvedSource,
    id: GlobalId,
    level: Level,
    location: Option<&Location>,
    metadata: &BTreeMap<String, Value>,
    content: Option<&str>,
) {
    if let Some(location) = located(location) {
        let links = counterparts_linked(metadata.get(MetadataKey::COPIES_KEY));
        into.push(Referent {
            stored: source.source().stored_headings(level.record(), &id.native),
            id,
            origin: origin_of(metadata),
            level,
            location,
            located: locations_linked(metadata, &links),
            links,
            content: content.map(str::to_owned),
        });
    }
}

/// Whether `location` occurs at `at` **stopped on both sides** — by
/// [`stops_a_location`], or by the end of the content — rather than as part of a longer
/// location-like string.
///
/// A location string occurring inside a longer one is a different string naming a
/// different record: `/…/tasks/p/t.md` must not be rewritten inside `/…/tasks/p/t.md.bak`,
/// `https://example.invalid/1` must not be rewritten inside `https://example.invalid/12`,
/// and a project's location that is a directory prefix of a task's must not be rewritten
/// inside that task's. What deciding it this way costs is stated on [`stops_a_location`].
///
/// A `#` straight after the location also stops it, and only on that side: it opens a
/// fragment — `[budgets](/…/t.md#budgets)`, `https://…/issues/1#budgets` — which names a
/// place *inside* the record rather than another record. The location before it is
/// rewritten like any other, and the fragment, never part of the match, is carried through
/// [`substitute`]'s walk byte-for-byte. Read this way, a file whose own name holds a `#`
/// beside a referent's — `t.md#2` beside `t.md` — would be taken for that referent with a
/// fragment; a *referent* so named is still matched whole, because the table is tried
/// longest first.
fn delimited_at(content: &str, at: usize, location: &str) -> bool {
    if !content.is_char_boundary(at) || !content[at..].starts_with(location) {
        return false;
    }
    let before = content[..at].chars().next_back();
    let after = content[at + location.len()..].chars().next();
    stops_a_location(before) && (stops_a_location(after) || after == Some(FRAGMENT))
}

/// What opens a fragment after a location: see [`delimited_at`].
const FRAGMENT: char = '#';

/// Whether a character cannot continue a path or a link, so a location string beside one
/// ends there.
///
/// Stated as what *stops* a location rather than as what one may contain, because the
/// second list is unbounded — a path may hold very nearly any byte, and a URL more. Every
/// character not named here continues, which is what leaves the three cases above alone;
/// the end of the content counts as a stop. The set is what the artifact this exists for
/// really wraps a bare path in — a backtick in a table cell — plus the delimiters prose
/// and Markdown put next to one.
///
/// **Sentence punctuation is deliberately absent, and that is a stated cost rather than an
/// oversight.** `.`, `!`, `?` and `:` each equally *continue* a real location — `/…/t.md`
/// and `/…/t.md.bak` are two files, `…/1` and `…/1?q=2` two pages — so admitting them as
/// stops would rewrite one record's location into another's. The price is that a location
/// written bare at the end of a sentence is not recognised at all: its text is left
/// byte-for-byte and it is counted in neither figure, exactly as a reference to another
/// project's record is. That is the direction to be wrong in, because this edits the
/// content of somebody's document, where a confidently wrong rewrite is worse than one
/// that never happens.
fn stops_a_location(character: Option<char>) -> bool {
    match character {
        None => true,
        Some(character) => character.is_whitespace() || "`\"'()[]{}<>|,;".contains(character),
    }
}

/// The origin one item records, when it records a usable one.
fn origin_of(metadata: &BTreeMap<String, Value>) -> Option<GlobalId> {
    metadata
        .get(GlobalId::ORIGIN_KEY)?
        .as_str()?
        .parse::<GlobalId>()
        .ok()
}

/// The destination item one item's `onetaskgraph.copies` entry names at `destination`, when
/// it holds a usable one.
///
/// An entry that is not a qualified id, or is one naming another source than the key it is
/// filed under, is no link at all: the copy goes on by the rules that follow and rewrites
/// it to what they find.
fn link_of(metadata: &BTreeMap<String, Value>, destination: &SourceName) -> Option<GlobalId> {
    let linked = metadata
        .get(MetadataKey::COPIES_KEY)?
        .as_object()?
        .get(destination.as_str())?
        .as_str()?
        .parse::<GlobalId>()
        .ok()?;
    (&linked.source == destination).then_some(linked)
}

/// The entries of a `onetaskgraph.copies` value that are links: a source name, and a
/// qualified id of that same source.
fn well_formed(value: Option<&Value>) -> serde_json::Map<String, Value> {
    let Some(Value::Object(held)) = value else {
        return serde_json::Map::new();
    };
    held.iter()
        .filter(|(destination, linked)| {
            linked
                .as_str()
                .and_then(|linked| linked.parse::<GlobalId>().ok())
                .is_some_and(|linked| linked.source.as_str() == destination.as_str())
        })
        .map(|(destination, linked)| (destination.clone(), linked.clone()))
        .collect()
}

/// The entries of a `onetaskgraph.copy_locations` value that say something: a source name, and
/// an object of the qualified id of that source it is about and a [`Location`].
fn located_entries(value: Option<&Value>) -> serde_json::Map<String, Value> {
    let Some(Value::Object(held)) = value else {
        return serde_json::Map::new();
    };
    held.iter()
        .filter(|(destination, entry)| location_entry(destination, entry).is_some())
        .map(|(destination, entry)| (destination.clone(), entry.clone()))
        .collect()
}

/// The id and the location one `onetaskgraph.copy_locations` entry filed under `destination`
/// records, when it is one: `{"id": <a qualified id of that source>, "location": <Location>}`.
fn location_entry(destination: &str, entry: &Value) -> Option<(GlobalId, Location)> {
    let id = entry.get("id")?.as_str()?.parse::<GlobalId>().ok()?;
    if id.source.as_str() != destination {
        return None;
    }
    let location = serde_json::from_value::<Location>(entry.get("location")?.clone()).ok()?;
    Some((id, location))
}

/// The location each `onetaskgraph.copy_locations` entry records for the counterpart its
/// `onetaskgraph.copies` link still names, by the destination: an entry whose id is not what
/// the link names there — the link moved since, or there is none — says nothing.
fn locations_linked(
    metadata: &BTreeMap<String, Value>,
    links: &BTreeMap<SourceName, NativeId>,
) -> BTreeMap<SourceName, Location> {
    located_entries(metadata.get(MetadataKey::COPY_LOCATIONS_KEY))
        .iter()
        .filter_map(|(destination, entry)| location_entry(destination, entry))
        .filter(|(id, _)| links.get(&id.source) == Some(&id.native))
        .map(|(id, location)| (id.source, location))
        .collect()
}

/// Why a value is not one `onetaskgraph.copy_locations` may hold, or nothing when it is: a
/// JSON object mapping each destination source name to the id of that source it is about and
/// a location.
///
/// What the stdio boundary holds an incoming value of that key to, as it holds a link.
pub(crate) fn malformed_locations(value: &Value) -> Option<String> {
    let Value::Object(held) = value else {
        return Some(format!(
            "{} holds an object of destination source names to an id and a location, not {value}",
            MetadataKey::COPY_LOCATIONS_KEY
        ));
    };
    let wrong = held.len() - located_entries(Some(value)).len();
    (wrong > 0).then(|| {
        format!(
            "{} holds an object of destination source names to {{\"id\": <a qualified id of that \
             source>, \"location\": <a location>}}, and {wrong} of its entries is not one: {value}",
            MetadataKey::COPY_LOCATIONS_KEY
        )
    })
}

/// The counterpart each well-formed entry of a `onetaskgraph.copies` value names, by the
/// destination it names it at.
fn counterparts_linked(value: Option<&Value>) -> BTreeMap<SourceName, NativeId> {
    well_formed(value)
        .values()
        .filter_map(|linked| linked.as_str()?.parse::<GlobalId>().ok())
        .map(|linked| (linked.source, linked.native))
        .collect()
}

/// Why a value is not one a `onetaskgraph.copies` entry may hold, or nothing when it is: a
/// JSON object mapping each destination source name to a qualified id of that source.
///
/// What the stdio boundary holds an incoming value of that key to, so a plugin is never
/// handed a link it could not answer a later copy with.
pub(crate) fn malformed_links(value: &Value) -> Option<String> {
    let Value::Object(held) = value else {
        return Some(format!(
            "{} holds an object of destination source names to qualified ids, not {value}",
            MetadataKey::COPIES_KEY
        ));
    };
    let wrong = held.len() - well_formed(Some(value)).len();
    (wrong > 0).then(|| {
        format!(
            "{} holds an object of destination source names to qualified ids of that source, \
             and {wrong} of its entries is not one: {value}",
            MetadataKey::COPIES_KEY
        )
    })
}

/// Where `item` landed at `id` in `destination`, when the copy knows it without asking: the
/// location its counterpart was read with, when that read was of the item at `id`, or else
/// what the destination's own write answered.
fn landed_location(
    destination: &ResolvedSource,
    item: &Planned,
    id: &NativeId,
) -> Option<Location> {
    if let Some(held) = &item.held {
        let (held_id, location) = match &held.item {
            Item::Task(task) => (&task.id, &task.location),
            Item::Project(project) => (&project.id, &project.location),
            Item::Document(document) => (&document.id, &document.location),
        };
        if held_id == id {
            return location.clone();
        }
    }
    let record = match item.item {
        Item::Task(_) => MetadataRecord::Task,
        Item::Project(_) => MetadataRecord::Project,
        Item::Document(_) => MetadataRecord::Document,
    };
    destination.source().written_location(record, id)
}

/// A status category as the command line spells it, for a refusal's message.
pub(super) fn spelled_category(category: StatusCategory) -> String {
    query::spelled(category)
}

/// The title and metadata of either kind of item.
fn described(item: &Item) -> (&str, &BTreeMap<String, Value>) {
    match item {
        Item::Task(task) => (&task.title, &task.metadata),
        Item::Project(project) => (&project.title, &project.metadata),
        Item::Document(document) => (&document.title, &document.metadata),
    }
}

/// What `item` holds, at the version it carries: a task's status and priority, a project's
/// status, and a document's neither.
fn item_result(item: &Item) -> WriteResult {
    match item {
        Item::Task(task) => WriteResult::of_task(task, task.version.clone()),
        Item::Project(project) => WriteResult {
            version: project.version.clone(),
            parts: None,
            status: Some(project.status.clone()),
            priority: None,
        },
        Item::Document(document) => WriteResult {
            version: document.version.clone(),
            parts: None,
            status: None,
            priority: None,
        },
    }
}

/// `outcome` reporting `result` as what its item holds, beside what an expected write's own
/// result already says of its parts.
fn with_result(mut outcome: CopyOutcome, result: WriteResult) -> CopyOutcome {
    outcome.expected.result = Some(match outcome.expected.result.take() {
        Some(expected) => expected.merged(result),
        None => result,
    });
    outcome
}

/// Decide each task's conditions on the counterpart it updates from the copy's own read of
/// that counterpart, before anything is written: record what each found, turn an item whose
/// status is unmet under `--otherwise preserve-destination` into one that keeps its status, and
/// answer every item an unmet status refuses, by its index in `planned`.
///
/// No read is made here: the counterpart is what [`Planned::held`] already holds, read where
/// the target was found — from the cache under `--expect`. A create, and an item whose
/// counterpart was not read, are never conditioned.
fn decide_conditions(
    request: &CopyRequest,
    planned: &mut [Planned],
) -> HashMap<usize, EngineError> {
    let conditions = &request.conditions;
    let mut refused = HashMap::new();
    if conditions.status.is_empty()
        && conditions.priority.is_empty()
        && conditions.versions.is_empty()
    {
        return refused;
    }
    for (index, item) in planned.iter_mut().enumerate() {
        let (Target::Update { id, .. }, Some(Item::Task(held))) =
            (&item.target, item.held.as_ref().map(|held| &held.item))
        else {
            continue;
        };
        if !matches!(item.item, Item::Task(_)) {
            continue;
        }
        let id = id.clone();
        let held_category = held.status.category;
        let held_priority = held.priority;
        // A held version is decided first: an item it refuses is written nothing at all, so
        // no other condition of it is decided.
        if let Some(version) = conditions.versions.get(&item.source)
            && held.version.as_deref() != Some(version.as_str())
        {
            let found = held.version.clone().unwrap_or_default();
            refused.insert(
                index,
                EngineError::VersionMoved {
                    id: GlobalId::new(item.to.clone(), id).to_string(),
                    expected: version.clone(),
                    held: Box::new(WriteResult {
                        version: Some(found.clone()),
                        parts: None,
                        status: None,
                        priority: None,
                    }),
                    found,
                },
            );
            continue;
        }
        if !conditions.priority.is_empty() {
            item.priority_condition = Some(PriorityCondition {
                held: held_priority,
                met: conditions.priority.contains(&held_priority),
            });
        }
        if conditions.status.is_empty() {
            continue;
        }
        let met = conditions.status.contains(&held_category);
        item.status_condition = Some(StatusCondition {
            held: held_category,
            met,
        });
        if met {
            continue;
        }
        match conditions.otherwise.unwrap_or_default() {
            Otherwise::PreserveDestination => {
                item.status_policy = StatusPolicy::PreserveDestination;
            }
            Otherwise::Refuse => {
                refused.insert(
                    index,
                    EngineError::DestinationStatus {
                        item: item.source.clone(),
                        destination: GlobalId::new(item.to.clone(), id),
                        held: held_category,
                        allowed: conditions.status.clone(),
                    },
                );
            }
        }
    }
    refused
}

/// The item as the destination should hold it.
///
/// `url`, `location`, `created_at` and `updated_at` are the destination's own and are
/// never written — where the *source* holds an item says nothing about where the
/// destination does, which is why a copied document does not arrive claiming the path or
/// the link its source reported. The two reserved keys this product encodes typed fields
/// under are removed, because those fields travel as themselves — leaving the encoding
/// beside them would have the destination hold one thing twice, and disagree with itself
/// the moment one changed.
///
/// A task's `delivers` is `delivers`, already resolved against the destination, and its
/// `delivered_by` is the one the destination holds — never the source's, and empty for an
/// item this copy creates: that list is the store's to keep, at the destination. The
/// `onetaskgraph.copies` link is kept on the same terms: it says where the *destination*
/// item was copied to, which is the destination's to hold, never the source's to send.
fn outgoing(
    item: &Planned,
    id: NativeId,
    project: Option<NativeId>,
    origin: &Origin,
    delivers: &[TaskRef],
    held: Option<&Prior>,
) -> Item {
    let own = held.map(|held| described(&held.item).1);
    let carried = |metadata: &BTreeMap<String, Value>| carried(metadata, origin, own);
    // Never looser than the destination already holds it: a project once private stays
    // private after the members that made it so are gone, and so does anything else.
    let classification = item
        .classification
        .strictest(held.map_or(Classification::Public, |held| classification_of(&held.item)));
    match &item.item {
        Item::Task(task) => Item::Task(Box::new(Task {
            id,
            url: None,
            location: None,
            created_at: None,
            updated_at: None,
            project,
            delivers: delivers.to_vec(),
            delivered_by: match held.map(|held| &held.item) {
                Some(Item::Task(held)) => held.delivered_by.clone(),
                _ => Vec::new(),
            },
            status: match (item.status_policy, held.map(|held| &held.item)) {
                (StatusPolicy::PreserveDestination, Some(Item::Task(held))) => held.status.clone(),
                _ => task.status.clone(),
            },
            priority: match held.map(|held| &held.item) {
                Some(Item::Task(held)) if item.keeps_priority() => held.priority,
                _ => task.priority,
            },
            assignees: item.assignees.clone(),
            metadata: {
                let mut metadata = carried(&task.metadata);
                created_with(&mut metadata, item, held);
                item.rewrite.metadata_values(&mut metadata);
                metadata
            },
            classification,
            ..(**task).clone()
        })),
        Item::Project(project) => {
            let mut metadata = carried(&project.metadata);
            // A member taken over as this project's copy is a member no longer.
            if matches!(item.target.found(), Some(Found::Member { .. })) {
                metadata.remove(MetadataKey::MEMBER_OF_KEY);
            }
            created_with(&mut metadata, item, held);
            item.rewrite.metadata_values(&mut metadata);
            Item::Project(Box::new(Project {
                id,
                url: None,
                location: None,
                created_at: None,
                updated_at: None,
                metadata,
                status: match (item.status_policy, held.map(|held| &held.item)) {
                    (StatusPolicy::PreserveDestination, Some(Item::Project(held))) => {
                        held.status.clone()
                    }
                    _ => project.status.clone(),
                },
                classification,
                ..(**project).clone()
            }))
        }
        Item::Document(document) => Item::Document(Box::new(Document {
            id,
            url: None,
            location: None,
            created_at: None,
            updated_at: None,
            project,
            metadata: {
                let mut metadata = carried(&document.metadata);
                created_with(&mut metadata, item, held);
                item.rewrite.metadata_values(&mut metadata);
                metadata
            },
            classification,
            ..(**document).clone()
        })),
    }
}

/// A create carries its key, written with the create itself.
fn created_with(metadata: &mut BTreeMap<String, Value>, item: &Planned, held: Option<&Prior>) {
    if let (Some(intent), None, Target::Create) = (&item.intent, held, &item.target) {
        metadata.insert(
            MetadataKey::CREATE_KEY.to_owned(),
            Value::String(intent.key()),
        );
    }
}

/// Who may read `item`, as its own record says.
fn classification_of(item: &Item) -> Classification {
    match item {
        Item::Task(task) => task.classification,
        Item::Project(project) => project.classification,
        Item::Document(document) => document.classification,
    }
}

fn repositories_of(item: &Item) -> &[Repository] {
    match item {
        Item::Task(task) => &task.repositories,
        Item::Project(project) => &project.repositories,
        Item::Document(document) => &document.repositories,
    }
}

/// What one planned item would put in front of a reader where it lands: everything it carries
/// once its provenance is settled, and the names of its assets.
///
/// An id the write withholds is not part of what it exposes: `withheld` says which, by the far
/// end's own id as the item's source names it.
fn exposure_of(item: &Planned, withheld: impl Fn(&GlobalId) -> bool) -> Exposure {
    let origin = &item.source.source;
    let shown = |id: &str| id.parse::<GlobalId>().map_or(true, |far| !withheld(&far));
    let delivers = match &item.item {
        Item::Task(task) => task
            .delivers
            .iter()
            .filter(|entry| shown(entry.in_source(origin).as_str()))
            .cloned()
            .collect(),
        Item::Project(_) | Item::Document(_) => Vec::new(),
    };
    let preview = outgoing(
        item,
        item.item.id().clone(),
        None,
        &recorded(item, item.held.as_ref()),
        &delivers,
        item.held.as_ref(),
    );
    match &preview {
        Item::Task(task) => Exposure::of_task(task),
        Item::Project(project) => Exposure::of_project(project),
        Item::Document(document) => Exposure::of_document(document),
    }
    .assets(&item.assets)
    // The id a create offers the destination is made up from the source's, and a destination
    // whose ids are paths writes it where a reader sees it, so it is screened as the title is.
    .and(match item.target {
        Target::Create => Exposure::short([item.item.id().as_str().to_owned()]),
        Target::Update { .. } => Exposure::default(),
    })
    // The source spelling its links are rendered from at each write, so the rendering is
    // rewritten exactly as the content it becomes.
    .and(item.links.as_ref().map_or_else(Exposure::default, |links| {
        Exposure::text(links.original.clone())
    }))
    // So is what a link not addressable yet is rendered from once it is: a placeholder stands in
    // for it above, and its fragment lands as written.
    .and(
        item.pending
            .iter()
            .chain(
                item.comment_links
                    .iter()
                    .flat_map(|(_, _, pending)| pending),
            )
            .fold(Exposure::metadata(Vec::new()), |exposure, pending| {
                exposure.and(Exposure::text(pending.source_spelling().to_owned()))
            }),
    )
    .and(
        item.comment_links
            .iter()
            .fold(Exposure::metadata(Vec::new()), |exposure, (_, links, _)| {
                exposure.and(Exposure::text(links.original.clone()))
            }),
    )
    // The comments a `--comments` copy carries land beside the item, so they are held to the
    // boundary with it — every one of them, before any is written.
    .and(
        item.carried
            .iter()
            .flatten()
            .fold(Exposure::metadata(Vec::new()), |exposure, comment| {
                exposure
                    .and(Exposure::text(comment.body.clone()))
                    .and(Exposure::metadata(comment.author.iter().cloned()))
            }),
    )
    .and(Exposure::metadata(
        item.edges
            .iter()
            .filter(|edge| far_end(&edge.to, origin).is_none_or(|far| !withheld(&far)))
            .map(|edge| edge.to.id().to_owned()),
    ))
}

/// `item` as the sanitisation that admitted its copy passed it: its own rewritable fields, the
/// source spelling its links are rendered from, and the body of every comment it carries.
///
/// # Errors
///
/// [`EngineError::BoundaryRefused`] when a replacement left a carried comment empty, which no
/// destination takes as a comment: the copy is refused before anything is written.
fn rewritten(item: &mut Planned, rewrite: &Rewrite) -> Result<(), EngineError> {
    if rewrite.is_identity() {
        return Ok(());
    }
    item.rewrite = rewrite.clone();
    let creates = matches!(item.target, Target::Create);
    match &mut item.item {
        Item::Task(task) => {
            rewrite.task(task);
            if creates {
                task.id = NativeId::from(rewrite.short(task.id.as_str()).as_str());
            }
        }
        Item::Project(project) => {
            rewrite.project(project);
            if creates {
                project.id = NativeId::from(rewrite.short(project.id.as_str()).as_str());
            }
        }
        Item::Document(document) => {
            rewrite.document(document);
            if creates {
                document.id = NativeId::from(rewrite.short(document.id.as_str()).as_str());
            }
        }
    }
    if let Some(links) = &mut item.links {
        links.original = rewrite.text(&links.original);
    }
    for (_, links, _) in &mut item.comment_links {
        links.original = rewrite.text(&links.original);
    }
    for pending in item.pending.iter_mut().chain(
        item.comment_links
            .iter_mut()
            .flat_map(|(_, _, pending)| pending),
    ) {
        pending.reword(|spelling| rewrite.text(spelling));
    }
    for comment in item.carried.iter_mut().flatten() {
        comment.body = rewrite.text(&comment.body);
    }
    let emptied = || EngineError::BoundaryRefused {
        item: item.source.to_string(),
        destination: item.to.to_string(),
        reason: "a replacement leaves a comment it carries empty".to_owned(),
    };
    let mut refused = None;
    for step in item
        .comment_plan
        .iter_mut()
        .flat_map(|plan| plan.steps.iter_mut())
    {
        match step {
            comments::Step::Add(copy) | comments::Step::Edit { copy, .. } => {
                match onetaskgraph_plugin_api::CommentBody::new(rewrite.text(copy.body.as_str())) {
                    Ok(body) => copy.body = body,
                    Err(_) => refused = Some(()),
                }
            }
            comments::Step::Delete(_) => {}
        }
    }
    match refused {
        Some(()) => Err(emptied()),
        None => Ok(()),
    }
}

/// The id a created item is offered to the destination under.
///
/// A task whose id is `<its project's id>/<rest>` is offered as `<rest>` under the
/// destination project it is filed in, so a destination whose ids are paths files it in that
/// project rather than the source's. Any other id, and every update, is left as it is. The
/// `a_project_copied_*` journeys in `crates/onetaskgraph-e2e/tests/e2e/copy.rs` hold the Markdown
/// plugin's id shape to this rule.
fn created_id(item: &Item, filed: Option<&NativeId>) -> NativeId {
    if let (Item::Task(task), Some(filed)) = (item, filed)
        && let Some(own) = &task.project
        && let Some(rest) = task
            .id
            .as_str()
            .strip_prefix(own.as_str())
            .and_then(|rest| rest.strip_prefix('/'))
        && !rest.is_empty()
    {
        return NativeId(format!("{}/{rest}", filed.as_str()));
    }
    item.id().clone()
}

/// The metadata a copy carries: the caller's own keys untouched, the origin settled, and the
/// destination's own store-kept keys — its `onetaskgraph.copies` link, and the
/// `onetaskgraph.members` or `onetaskgraph.member_of` that ties it to its member projects or
/// its home — kept as `own`, the destination item, holds them.
///
/// An update starts from what `own` holds and lays the source's keys over it, so a key only
/// the destination holds — an approval recorded on the copy, say — survives the copy and a
/// key both hold takes the source's value: a copy never deletes a key the destination holds.
/// `onetaskgraph.template` and `onetaskgraph.item_kind` are the exception, because they
/// describe the item's content and kind rather than annotate it: they are the source's, and
/// where the source has none the destination's is dropped rather than left stale. A create
/// has no `own`, and carries exactly the source's keys.
///
/// The key is removed before it is settled rather than overwritten, because the item being
/// copied carries an origin of its own and [`Origin::Keeps`] must not let it through. Its
/// link is removed for the same reason: it names where *that* item was copied to, and on
/// the destination it would name the destination item's own counterpart as itself. Its
/// member keys name projects tied to *it*, which say nothing about the destination's, and its
/// `onetaskgraph.links` name links whose endpoint is *it*.
fn carried(
    metadata: &BTreeMap<String, Value>,
    origin: &Origin,
    own: Option<&BTreeMap<String, Value>>,
) -> BTreeMap<String, Value> {
    let mut carried = own.cloned().unwrap_or_default();
    carried.remove(MetadataKey::TEMPLATE_KEY);
    carried.remove(ItemKind::METADATA_KEY);
    carried.extend(metadata.clone());
    carried.remove(Repository::METADATA_KEY);
    carried.remove(DependencyEdge::RECORDED_KEY);
    carried.remove(TaskRef::DELIVERS_KEY);
    carried.remove(TaskRef::DELIVERED_BY_KEY);
    carried.remove(GlobalId::ORIGIN_KEY);
    // An intent to create is the copied item's own, never its counterpart's.
    carried.remove(MetadataKey::CREATING_KEY);
    for kept in [
        MetadataKey::CREATE_KEY,
        MetadataKey::COPIES_KEY,
        MetadataKey::COPY_LOCATIONS_KEY,
        MetadataKey::MEMBERS_KEY,
        MetadataKey::MEMBER_OF_KEY,
        MetadataKey::ASSETS_KEY,
        MetadataKey::EDGES_ON_RECORD_KEY,
        dependents::DEPENDED_ON_BY_KEY,
        PublicProjections::KEY,
        // A link names its endpoints; the copy is neither, so it never carries the source's
        // and keeps whatever links the destination item holds of its own.
        onetaskgraph_plugin_api::TaskLink::METADATA_KEY,
    ]
    .into_iter()
    .chain(record::KEYS)
    {
        carried.remove(kept);
        if let Some(held) = own.and_then(|own| own.get(kept)) {
            carried.insert(kept.to_owned(), held.clone());
        }
    }
    // The source's lineage is settled with its origin: a copy carries the root the source
    // leads back to, and a copy that keeps the destination's origin keeps its lineage too.
    carried.remove(GlobalId::LINEAGE_KEY);
    let (held, lineage) = match origin {
        Origin::Records(id, lineage) => (Some(Value::String(id.to_string())), lineage.clone()),
        Origin::Keeps(held) => (
            held.clone(),
            own.and_then(|own| own.get(GlobalId::LINEAGE_KEY)).cloned(),
        ),
    };
    if let Some(held) = held {
        carried.insert(GlobalId::ORIGIN_KEY.to_owned(), held);
    }
    if let Some(lineage) = lineage {
        carried.insert(GlobalId::LINEAGE_KEY.to_owned(), lineage);
    }
    carried
}

/// The root of the chain of copies `item` is one of, as `item` records it: its
/// [`GlobalId::LINEAGE_KEY`], else its [`GlobalId::ORIGIN_KEY`] — `None` when it records
/// neither, where `item` is its own root.
///
/// Taken as the item holds it, a value that is no qualified id included: a copy is not where
/// provenance is judged, and carrying it unread keeps it exactly as refusable where it lands —
/// by an attestation naming the item as a far end — as it is where it was recorded, rather than
/// putting another root in its place.
fn lineage_of(item: &Item) -> Option<Value> {
    let metadata = described(item).1;
    [GlobalId::LINEAGE_KEY, GlobalId::ORIGIN_KEY]
        .into_iter()
        .find_map(|key| metadata.get(key).cloned())
}

/// What one landed item records at [`GlobalId::ORIGIN_KEY`].
enum Origin {
    /// The qualified id this item was copied from, as the id type rather than as its
    /// spelling: the key holds a [`GlobalId`] and nothing else may be recorded there — and
    /// the root of its chain of copies, where that is not this id, as its source holds it
    /// ([`Planned::lineage`](Planned)).
    Records(GlobalId, Option<Value>),
    /// Whatever the destination already holds there — `None` when it holds nothing, which
    /// is written as the key being absent rather than as a null.
    ///
    /// A [`Value`] and not a [`GlobalId`], because this variant does not interpret what it
    /// carries: it is the destination's own metadata entry, held for the length of one
    /// write and put back exactly as it was read. Parsing it would turn a value a
    /// destination holds and this engine cannot read into a value this engine deletes,
    /// which is the opposite of what keeping it means.
    Keeps(Option<Value>),
}

/// Which of the two a copy of this item does.
///
/// A copy that reached its target by rule 1 is a copy-back: the item being copied names
/// the destination item, so the destination is the *original* and the id being copied
/// belongs to the copy that came out of it. Recording that id there would overwrite the
/// original's own provenance — and with it the correspondence every later copy from the
/// source it was authored in depends on. That copy would then match nothing and create a
/// second item beside the one it meant to update, which is the whole failure: nothing is
/// reported, and whoever reads that board now has two. So a copy-back leaves the
/// destination's origin exactly as the destination holds it, absent included, and every
/// other copy records the id it was copied from.
///
/// A copy out of a private source into somewhere not declared private records no origin at
/// all: the id it would record names the private source, and provenance is not a way for it
/// to leave. The correspondence is kept on the private side instead, as the link the copied
/// item records.
fn recorded(item: &Planned, held: Option<&Prior>) -> Origin {
    if let Target::Update {
        found: Found::Origin,
        ..
    } = &item.target
    {
        return Origin::Keeps(
            held.and_then(|held| described(&held.item).1.get(GlobalId::ORIGIN_KEY).cloned()),
        );
    }
    if item.withheld {
        return Origin::Keeps(None);
    }
    Origin::Records(item.source.clone(), item.lineage.clone())
}

/// Whether a copy reads each task's comments from its source: to carry them, or to confirm the
/// comment activity of the query that names its items.
fn reads_comments(request: &CopyRequest) -> bool {
    request.comments
        || request
            .items
            .query()
            .is_some_and(|query| query.commented_since.is_some())
}

impl Engine {
    /// Whether `item`'s source token vouches for every part a copy of it writes that
    /// [`recorded_unchanged`] does not compare for itself, on the terms of
    /// [`Self::token_covers_a_copy`] — a project's fields, a document's fields and assets.
    ///
    /// A project's forward edges are not among them: [`recorded_unchanged`] holds the edges this
    /// copy would write, read from the source now, to the ones the destination holds, so a token
    /// that does not cover them — a GitHub board's — still says the rest of the project is as the
    /// copy that recorded it left it. Asking it to cover them too left such a project never
    /// recognised as unchanged on a destination that rewrites the Markdown it is sent, and
    /// written again on every repeat copy.
    fn version_covers_a_copy(&self, item: &Planned) -> bool {
        match &item.item {
            Item::Task(_) => self.token_covers_a_copy(&item.source.source, item.carried.is_some()),
            Item::Project(_) => self.readable(&item.source.source).is_ok_and(|source| {
                source
                    .source()
                    .project_version_coverage()
                    .fields
                    .reason()
                    .is_none()
            }),
            Item::Document(_) => self.readable(&item.source.source).is_ok_and(|source| {
                let coverage = source.source().document_version_coverage();
                coverage.fields.reason().is_none() && coverage.assets.reason().is_none()
            }),
        }
    }

    /// Whether `source`'s token vouches for every part a copy writes of a task — its fields,
    /// its forward edges, and its comments when the copy carries them — so a copy may skip one
    /// whose token is unchanged: a part it does not cover can change with the token as it was.
    pub(super) fn token_covers_a_copy(&self, source: &SourceName, with_comments: bool) -> bool {
        self.readable(source).is_ok_and(|source| {
            let coverage = source.source().copy_version_coverage();
            coverage.fields.reason().is_none()
                && coverage.forward_edges.reason().is_none()
                && (!with_comments || coverage.comments.reason().is_none())
        })
    }
}

/// Whether a copy may skip `task`, whose counterpart the destination holds as `held`: its
/// source token is the one the counterpart records, and the counterpart digests now — over the
/// read the copy made of it to find it, comments included when the copy carries them — to the
/// digest the copy that left it so recorded.
fn skips(task: &Task, held: &Prior, with_comments: bool, destination: &SourceName) -> bool {
    let Item::Task(held_task) = &held.item else {
        return false;
    };
    let comments = if with_comments {
        match &held.comments {
            Some(held) => Some(comments::digest_entries(held)),
            None => return false,
        }
    } else {
        None
    };
    let now = record::digests_of(&held.item, &held.edges, comments, destination);
    record::skips(
        task.version.as_deref(),
        with_comments,
        &held_task.metadata,
        &now,
    )
}

/// What a `--comments` copy reports for the comments of `source`, a task it skipped: every
/// copy the destination holds of one of that task's comments, each already reading as its
/// origin.
///
/// A copy is that task's when its origin names the task, or `recorded` — the item the task was
/// itself copied from, whose comments it carried on. A copy another task's copy left there is
/// none of this task's, and is not counted, exactly as a copy that is not skipped counts it.
fn skipped_counts(
    held: Option<&Prior>,
    source: &GlobalId,
    recorded: Option<&GlobalId>,
) -> CommentCounts {
    let vouched = |task: &str| {
        task == source.to_string() || recorded.is_some_and(|recorded| task == recorded.to_string())
    };
    let copies = held
        .and_then(|held| held.comments.as_ref())
        .map_or(0, |held| {
            held.iter()
                .filter(|comment| {
                    comment
                        .origin
                        .as_ref()
                        .is_some_and(|origin| vouched(&origin.task))
                })
                .count()
        });
    CommentCounts {
        unchanged: u64::try_from(copies).unwrap_or(u64::MAX),
        ..CommentCounts::default()
    }
}

/// The record a copy of `item` leaves on its destination item, which lands as `landing` with
/// `edges`: what the source was — its token, or the digest of what this copy composed of it, its
/// content with every image the destination already serves pointed where `recorded` says — whether
/// the comments were carried, and the digest of what lands, the comments as the copy leaves them
/// included when it carries them.
fn record_of(
    item: &Planned,
    landing: &Item,
    edges: &[DependencyEdge],
    destination: &SourceName,
    recorded: Option<&AssetUploads>,
    comments: Option<Vec<Value>>,
) -> record::Record {
    let content = served_by(content_of(landing), &item.assets, recorded);
    record::Record {
        version: source_version(item, content.as_deref()),
        with_comments: item.carried.is_some(),
        digest: record::digest_of(landing, edges, comments, destination),
    }
}

/// What a copy records of `item`'s source at [`MetadataKey::COPIED_VERSION_KEY`]: its change
/// token where its source gives one, otherwise the digest of what this copy composes of it with
/// `content`.
fn source_version(item: &Planned, content: Option<&str>) -> record::SourceVersion {
    token_of(&item.item).map_or_else(
        || {
            record::SourceVersion::Composed(record::composed(
                &item.item,
                content,
                item.source_status.as_ref(),
            ))
        },
        |token| record::SourceVersion::Token(token.to_owned()),
    )
}

/// The change token a source gave `item` — a folder of Markdown gives a project and a document
/// none.
fn token_of(item: &Item) -> Option<&str> {
    match item {
        Item::Task(task) => task.version.as_deref(),
        Item::Project(project) => project.version.as_deref(),
        Item::Document(document) => document.version.as_deref(),
    }
}

/// The content of any kind of item, `None` for one holding none.
fn content_of(item: &Item) -> Option<&str> {
    match item {
        Item::Task(task) => task.content.as_deref(),
        Item::Project(project) => project.content.as_deref(),
        Item::Document(document) => document.content.as_deref(),
    }
}

fn set_content(item: &mut Item, content: String) {
    match item {
        Item::Task(task) => task.content = Some(content),
        Item::Project(project) => project.content = Some(content),
        Item::Document(document) => document.content = Some(content),
    }
}

fn metadata_ref(item: &Item) -> &BTreeMap<String, Value> {
    match item {
        Item::Task(task) => &task.metadata,
        Item::Project(project) => &project.metadata,
        Item::Document(document) => &document.metadata,
    }
}

fn location_of(item: &Item) -> Option<&Location> {
    match item {
        Item::Task(task) => task.location.as_ref(),
        Item::Project(project) => project.location.as_ref(),
        Item::Document(document) => document.location.as_ref(),
    }
}

/// The project whose records `item`'s content may name: its own project, and a project's
/// own self.
fn project_of(item: &Item) -> Option<NativeId> {
    match item {
        Item::Task(task) => task.project.clone(),
        Item::Project(project) => Some(project.id.clone()),
        Item::Document(document) => document.project.clone(),
    }
}

/// Whether `content` holds anything a reference or a section link could be — a Markdown
/// link, an address, or a path — so a body that holds none never has its project read. A
/// path is either separator's: a folder of Markdown on Windows reports `C:\…\t.md`, which
/// holds no `/`.
fn could_name(content: &str) -> bool {
    content.contains("](") || content.contains("://") || content.contains(['/', '\\'])
}

/// Whether `content` holds anything `reading` could take for a link.
fn could_be_read(content: &str, reading: sections::Reading) -> bool {
    could_name(content)
        || (reading == sections::Reading::BarePathsToo && privacy::could_address(content))
}

/// Read the record `id` of `destination` through `level`, for what its answer says its
/// headings are stored under: see [`TaskSource::stored_headings`].
async fn read_destination(
    destination: &ResolvedSource,
    level: Level,
    id: &NativeId,
) -> Result<(), EngineError> {
    let source = destination.source();
    match level {
        Level::Task => source.get_task(id).await.map(|_| ()),
        Level::Project => source.get_project(id).await.map(|_| ()),
        Level::Document => source.get_document(id).await.map(|_| ()),
    }
    .map_err(|error| refused(destination, error))
}

/// `planned` with every item a planned body links to — and, with `dependencies`, every item a
/// planned task depends on — ahead of it, and otherwise in the order it came in: an item is taken once every item of
/// `planned` it links to or depends on has been, and where they run in a cycle, the earliest
/// item left is taken as it stands. A task created after what it depends on lands with its
/// edges in its own write, rather than in a second write once its far end exists.
fn targets_first(planned: Vec<Planned>, dependencies: bool) -> Vec<Planned> {
    let links: Vec<Vec<usize>> = planned
        .iter()
        .map(|item| {
            let referents: Vec<&sections::Linked> = item
                .links
                .iter()
                .chain(item.comment_links.iter().map(|(_, links, _)| links))
                .flat_map(|links| &links.referents)
                .collect();
            planned
                .iter()
                .enumerate()
                .filter(|(_, other)| {
                    other.to == item.to
                        && !std::ptr::eq(*other, item)
                        && (referents.iter().any(|referent| {
                            referent.level == other.item.level() && referent.id == other.source
                        }) || (dependencies && depends_on(item, other)))
                })
                .map(|(index, _)| index)
                .collect()
        })
        .collect();
    if links.iter().all(Vec::is_empty) {
        return planned;
    }
    let mut taken = vec![false; planned.len()];
    let mut order = Vec::with_capacity(planned.len());
    while order.len() < planned.len() {
        let next = (0..planned.len())
            .find(|&index| !taken[index] && links[index].iter().all(|&target| taken[target]))
            .or_else(|| (0..planned.len()).find(|&index| !taken[index]))
            .expect("an item not yet taken");
        taken[next] = true;
        order.push(next);
    }
    let mut slots: Vec<Option<Planned>> = planned.into_iter().map(Some).collect();
    order
        .into_iter()
        .map(|index| slots[index].take().expect("each item taken once"))
        .collect()
}

/// Whether the planned `item`'s body links to the planned `other` — a link it can only finish
/// once `other` is written.
fn body_links_to(item: &Planned, other: &Planned) -> bool {
    !std::ptr::eq(item, other)
        && item
            .links
            .iter()
            .chain(item.comment_links.iter().map(|(_, links, _)| links))
            .flat_map(|links| &links.referents)
            .any(|referent| referent.level == other.item.level() && referent.id == other.source)
}

/// Whether the planned `item` depends on the planned `other`: one of its edges names `other`'s
/// source id, qualified or by the native id its own source names its own items by.
fn depends_on(item: &Planned, other: &Planned) -> bool {
    item.edges.iter().any(|edge| {
        let kind = match other.item.level() {
            Level::Task => ItemKind::Task,
            Level::Project => ItemKind::Project,
            Level::Document => return false,
        };
        edge.to.kind == kind
            && (edge.to.id() == other.source.to_string()
                || (other.source.source == item.source.source
                    && edge.to.id() == other.source.native.as_str()))
    })
}

/// One item a copy call writes: the source it lands in, its interface and its source id.
type WrittenKey = (SourceName, Level, GlobalId);

/// What a body's rendering knows while the copy is planned: where the item's own counterpart
/// is when it is updated, and the heading ids the destination's reads so far answered.
struct Planning<'a> {
    /// The destination.
    destination: &'a ResolvedSource,
    /// The item's own counterpart, when the copy updates one.
    own: Option<(NativeId, String)>,
    /// Every record whose heading ids were asked for and not known.
    missed: std::cell::RefCell<Vec<(Level, NativeId)>>,
}

impl sections::Knows for Planning<'_> {
    fn landed(&self, _referent: &sections::Linked) -> Option<(NativeId, String)> {
        None
    }

    fn stored(&self, level: Level, id: &NativeId) -> Option<Vec<StoredHeading>> {
        let stored = self
            .destination
            .source()
            .stored_headings(level.record(), id);
        if stored.is_none() {
            self.missed.borrow_mut().push((level, id.clone()));
        }
        stored
    }

    fn own(&self) -> Option<(NativeId, String)> {
        self.own.clone()
    }
}

/// What a body's rendering knows while the copy writes: every record this copy has landed so
/// far and where its destination says it is, and the heading ids its writes were answered
/// with.
struct Writing<'a> {
    /// The destination.
    destination: &'a ResolvedSource,
    /// What this copy has landed so far, by interface and source id.
    landed: &'a HashMap<(Level, GlobalId), NativeId>,
    /// The item's own counterpart, once known.
    own: Option<(NativeId, String)>,
}

impl sections::Knows for Writing<'_> {
    fn landed(&self, referent: &sections::Linked) -> Option<(NativeId, String)> {
        let id = self.landed.get(&(referent.level, referent.id.clone()))?;
        let location = located(
            self.destination
                .source()
                .written_location(referent.level.record(), id)
                .as_ref(),
        )?;
        Some((id.clone(), location))
    }

    fn stored(&self, level: Level, id: &NativeId) -> Option<Vec<StoredHeading>> {
        self.destination
            .source()
            .stored_headings(level.record(), id)
    }

    fn own(&self) -> Option<(NativeId, String)> {
        self.own.clone()
    }
}

/// One carried comment's links not addressable yet: the origin its copy carries, its links and
/// what is still pending.
type CommentLinks = (
    comments::OriginKey,
    sections::BodyLinks,
    Vec<sections::Pending>,
);

/// One comment written with links not addressable yet, finished once every write of the copy
/// has landed: see [`Engine::finish_sections`].
struct CommentPending {
    /// The source the destination task is in.
    at: SourceName,
    /// The task copied, at its source.
    source: GlobalId,
    /// The destination task.
    task: NativeId,
    /// The comment at the destination.
    comment: NativeId,
    /// The copy as it was written.
    copy: CommentCopy,
    /// Its links.
    links: sections::BodyLinks,
    /// The placeholders it was written with.
    pending: Vec<sections::Pending>,
}

/// A task a `--comments` write stamped with a record: see [`Journal::stamped`].
struct Stamped {
    /// The task copied, at its source.
    source: GlobalId,
    /// The task as that write sent it, its record included.
    item: Item,
    /// Its forward edges, as that write sent them.
    edges: Vec<DependencyEdge>,
    /// The record it was stamped with.
    record: record::Record,
    /// The comments that record digests.
    comments: Vec<Value>,
}

/// One item whose content was written with links not addressable yet, finished once every
/// write of the copy has landed: see [`Engine::finish_sections`].
struct SectionsPending {
    /// The source the destination item is in.
    at: SourceName,
    /// The destination item.
    id: NativeId,
    /// The item copied, at its source.
    source: GlobalId,
    /// Its content's links.
    links: sections::BodyLinks,
    /// The placeholders the write sent.
    pending: Vec<sections::Pending>,
    /// The item as that write sent it.
    item: Item,
    /// Its forward edges, as that write sent them.
    edges: Vec<DependencyEdge>,
    /// The record of the copy that write landed with.
    record: Option<record::Record>,
    /// The comments its record digests.
    comments: Option<Vec<Value>>,
    /// The item's own counterpart and where its destination says it is.
    own: Option<(NativeId, String)>,
}

/// `content` with each of `assets` pointed where `recorded` says the destination serves it, when
/// it already serves every one — otherwise as it is, since what an upload would serve the rest
/// at is not known before it is made.
fn served_by(
    content: Option<&str>,
    assets: &[AssetPayload],
    recorded: Option<&AssetUploads>,
) -> Option<String> {
    let content = content?;
    let Some(recorded) = recorded.filter(|_| !assets.is_empty()) else {
        return Some(content.to_owned());
    };
    let mut served = BTreeMap::new();
    for payload in assets {
        let Some(url) = recorded.reusable(&payload.name, &payload.sha256) else {
            return Some(content.to_owned());
        };
        served.insert(payload.name.clone(), url.to_owned());
    }
    Some(onetaskgraph_plugin_api::rewrite_asset_references(
        content, &served,
    ))
}

/// The comments a task's record digests: the ones a `--comments` copy leaves at the destination,
/// and none for a copy without it.
fn record_comments(item: &Planned) -> Option<Vec<Value>> {
    item.carried.is_some().then(|| match &item.comment_plan {
        Some(plan) => plan.after.clone(),
        None => comments::digest_entries(
            item.held
                .as_ref()
                .and_then(|held| held.comments.as_deref())
                .unwrap_or_default(),
        ),
    })
}

/// The follow-up write a copy owes the item it landed as `id` from `composed`, whose write sent
/// `sent` and whose answer said the destination stores `stored`: that content verbatim, its
/// rendering's provenance moved onto it where it vouched for what was sent, the assets the write
/// uploaded recorded as it recorded them, and `record` over `comments`, the comments it
/// digests — `None` when the item records nothing a stored content could make untrue, so it is
/// written once and no more.
#[expect(
    clippy::too_many_arguments,
    reason = "each is one fact the first write settled and the follow-up has to repeat"
)]
fn follow_up_of(
    item: &Planned,
    mut composed: Item,
    id: &NativeId,
    edges: &[DependencyEdge],
    destination: &SourceName,
    sent: Option<&str>,
    stored: String,
    record: Option<record::Record>,
    comments: Option<Vec<Value>>,
) -> Option<FollowUp> {
    let before = content_of(&composed).unwrap_or_default().to_owned();
    let sent = sent.unwrap_or_default();
    let metadata = metadata_of(&mut composed);
    if !item.assets.is_empty()
        && let Some(uploads) = stored::served(&before, sent, &item.assets)
    {
        if uploads.0.is_empty() {
            metadata.remove(MetadataKey::ASSETS_KEY);
        } else {
            metadata.insert(MetadataKey::ASSETS_KEY.to_owned(), uploads.to_value());
        }
    }
    let restamped = stored::restamp(metadata, &[&before, sent], &stored);
    if record.is_none() && !restamped {
        return None;
    }
    match &mut composed {
        Item::Task(task) => task.content = Some(stored.clone()),
        Item::Project(project) => project.content = Some(stored.clone()),
        Item::Document(document) => document.content = Some(stored.clone()),
    }
    let record = record.map(|mut record| {
        // A composed digest is of what was sent, now that the copy has sent it.
        if let record::SourceVersion::Composed(_) = record.version {
            record.version = source_version(item, Some(sent));
        }
        record
    });
    Some(FollowUp {
        at: destination.clone(),
        id: id.clone(),
        source: item.source.clone(),
        item: composed,
        edges: edges.to_vec(),
        record,
        comments,
        stored,
        answer: FollowUpAnswer::Exact,
    })
}

/// Whether a destination task that already reads as the source does holds a record of the
/// copy other than the one this copy would leave — and this copy is one that brings it up to
/// date: a `--comments` copy always, and a copy without it only to take away a record that
/// says the comments were carried.
///
/// A copy without `--comments` leaves any other stale record where it is rather than spend a
/// write on it, so a repeat copy of an unchanged source costs exactly what it did before
/// there was a record; the next copy that writes the task records it afresh.
#[expect(
    clippy::too_many_arguments,
    reason = "the same facts `changes` decides from, and whether the source's token is trusted"
)]
fn stale_record(
    item: &Planned,
    held: &Prior,
    target: &NativeId,
    project: &Option<NativeId>,
    edges: &[DependencyEdge],
    delivers: &[TaskRef],
    destination: &ResolvedSource,
    covers: bool,
) -> bool {
    let landing = as_written(
        destination,
        item,
        outgoing(
            item,
            target.clone(),
            project.clone(),
            &recorded(item, Some(held)),
            delivers,
            Some(held),
        ),
    );
    let (Item::Task(_), Item::Task(held_task)) = (&landing, &held.item) else {
        return false;
    };
    // A record that still says both ends are as that copy left them is the record this copy
    // would leave, whatever the destination did to the content it was sent.
    if recorded_unchanged(item, held, &landing, edges, destination.name(), covers) {
        return false;
    }
    let record = record_of(
        item,
        &landing,
        edges,
        destination.name(),
        held.recorded.as_ref(),
        record_comments(item),
    );
    !record.held_by(&held_task.metadata)
        && (item.carried.is_some() || record::carried_comments(&held_task.metadata))
}

/// Whether the record `held` carries says neither end changed since the copy that left it: its
/// source is still what that copy recorded — by its token where `covers` says the token vouches
/// for every part a copy writes, otherwise by the digest of what this copy composes, `outgoing`
/// — and the destination still digests to what it recorded; and what this copy would write
/// beside the item's own fields — its edges, the tasks it delivers, the project it is filed
/// under and its status as the destination writes it — is what the destination holds.
///
/// It is what recognises an unchanged item on a destination that rewrites the Markdown it is
/// sent, where comparing what the copy composes with what the destination stores never could.
fn recorded_unchanged(
    item: &Planned,
    held: &Prior,
    outgoing: &Item,
    edges: &[DependencyEdge],
    destination: &SourceName,
    covers: bool,
) -> bool {
    let version = match token_of(&item.item) {
        Some(token) if covers => record::SourceVersion::Token(token.to_owned()),
        Some(_) => return false,
        None => source_version(item, content_of(outgoing)),
    };
    let with_comments = item.carried.is_some();
    let comments = if with_comments {
        match &held.comments {
            Some(held) => Some(comments::digest_entries(held)),
            None => return false,
        }
    } else {
        None
    };
    let counted = held.counted_edges(edges);
    let now = record::digests_of(&held.item, &counted, comments, destination);
    if !record::holds(
        described(&held.item).1,
        &version.to_string(),
        with_comments,
        &now,
    ) {
        return false;
    }
    // Who may read it is this copy's to decide afresh — a project a new private member makes
    // private — whatever its source holds.
    same_edges(&counted, edges)
        && classification_of(&held.item) == classification_of(outgoing)
        && match (&held.item, outgoing) {
            (Item::Task(held), Item::Task(outgoing)) => {
                // A status and a priority a guarded copy kept beside the source's token are
                // still written by a later copy that does not keep them.
                targets(&held.delivers, destination) == targets(&outgoing.delivers, destination)
                    && held.status == outgoing.status
                    && held.priority == outgoing.priority
                    && held.project == outgoing.project
            }
            (Item::Project(held), Item::Project(outgoing)) => held.status == outgoing.status,
            (Item::Document(held), Item::Document(outgoing)) => held.project == outgoing.project,
            _ => false,
        }
}

/// Whether `body` links the record at `location`: holds that address whole, with nothing either
/// side of it that would make it part of a longer one.
fn links_to(body: &str, location: &str) -> bool {
    body.match_indices(location).any(|(at, _)| {
        !body[..at].ends_with(|c: char| c.is_alphanumeric() || matches!(c, '/' | ':' | '.' | '-'))
            && !body[at + location.len()..].starts_with(|c: char| c.is_alphanumeric() || c == '-')
    })
}

/// Whether the destination already reads exactly as this copy would leave it.
///
/// The destination's own `url` and timestamps are excluded because a copy never writes
/// them, so a difference there is not one this copy would close.
fn same(held: &Item, outgoing: &Item, destination: &SourceName) -> bool {
    match (held, outgoing) {
        (Item::Task(held), Item::Task(outgoing)) => {
            // Qualified before they are compared, so `T-1` and `folder:T-1` at the destination
            // `folder` are the one entry they are.
            targets(&held.delivers, destination) == targets(&outgoing.delivers, destination)
                && targets(&held.delivered_by, destination)
                    == targets(&outgoing.delivered_by, destination)
                && held.title == outgoing.title
                && held.content == outgoing.content
                && held.status == outgoing.status
                && held.priority == outgoing.priority
                && held.labels == outgoing.labels
                && held.project == outgoing.project
                // Repositories are compared as the field they are: a hosted destination also
                // reports the slot it carries them in as metadata, which a folder keeps in its
                // front matter instead — the one difference `record::digest` leaves out too.
                && record::digest_metadata(&held.metadata)
                    == record::digest_metadata(&outgoing.metadata)
                && held.repositories == outgoing.repositories
                && held.classification == outgoing.classification
                // Assignees only where the copy writes them: one it leaves alone is the
                // destination's own, whatever it holds.
                && writes_held_assignees(held, outgoing)
        }
        (Item::Project(held), Item::Project(outgoing)) => {
            held.title == outgoing.title
                && held.content == outgoing.content
                && held.status == outgoing.status
                && held.labels == outgoing.labels
                && held.metadata == outgoing.metadata
                && held.repositories == outgoing.repositories
                && held.classification == outgoing.classification
        }
        // No status, because a document has none; no edges, because it is in no graph.
        (Item::Document(held), Item::Document(outgoing)) => {
            held.title == outgoing.title
                && held.content == outgoing.content
                && held.labels == outgoing.labels
                && held.project == outgoing.project
                && held.metadata == outgoing.metadata
                && held.repositories == outgoing.repositories
                && held.classification == outgoing.classification
        }
        _ => false,
    }
}

/// Whether `held` already holds the assignees `outgoing` writes — always, when it writes none.
fn writes_held_assignees(held: &Task, outgoing: &Task) -> bool {
    outgoing.assignees.as_ref().is_none_or(|written| {
        super::assignees::same_people(written, held.assignees.as_deref().unwrap_or_default())
    })
}

/// Whether the destination's forward edges already say what this copy would write.
fn same_edges(held: &[DependencyEdge], outgoing: &[DependencyEdge]) -> bool {
    let ends = |edges: &[DependencyEdge]| {
        let mut ends: Vec<(String, ItemKind, DependencyKind)> = edges
            .iter()
            .map(|edge| (edge.to.to_string(), edge.to.kind, edge.kind))
            .collect();
        ends.sort_by(|left, right| left.0.cmp(&right.0));
        ends
    };
    ends(held) == ends(outgoing)
}

/// Each read edge as the destination should record it, or `None` when its far end is a
/// member of this copy whose destination id is not known yet.
///
/// `destination` is the source the near end lands in. A far end this copy landed in another
/// source — a routed copy's other half — is written qualified, which every source records
/// as a cross-source edge on the near item.
fn mapped_edges(
    edges: &[DependencyEdge],
    origin: &SourceName,
    destination: &SourceName,
    copied: &[GlobalId],
    written: &BTreeMap<String, GlobalId>,
) -> Vec<Option<DependencyEdge>> {
    edges
        .iter()
        .map(|edge| {
            // An opaque far end names a tracker rather than a source, and is carried as it was
            // read: nothing this copy lands is what it names.
            if edge.to.designator().is_some() {
                return Some(edge.clone());
            }
            let far = GlobalId::new(origin.clone(), NativeId(edge.to.id().to_owned()));
            let qualified = edge
                .to
                .is_qualified()
                .then(|| edge.to.id().parse::<GlobalId>().ok())
                .flatten()
                .filter(|far| copied.contains(far));
            let id = if let Some(native) = names(&edge.to, destination) {
                // A far end already qualified to the destination's own source is that
                // source's own item, so it is written the way that source names its own:
                // unqualified. Leaving it qualified would have the destination hold an
                // edge into itself written as if it left, which is the one spelling the
                // reserved key exists to keep for edges that really do.
                Some(native)
            } else if !edge.to.is_qualified() && copied.contains(&far) {
                // A member of this copy, inside one source included: a copy there lands
                // beside the item it was read from, so the edge names the copy rather than
                // the original the copy's own project does not hold.
                written
                    .get(&far.to_string())
                    .map(|landed| spelled_from(landed, destination))
            } else if let Some(far) = qualified {
                // A member of this copy in another source — a home and its member copied as
                // one set — names that member's copy for the same reason.
                written
                    .get(&far.to_string())
                    .map(|landed| spelled_from(landed, destination))
            } else if edge.to.is_qualified() || origin == destination {
                // Already naming a source of its own, or a copy inside one source where
                // the far end's own id is the destination's id.
                Some(edge.to.id().to_owned())
            } else {
                Some(far.to_string())
            }?;
            DependencyEndpoint::new(id, edge.to.kind)
                .ok()
                .map(|to| DependencyEdge {
                    from: edge.from.clone(),
                    to,
                    kind: edge.kind,
                })
        })
        .collect()
}

/// Each `delivers` entry of one planned task as the destination should hold it, or `None`
/// where it names a member of this copy whose destination id is not known yet. Empty for
/// anything but a task.
fn delivers_of(
    item: &Planned,
    destination: &SourceName,
    copied: &[GlobalId],
    written: &BTreeMap<String, GlobalId>,
) -> Vec<Option<TaskRef>> {
    match &item.item {
        Item::Task(task) => mapped_delivers(
            &task.delivers,
            &item.source.source,
            destination,
            copied,
            written,
        ),
        Item::Project(_) | Item::Document(_) => Vec::new(),
    }
}

/// Each `delivers` entry as the destination should hold it, or `None` when it names a member
/// of this copy whose destination id is not known yet.
///
/// An entry naming a member of the copied set becomes that member's own id at the
/// destination — bare, because the member is the destination's, unless the id holds a colon
/// a bare entry would be misread by. Every other entry is carried through qualified, so a
/// bare one read at `origin` goes on naming a task of `origin`.
fn mapped_delivers(
    entries: &[TaskRef],
    origin: &SourceName,
    destination: &SourceName,
    copied: &[GlobalId],
    written: &BTreeMap<String, GlobalId>,
) -> Vec<Option<TaskRef>> {
    entries
        .iter()
        .map(|entry| {
            let qualified = entry.in_source(origin);
            let Ok(far) = qualified.as_str().parse::<GlobalId>() else {
                return Some(qualified);
            };
            if !copied.contains(&far) {
                return Some(qualified);
            }
            let landed = written.get(&far.to_string())?;
            let native = &landed.native;
            Some(
                if &landed.source != destination || native.as_str().contains(':') {
                    TaskRef::qualified(&landed.source, native)
                } else {
                    TaskRef::new(native.as_str())
                        .unwrap_or_else(|_| TaskRef::qualified(destination, native))
                },
            )
        })
        .collect()
}

/// How many of one task's `delivers` entries name a member of the copied set.
fn members_named(entries: &[TaskRef], origin: &SourceName, copied: &[GlobalId]) -> u64 {
    let named = entries
        .iter()
        .filter(|entry| {
            entry
                .in_source(origin)
                .as_str()
                .parse::<GlobalId>()
                .is_ok_and(|far| copied.contains(&far))
        })
        .count();
    u64::try_from(named).unwrap_or(u64::MAX)
}

/// The entries that could be resolved, which is every one of them on the second pass.
fn resolved_entries(entries: &[Option<TaskRef>]) -> Vec<TaskRef> {
    entries.iter().flatten().cloned().collect()
}

/// How an item landed at `landed` is named from one landed in `near`: by its native id in
/// the same source, qualified from any other.
fn spelled_from(landed: &GlobalId, near: &SourceName) -> String {
    if &landed.source == near {
        landed.native.0.clone()
    } else {
        landed.to_string()
    }
}

/// Record `item` among those `running` carries classified private, when it is: by its source
/// id, and by its destination id when it already has one.
fn note_private(running: &mut Running, item: &Planned) {
    if item.classification != Classification::Private {
        return;
    }
    let mut ids = vec![item.source.clone()];
    if let Target::Update { id, .. } = &item.target {
        ids.push(GlobalId::new(item.to.clone(), id.clone()));
    }
    for id in ids {
        if !running.private.contains(&id) {
            running.private.push(id);
        }
    }
}

/// The item `endpoint` names, read from an item of `origin`: a native id names one of
/// `origin`'s own, and an opaque one names none this configuration can say.
fn far_end(endpoint: &DependencyEndpoint, origin: &SourceName) -> Option<GlobalId> {
    if endpoint.designator().is_some() {
        return None;
    }
    if endpoint.is_qualified() {
        endpoint.id().parse().ok()
    } else {
        Some(GlobalId::new(
            origin.clone(),
            NativeId(endpoint.id().to_owned()),
        ))
    }
}

/// The native id a qualified endpoint names at `destination`, when it names one there.
fn names(endpoint: &DependencyEndpoint, destination: &SourceName) -> Option<String> {
    if !endpoint.is_qualified() {
        return None;
    }
    let id: GlobalId = endpoint.id().parse().ok()?;
    (&id.source == destination).then_some(id.native.0)
}

/// Where a project copy files the tasks it carries: in each source a task lands in, the
/// home there or the home's member project.
type Filing = BTreeMap<SourceName, NativeId>;

/// The edges that could be resolved, which is every one of them on the second pass.
fn resolved(edges: &[Option<DependencyEdge>]) -> Vec<DependencyEdge> {
    edges.iter().flatten().cloned().collect()
}

/// Refuse an item of a member copy whose edge names a member that copy does not carry
/// and whose destination id nothing records.
///
/// `unrecorded` is empty for every other copy, which is what makes this a no-op there. An
/// edge already naming a source of its own, and every edge of a copy inside one source,
/// is written as it was read by [`mapped_edges`], so neither can need a recorded origin.
fn unrecorded_far_end(item: &Planned, unrecorded: &[GlobalId]) -> Result<(), EngineError> {
    if item.source.source == item.to {
        return Ok(());
    }
    for edge in &item.edges {
        if !edge.to.is_native() {
            continue;
        }
        let far = GlobalId::new(
            item.source.source.clone(),
            NativeId(edge.to.id().to_owned()),
        );
        if unrecorded.contains(&far) {
            return Err(EngineError::UnrecordedMember {
                item: item.source.clone(),
                member: far,
                destination: item.to.clone(),
            });
        }
    }
    Ok(())
}
