//! Best-effort projection of the journal-owned graph into its onetaskgraph project.
//!
//! The reconcile loop remains the only author of graph state: it hands immutable folded
//! snapshots to this worker, and the worker only projects them. Store reads never feed back
//! into scheduling, and a failed or slow write is reported and retried off the engine thread
//! — unless the store *refused* it, which no retry changes: that is reported once and
//! attempted again only when the run's graph does. See [`FailureClass`]. A rate limit that
//! names how long to wait is waited out in full, with no call of any kind before it passes.
//!
//! # Ownership: the write-back owns exactly what the plan document declares
//!
//! A projection is a *total replacement* of the destination item, so every field it does
//! not restate is a field it deletes. One rule decides each of them, and it is the plan
//! document: what a plan declares, this worker owns and overwrites; what a plan does not
//! model, it reads off the destination and writes back unchanged. The consequences are
//! enumerated here rather than rediscovered per field, so the next field a destination item
//! grows is decided by the rule instead of by whichever neighbour it was copied from.
//!
//! * **A task's title, body, status, dependency edges and engine metadata are declared** —
//!   by the node the plan holds and the graph the run folded — so the projection replaces
//!   them. That is the whole point of the projection. A node carrying no title is written
//!   under its **id**: a destination that requires one refuses an item with none, and the
//!   copy is a whole-project write, so one untitled node stops every item of the run
//!   reaching the board. Untitled nodes are ordinary — see
//!   `graph::check_declared_version` for why an edited graph carries them.
//! * **A task's identity is its lineage root's, not the node's.** A `retry`'s replacement
//!   is projected onto the item the superseded node already holds, keyed by the lineage
//!   root and saying what the lineage head says. Entry 80 of `docs/contract-divergences.md`
//!   is the rule; [`Lineages`] computes it, [`task_document`] renders it, and `taskgraph`'s
//!   reader reads past the two lineage keys the way it reads past the settlement.
//! * **A project's title is not declared.** A plan's `name` is reserved project metadata,
//!   never the board's own heading, so the destination's title is read and written back. In
//!   particular it is *not* the project's native identifier: on a store where those two
//!   coincide the difference is invisible, and on one where they do not, writing the
//!   identifier renames a person's board to a machine id.
//! * **A project's description is not declared**, so the destination's `content` is read and
//!   written back.
//! * **Labels are not modelled by a plan at all** — neither a project's nor a task's — so
//!   both are read off the destination and written back. A destination may refuse a write
//!   whose labels differ from the ones it holds, so a projection that dropped them would
//!   stop reaching the board the moment anybody labelled one of its items.
//! * **Metadata a plan does not name is not declared**, so the destination's own keys
//!   survive and only the reserved `onepipeline.*` keys this worker owns are rewritten.
//!
//! What the destination alone owns — its native id, URL and timestamps — is never written
//! by anybody, so it is neither replaced nor carried.
//!
//! # The status a node is projected under is the settlement's own word
//!
//! A settled run is read as the record of what happened, so the word on the board is the
//! word the settlement used: `done` for a node that is done, `failed` for a task failure,
//! `provider-failed` for the provider death that is not the work's fault, `cancelled` for a
//! cancel, `parked` for a planner's own idle, and `skipped` for a node a failed dependency
//! made unsafe. See [`ProjectedStatus`] for why that is a *name* rather than one of
//! onetaskgraph's seven normalised categories.
//!
//! Beside it, the **change that closed the node**: the commit its change landed at, or the
//! change request a person reads it in. A status alone cannot say which of those happened,
//! and a reader closing work on the status would close it on a change that reached nobody.
//! Both are absent for a node with no change of its own, which is most of them.
//!
//! # A projection carries what changed since it landed
//!
//! Against a hosted destination every item a copy reads or writes spends the same allowance
//! every other reader of the token needs, so an attempt carries only the lineages whose
//! projection differs from what the run last put on the board, named as the copy's members —
//! and never the whole project. What landed is the run's **landed baseline**
//! ([`LandedBaseline`], `writeback-landed.json` beside the record): seeded from the launch's own
//! read before the first projection, advanced per item only as a write lands, and read by every
//! later driver, an adopting one included. It also says where each lineage's item is, so no
//! attempt reads the project's page of tasks; a lineage it does not hold — a run an older build
//! started — is read once by its own id. After a failure the next attempt carries what has still
//! not landed and nothing more. What a member copy does not name it neither reads nor rewrites,
//! so the ownership rule above still decides every field of every item a projection writes, and
//! a person's edit on an item the run did not change since it last landed stands. Every attempt
//! is appended to the run's projection record; see [`ProjectionRecord`].
//!
//! # The store is linked, and built afresh for every attempt
//!
//! Every read and the copy go through `onetaskgraph-core`'s own [`Engine`], in process, and
//! every answer is that library's own value — a [`CopyReport`], a [`SourceFailure`], an
//! [`EngineError`] — so what a failure *is* is matched on its type and never read off a
//! message or an exit status. Each attempt builds its engine from the configuration the
//! launch record's directory discovers, with the shadow source declared beside it, and
//! drops it when the attempt ends: a hosted source keeps its whole-board read for as long as
//! it lives, so an engine that outlived one attempt would answer the next from the board as
//! it was.

use std::collections::{BTreeMap, BTreeSet};
use std::future::Future;
use std::num::{NonZeroU32, NonZeroU64};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use onetaskgraph_core::config::{Layer, Origin as SettingOrigin, Setting, SettingPath};
use onetaskgraph_core::{
    classify, ConfigError, CopyAction, CopyItems, CopyReport, CopyRequest, CopyScope, Delivered,
    DeliveryOutcome, Engine, EngineError, Failure, GlobalId, Qualified, QueryResponse,
    SourceFailure,
};
use onetaskgraph_plugin_api::{
    Label, NativeId, Project, SourceError, SourceName, StatusCategory, Task,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

use crate::cli::{
    DEFAULT_WRITEBACK_ITEM_BUDGET_SECONDS, WRITEBACK_CLASSIFIED_COMMANDS,
    WRITEBACK_COMMAND_FLOOR_SECONDS, WRITEBACK_LANDED_FILE, WRITEBACK_LANDED_SCHEMA_VERSION,
    WRITEBACK_MEMBER_READ, WRITEBACK_PROJECTIONS_FILE, WRITEBACK_PROJECTIONS_SCHEMA_VERSION,
    WRITEBACK_REFUSED_CLASS,
};
use crate::edits::Operation;
use crate::event::Source;
use crate::graph::{Landing, NodeStatus};
use crate::ledger::{LaunchRecord, RunPaths};
use crate::plan::Node;
use crate::projection::RunState;
use crate::taskgraph::{QualifiedId, Store, NODE_KEY, SUPERSEDES_KEY};

const SHADOW_SOURCE: &str = "onepipeline-writeback";
/// The reserved key naming the plan node a destination item is the shadow of: the
/// **lineage root's** id, which is what the store's `task list` is read back by.
const ID_KEY: &str = "onepipeline.id";
// The two lineage keys, [`NODE_KEY`] and [`SUPERSEDES_KEY`], live beside the settlement
// key in `taskgraph`, because the plan reader has to skip them exactly as it skips that
// one; [`tests::every_word_and_key_this_projection_writes_is_named_by_the_divergence`]
// holds them to entry 80 of `docs/contract-divergences.md`.
/// The reserved key saying whether the change a node published reached its base.
///
/// Named once, and held against the document that records them by
/// [`tests::every_word_and_key_this_projection_writes_is_named_by_the_divergence`]:
/// `docs/contract.md` fixes a narrower vocabulary than this projection writes, so
/// what these three and the words below are reconciled against is
/// `docs/contract-divergences.md`, where that departure is recorded.
const LANDING_KEY: &str = "onepipeline.landing";
/// The reserved key naming the commit a landed change reached its base at.
const LANDING_COMMIT_KEY: &str = "onepipeline.landing_commit";
/// The reserved key naming where a person reads the change a node published.
const CHANGE_URL_KEY: &str = "onepipeline.change_url";
/// The reserved key naming the evidence tier a landing rests on, where that is not
/// the run's own observation: an operator's statement, settled from evidence —
/// see [`StatedLanding::tier`](crate::edits::StatedLanding::tier).
const LANDING_EVIDENCE_KEY: &str = "onepipeline.landing_evidence";
/// The shadow task's own top-level field carrying the tickets a node delivers, held against
/// the same document as the words and keys above.
const DELIVERS_FIELD: &str = "delivers";
// Cross-platform runners have measured real store calls taking longer than ten seconds
// under suite-wide contention. This remains a backstop for an unreachable store, not a
// latency target: projection stays off the reconcile loop while the call runs. It is the
// whole deadline for the reads, and the floor under the copy's — see [`Deadline`].
const COMMAND_FLOOR: Duration = Duration::from_secs(WRITEBACK_COMMAND_FLOOR_SECONDS);
// The retry schedule, chosen from the refusal that produces it in practice: a hosted
// destination's rate limiter, which every further attempt extends. It starts where a fixed
// quarter-second schedule did, so a projection that fails once still lands unnoticeably
// fast, and quadruples so that five attempts reach the ceiling inside twenty seconds rather
// than spending a limiter's whole minutes-long window asking several times a minute. The
// ceiling is what GitHub's secondary limit — reported by no endpoint a caller can poll —
// documents as the wait to take: at least one minute. It is a ceiling and not a give-up,
// because a board nobody projects to stays wrong for the rest of the run.
const FIRST_RETRY_AFTER: Duration = Duration::from_millis(250);
const RETRY_GROWTH: u32 = 4;
const RETRY_CEILING: Duration = Duration::from_secs(60);
// Closeout never inherits the duration of a store call. A slow store may keep working in
// the worker, but it still cannot turn a completed graph into run settlement.
const CLOSEOUT_WAIT: Duration = Duration::from_millis(2_250);
// The three store calls one attempt makes, by the name each one's refusals carry. Each
// one's failure is classified by its own type.
const PROJECT_SHOW: &str = WRITEBACK_CLASSIFIED_COMMANDS[0];
const PROJECT_COPY: &str = WRITEBACK_CLASSIFIED_COMMANDS[2];
// What a member projection reads each named member with, in place of the page of tasks.
const TASK_SHOW: &str = WRITEBACK_MEMBER_READ;
// What building an attempt's store is called where it outlasts the floor.
const STORE_OPEN: &str = "store-open";

/// How long one store call may run, and the account a refusal gives of the figure.
///
/// A call that outlasts it is **cancelled**: the future is dropped where it waits, and the
/// engine that was driving it — a hosted plugin's child process with it — is dropped before
/// the attempt is recorded, so nothing the cancelled call started lands after the record says
/// it was refused.
///
/// The reads are the same size whatever the plan, so [`COMMAND_FLOOR`] alone bounds them.
/// The copy writes one item per lineage it carries, so its deadline is the floor **plus** the
/// launch's per-item budget multiplied by those items — the list an [`Unprojected`] surface
/// names. Added rather than the larger of the two, because every copy also spends the fixed
/// round trips a read does: a seven-item copy onto the `plans` board measured 71 to 72 seconds
/// against the 70 the larger of the two allowed it (#521). The account is derived from the
/// figure rather than stored beside it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Deadline {
    /// The fixed floor, which is the whole deadline for a read.
    Floor,
    /// The copy's: `floor + per_item × items`, with the budget in seconds.
    Copy { per_item: NonZeroU64, items: usize },
}

impl Deadline {
    /// The budget multiplied through, in seconds, before the floor is added.
    ///
    /// Exact for every product that fits in a `u64` of seconds, and `u64::MAX` seconds for
    /// one that does not — saturating rather than wrapping, because a product that wrapped
    /// to nothing would leave the floor alone governing exactly the plan the budget exists
    /// to accommodate.
    fn product(per_item: NonZeroU64, items: usize) -> Duration {
        // llmlint: ignore[changed_behavior_has_e2e] a product past `u64::MAX` seconds is
        // more items than any store holds and more seconds than any host runs, so no
        // journey can reach the saturated arm; the unit test holds the exact product on
        // both sides of that line, and the e2e journeys drive the deadline it produces.
        let items = u64::try_from(items).unwrap_or(u64::MAX);
        Duration::from_secs(per_item.get().saturating_mul(items))
    }

    /// How long the command is allowed.
    fn within(self) -> Duration {
        match self {
            Self::Floor => COMMAND_FLOOR,
            Self::Copy { per_item, items } => {
                COMMAND_FLOOR.saturating_add(Self::product(per_item, items))
            }
        }
    }

    /// The one line a call that outlasted this is refused with.
    fn refusal(self, name: &str) -> String {
        let seconds = self.within().as_secs();
        match self {
            Self::Floor => format!("{name} exceeded {seconds} seconds"),
            Self::Copy { per_item, items } => format!(
                "{name} exceeded {seconds} seconds (the {} second floor + {items} {} × {} {} \
                 per item)",
                COMMAND_FLOOR.as_secs(),
                if items == 1 { "item" } else { "items" },
                per_item,
                if per_item.get() == 1 {
                    "second"
                } else {
                    "seconds"
                }
            ),
        }
    }
}

/// The refusal for a per-item budget of zero, wherever one is named.
///
/// One sentence for the flag, the variable and the config key, each naming the spelling
/// that carried it: zero is no budget at all, and leaves the floor as the whole deadline.
pub(crate) fn refused_zero_budget(spelling: &str) -> String {
    format!(
        "{spelling} names a write-back budget of zero seconds per item, which is no budget \
         at all — give it a positive whole number of seconds, or leave it out to take \
         {DEFAULT_WRITEBACK_ITEM_BUDGET_SECONDS} seconds per item"
    )
}

#[derive(Clone, PartialEq)]
struct Snapshot {
    project: QualifiedId,
    dir: PathBuf,
    nodes: BTreeMap<String, Node>,
    statuses: BTreeMap<String, NodeStatus>,
    // llmlint: ignore-block[invalid_states_unrepresentable] the string-keyed maps below are
    // copies of `RunState`'s own fields, each of which records there why its value is the
    // plain string every identifier in this crate is: an outcome is the *harness's* open
    // vocabulary and a set declared here would refuse a classification that layer added, a
    // landing commit is checked where it enters by `vcs::landing_commit_of`, and a change
    // URL is the sibling's own and never minted here. Narrowing a copy of a field the crate
    // holds unnarrowed would put a type on this side of a boundary the other side does not
    // have.
    /// The named outcome each settled node carries, which is what tells a
    /// provider death from a task the agent failed. Both settle `failed`.
    outcomes: BTreeMap<String, String>,
    /// Whether each published node's change reached its base branch.
    landings: BTreeMap<String, Landing>,
    /// The commit each landed change reached its base at.
    landing_commits: BTreeMap<String, String>,
    /// Where a person reads the change a node published.
    change_urls: BTreeMap<String, String>,
    // llmlint: ignore-end[invalid_states_unrepresentable]
    /// Where an operator settling a node from evidence stated its work landed.
    // llmlint: ignore[invalid_states_unrepresentable] a node id is the plain `String` every neighbouring map of this struct keys by — `landings`, `branches`, `change_urls` — and it was validated where the graph took the node; a node-id newtype on this one field would disagree with each of them and convert at every read, which `src/AGENTS.md` names as drift. The value is the typed half: `StatedLanding` holds only a spelling its own parser accepted.
    stated_landings: BTreeMap<String, crate::edits::StatedLanding>,
    settlements: BTreeMap<String, Value>,
    // llmlint: ignore[invalid_states_unrepresentable] both sides are node ids, the plain
    // `String` every map here keys by, copied from `RunState::superseded`, which records the
    // same reason and where each side was validated.
    /// The replacement each superseded node was retried under — what a lineage is read off,
    /// and nothing else: never a title, never an id's suffix.
    superseded: BTreeMap<String, String>,
    project_metadata: BTreeMap<String, Value>,
    /// Whether a node the run has not started is written as claimed or released.
    claim: Claim,
}

/// What a node the run has not started says about the work it names.
///
/// While a driver drives the run, the run has claimed its whole plan, and an unstarted node
/// is written `queued`, which the store counts as a claim on every ticket it delivers. At
/// closeout — settled or stopped — a node that never started is written `todo`, which the
/// store counts as a release.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Claim {
    Held,
    Released,
}

impl Snapshot {
    /// The lineages this snapshot projects, one shadow task apiece.
    fn lineages(&self) -> Lineages {
        Lineages::of(self)
    }

    /// The word one node's state is written under in this snapshot.
    fn word_of(&self, node: &str) -> ProjectedStatus {
        projected(
            self.statuses
                .get(node)
                .copied()
                .unwrap_or(NodeStatus::Cancelled),
            self.outcomes.get(node).map(String::as_str),
            self.claim,
        )
    }
}

/// A node and every retry replacement of it, in order — read off the snapshot's
/// `superseded` map and nothing else.
///
/// Its **root** is the id the plan authored or an `add` stated; its **head** is the one node
/// in it nothing superseded. One shadow task is written per lineage, under the root's file
/// and member id, saying what the head says. Superseded nodes are projected as no shadow task
/// of their own, which is what stops a retry minting a new destination item beside a closed
/// one. Entry 80 of `docs/contract-divergences.md` states the rule.
struct Lineages {
    // llmlint: ignore-block[invalid_states_unrepresentable] node ids, the plain `String` the
    // snapshot's own maps key by, for the reason `Snapshot::superseded` records.
    /// Every node's root, itself for a node nothing superseded and nothing replaced.
    roots: BTreeMap<String, String>,
    /// Each root's chain, root first and head last.
    chains: BTreeMap<String, Vec<String>>,
    // llmlint: ignore-end[invalid_states_unrepresentable]
}

impl Lineages {
    fn of(snapshot: &Snapshot) -> Self {
        let replacements: BTreeSet<&String> = snapshot.superseded.values().collect();
        let mut lineages = Self {
            roots: BTreeMap::new(),
            chains: BTreeMap::new(),
        };
        // Every node that is nobody's replacement roots a lineage, and every node the walk
        // from a root does not reach — a replacement whose predecessor the snapshot no longer
        // holds — roots one of its own, so no node the snapshot holds is left unprojected.
        let rooted: Vec<&String> = snapshot
            .nodes
            .keys()
            .filter(|id| !replacements.contains(id))
            .collect();
        for root in rooted {
            lineages.walk(snapshot, root);
        }
        let unreached: Vec<String> = snapshot
            .nodes
            .keys()
            .filter(|id| !lineages.roots.contains_key(*id))
            .cloned()
            .collect();
        for root in &unreached {
            if !lineages.roots.contains_key(root) {
                lineages.walk(snapshot, root);
            }
        }
        lineages
    }

    /// Walk the supersessions forward from `root`, stopping at a node the snapshot does not
    /// hold or one already placed — the reconciler refuses a replacement id already taken, so
    /// the second is defence rather than a case.
    fn walk(&mut self, snapshot: &Snapshot, root: &str) {
        let mut chain = vec![root.to_owned()];
        let mut at = root;
        while let Some(next) = snapshot.superseded.get(at) {
            if !snapshot.nodes.contains_key(next)
                || self.roots.contains_key(next)
                || chain.contains(next)
            {
                break;
            }
            chain.push(next.clone());
            at = next;
        }
        for id in &chain {
            self.roots.insert(id.clone(), root.to_owned());
        }
        self.chains.insert(root.to_owned(), chain);
    }

    /// The root of the lineage `id` belongs to, or `id` itself where the snapshot never held
    /// it — an item of the destination this run did not write.
    fn root_of<'a>(&'a self, id: &'a str) -> &'a str {
        self.roots.get(id).map_or(id, String::as_str)
    }

    /// Each root's chain, root first and head last.
    fn chain(&self, root: &str) -> Option<&[String]> {
        self.chains.get(root).map(Vec::as_slice)
    }

    /// Every root, in order.
    fn roots(&self) -> impl Iterator<Item = &String> {
        self.chains.keys()
    }

    fn len(&self) -> usize {
        self.chains.len()
    }
}

/// One projection that failed, as the planner is told about it.
///
/// Carried out of the worker rather than raised there: the worker runs on a
/// thread of its own and the journal has one writer, so what it produces is this
/// record and the reconcile loop raises the surface.
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct Unprojected {
    /// The onetaskgraph project the projection could not reach.
    pub project: QualifiedId,
    /// The items it was carrying, by the plan node id each is the shadow of — a lineage's
    /// root, where the node was retried.
    // llmlint: ignore-block[invalid_states_unrepresentable] a node id is the plain string
    // every identifier in this crate is, for the reason `NodeResult::superseded_by` records:
    // these are the ids of the plan this run is executing, read straight off the snapshot
    // the worker was projecting, and the only thing done with them is naming them on a
    // surface.
    pub items: Vec<String>,
    // llmlint: ignore-end[invalid_states_unrepresentable]
    /// What the sibling, or this worker, said went wrong.
    pub reason: String,
    /// What the store classed the failure as, where it wrote a class this worker reads.
    pub classified: Option<Classified>,
}

/// Whether asking the store again, unchanged, could change its answer — the store's own
/// word, and the one thing this worker branches on.
///
/// `refused` is a failure no retry can change: a projection the store refused never reaches
/// the board however often it is asked, and each attempt against a hosted destination spends
/// its allowance for nothing. `transient` is everything a wait can change, a rate limit
/// included. The mapping from a failure to its class is the store's —
/// [`onetaskgraph_core::classify`] — and is never restated here, and no message is ever read
/// to decide. This is the record's own vocabulary, which [`ProjectionRecord`] writes and a
/// reader of it names, so it is converted from the store's by an exhaustive match: a class
/// the store adds is a compile error here rather than a line this build writes and cannot
/// read back.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum FailureClass {
    /// A failure no retry can change.
    Refused,
    /// A failure a wait can change.
    Transient,
}

impl FailureClass {
    fn as_str(self) -> &'static str {
        match self {
            Self::Refused => WRITEBACK_REFUSED_CLASS,
            Self::Transient => "transient",
        }
    }
}

impl From<onetaskgraph_core::FailureClass> for FailureClass {
    fn from(class: onetaskgraph_core::FailureClass) -> Self {
        match class {
            onetaskgraph_core::FailureClass::Refused => Self::Refused,
            onetaskgraph_core::FailureClass::Transient => Self::Transient,
        }
    }
}

/// What the store said about one failed attempt: its class, and the kind of failure it was.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Classified {
    pub class: FailureClass,
    // llmlint: ignore[invalid_states_unrepresentable] the store's `kind` is open by its own
    // contract — a source error's kind is its own wire tag, and a plugin a release newer than
    // this build speaks kinds this one has never seen — and it is only ever named to a reader.
    // `class` is the closed half this worker acts on.
    pub kind: String,
}

impl Classified {
    /// The class and kind, as the line and the surface each name them.
    pub(crate) fn said(&self) -> String {
        format!("class: {}, kind: {}", self.class.as_str(), self.kind)
    }

    fn refused(&self) -> bool {
        self.class == FailureClass::Refused
    }

    /// An engine error, classed by the source error it wraps — or, where it wraps none, as
    /// the failure the engine decided on its own, which no retry changes.
    fn of_engine(error: &EngineError) -> Self {
        let (kind, cause) = cause_of(error);
        Self {
            class: classify(cause).into(),
            kind,
        }
    }

    /// A configuration the store will not run on, which no retry changes.
    fn of_config(error: &ConfigError) -> Self {
        Self {
            class: classify(None).into(),
            kind: match error {
                ConfigError::Read { .. } => "config-read",
                ConfigError::Syntax { .. } => "config-syntax",
                ConfigError::Setting { .. } => "config-setting",
            }
            .to_owned(),
        }
    }

    /// An answer that is missing the one item a `show` addressed and reports no failure:
    /// an item that is not there, which the store's own CLI decided and which no retry
    /// changes.
    fn no_such_item() -> Self {
        Self {
            class: classify(None).into(),
            kind: "no-such-item".to_owned(),
        }
    }

    /// Several failures of one answer, refused only where **every** one is: a source that
    /// could not be reached beside one that refused could still answer next time. Each kind
    /// named once, in the order the failures arrived.
    fn of_all(failures: impl IntoIterator<Item = Self>) -> Option<Self> {
        let failures: Vec<Self> = failures.into_iter().collect();
        if failures.is_empty() {
            return None;
        }
        let class = if failures.iter().all(Self::refused) {
            FailureClass::Refused
        } else {
            FailureClass::Transient
        };
        let mut kinds: Vec<String> = Vec::new();
        for failure in failures {
            if !kinds.contains(&failure.kind) {
                kinds.push(failure.kind);
            }
        }
        Some(Self {
            class,
            kind: kinds.join(", "),
        })
    }

    /// One source's failure in a partial answer.
    fn of_source(error: &SourceError) -> Self {
        Self {
            class: classify(Some(error)).into(),
            kind: source_kind(error).to_owned(),
        }
    }

    /// The store's failure for one delivered ticket it could not keep in step: the one
    /// failure that arrives as a [`Failure`] with no [`EngineError`] behind it, so its class
    /// and kind are the ones the store decided, read through the failure's own accessors.
    fn of_delivery(failure: &Failure) -> Self {
        Self {
            class: failure.class().into(),
            kind: failure.kind().to_owned(),
        }
    }
}

/// The kind an engine error amounts to, and the source error that caused it, if one did.
///
/// A failure that wraps another — a destination that could not be built, a source that
/// refused part of a copy, a copy that could not be undone — takes the kind and cause of the
/// failure it wraps, because that is what a caller has to act on; the rest are failures the
/// engine decided on its own. The words are the store's own kinds for each, so the record
/// names a failure as the store's failure document did. Exhaustive, so a failure the store
/// adds is a compile error here rather than one this worker silently misclasses.
fn cause_of(error: &EngineError) -> (String, Option<&SourceError>) {
    let decided = |kind: &str| (kind.to_owned(), None);
    match error {
        EngineError::UnknownSource { .. } => decided("unknown-source"),
        EngineError::Token { .. } => decided("page-token"),
        EngineError::NoSources => decided("no-sources"),
        EngineError::NotWritable { .. }
        | EngineError::CommentsNotWritable { .. }
        | EngineError::StatusNotWritable { .. }
        | EngineError::MetadataNotWritable { .. }
        | EngineError::PriorityNotWritable { .. }
        | EngineError::ContentNotWritable { .. } => decided("not-writable"),
        EngineError::NoDocuments { .. } => decided("no-documents"),
        EngineError::NoPriority { .. } => decided("no-priority"),
        EngineError::NoComments { .. } => decided("no-comments"),
        EngineError::NoSuchItem { .. }
        | EngineError::NoSuchTask { .. }
        | EngineError::NoSuchProject { .. }
        | EngineError::NoSuchDocument { .. } => decided("no-such-item"),
        EngineError::NoSuchComment { .. } => decided("no-such-comment"),
        EngineError::StaleOrigin { .. } => decided("stale-origin"),
        EngineError::NotAMember { .. } => decided("not-a-member"),
        EngineError::UnrecordedMember { .. } => decided("unrecorded-member"),
        EngineError::DestinationUnavailable { error, .. }
        | EngineError::SourceRefused { error, .. }
        | EngineError::SourceUnavailable { error, .. }
        | EngineError::SourceFailed { error, .. } => (source_kind(error).to_owned(), Some(error)),
        EngineError::CopyNotUndone { error, .. } => cause_of(error),
    }
}

/// A source error's kind, as its own wire tag spells it.
///
/// Exhaustive, for the reason [`cause_of`] is; `writeback::tests` holds each word to the tag
/// the store's own serialiser writes.
fn source_kind(error: &SourceError) -> &'static str {
    match error {
        SourceError::Config { .. } => "config",
        SourceError::Auth { .. } => "auth",
        SourceError::Refused { .. } => "refused",
        SourceError::RateLimited { .. } => "rate-limited",
        SourceError::Unavailable { .. } => "unavailable",
        SourceError::Malformed { .. } => "malformed",
    }
}

struct Failed {
    reason: String,
    classified: Option<Classified>,
    /// The copy report's `delivered` entries, where a copy that landed in part wrote one.
    delivered: Vec<Map<String, Value>>,
    /// How long the store asked to be left alone: the `retry_after_seconds` a rate-limited
    /// failure carried, read off the store's own typed value. `None` where it gave none, and
    /// then the retry schedule decides.
    wait: Option<Duration>,
}

/// How long one source error asked the caller to wait before asking again: the rate limit's
/// own `retry_after_seconds`, where it gave one, and nothing for every other failure.
fn asked_to_wait(error: &SourceError) -> Option<Duration> {
    match error {
        SourceError::RateLimited {
            retry_after_seconds,
            ..
        } => retry_after_seconds.map(Duration::from_secs),
        SourceError::Config { .. }
        | SourceError::Auth { .. }
        | SourceError::Refused { .. }
        | SourceError::Unavailable { .. }
        | SourceError::Malformed { .. } => None,
    }
}

impl Failed {
    /// A failure the store classed.
    fn classed(reason: String, classified: Classified) -> Self {
        Self {
            reason,
            classified: Some(classified),
            delivered: Vec::new(),
            wait: None,
        }
    }

    /// One store call the engine refused to run, with the wait its source asked for.
    fn engine(call: &str, error: &EngineError) -> Self {
        Self {
            wait: cause_of(error).1.and_then(asked_to_wait),
            ..Self::classed(
                format!("{call} failed: {error}"),
                Classified::of_engine(error),
            )
        }
    }

    /// One store call answered in part, naming every source that could not contribute.
    // llmlint: ignore[changed_behavior_has_e2e] every read that reaches this is addressed to one source — `shown`'s `show` of a qualified id, and `destination_origins`' page under a `Qualified` selector once that project's own `show` has answered — so a write-back answer carries at most one source's failure and no journey can mix a refused source with a transient one here. Each single-source class is driven end to end (`store::a_refusal_of_the_member_read_the_copy_or_the_project_read_stops_the_retry_timer`, `store::a_failure_the_store_does_not_refuse_is_retried_on_the_schedule`), the every-or-transient rule `Classified::of_all` applies is driven mixed by `delivers::a_partial_copy_mixing_refused_and_transient_tickets_is_classed_transient`, and the mixed source case is held by `the_divergence_records_refusal_rule_is_the_one_the_worker_classifies_by`.
    fn partial(call: &str, errors: &[SourceFailure]) -> Self {
        let named: Vec<String> = errors
            .iter()
            .map(|failure| {
                format!(
                    "source {} could not answer: {}",
                    failure.source, failure.error
                )
            })
            .collect();
        Self {
            reason: format!("{call} answered in part: {}", named.join("; ")),
            classified: Classified::of_all(
                errors
                    .iter()
                    .map(|failure| Classified::of_source(&failure.error)),
            ),
            delivered: Vec::new(),
            // The longest any of them asked for: asking sooner asks one that said not yet.
            wait: errors
                .iter()
                .filter_map(|failure| asked_to_wait(&failure.error))
                .max(),
        }
    }

    fn refused(&self) -> bool {
        self.classified.as_ref().is_some_and(Classified::refused)
    }

    /// The reason, with the store's class and kind beside it where it gave them.
    ///
    /// A classified reason is the store's own words, which are several lines ending in its
    /// `next:` — so it is closed up onto one, or the class, the kind and what the worker does
    /// next would land on a line nobody scanning the log for the failure reads. An
    /// unclassified reason is carried exactly as it always was.
    fn said(&self) -> String {
        match &self.classified {
            Some(classified) => format!(
                "{} ({})",
                self.reason.split_whitespace().collect::<Vec<_>>().join(" "),
                classified.said()
            ),
            None => self.reason.clone(),
        }
    }
}

/// A failure the store said nothing about: this worker's own, or a call that never
/// answered inside its deadline.
impl From<String> for Failed {
    fn from(reason: String) -> Self {
        Self {
            reason,
            classified: None,
            delivered: Vec::new(),
            wait: None,
        }
    }
}

#[derive(Default, PartialEq, Eq)]
enum WorkerState {
    #[default]
    Idle,
    Working,
}

/// Where the run holding this worker has got to.
///
/// One value rather than a flag beside the worker's own, because these are phases of one
/// run and it only ever moves forward through them: what a worker between snapshots and a
/// worker waiting out a retry interval each do next is decided by the same value.
#[derive(Default, PartialEq, Eq)]
enum RunPhase {
    #[default]
    Running,
    /// The graph has converged and the run is inside its bounded closeout window, which
    /// suspends the retry schedule: a terminal snapshot left sitting out a minute's
    /// interval inside a two-second window would never be projected at all.
    ClosingOut,
    Stopping,
}

#[derive(Default)]
struct Pending {
    latest: Option<Snapshot>,
    last_success: Option<Snapshot>,
    /// The snapshot the store last refused, until the worker takes another.
    ///
    /// Remembered so that publishing it again attempts nothing: the store would refuse the
    /// same projection the same way, and only a graph that changed is worth asking about.
    refused: Option<Snapshot>,
    /// How many attempts this worker has ended, landed or not: what a driver's launch wait
    /// ends on.
    attempts: u64,
    /// How many nodes the snapshot most recently queued carries, kept when the worker takes it:
    /// what a driver's launch wait is bounded by, however soon the worker picks that snapshot up.
    queued_items: usize,
    worker: WorkerState,
    phase: RunPhase,
    /// Projections that failed and have not yet been raised with the planner.
    ///
    /// One entry per outage rather than one per retry: the worker retries until
    /// the store returns, and a surface for every attempt would bury the first.
    unprojected: Vec<Unprojected>,
}

impl Pending {
    fn queue(&mut self, snapshot: Snapshot) -> bool {
        if self.latest.as_ref() == Some(&snapshot) {
            return false;
        }
        if self.worker == WorkerState::Idle
            && self.latest.is_none()
            && (self.last_success.as_ref() == Some(&snapshot)
                || self.refused.as_ref() == Some(&snapshot))
        {
            return false;
        }
        self.queued_items = snapshot.lineages().len();
        self.latest = Some(snapshot);
        true
    }
}

/// A non-blocking handle owned by the one reconcile loop.
pub struct Writeback {
    pending: Arc<(Mutex<Pending>, Condvar)>,
    /// The copy's per-item budget, which bounds the launch wait as it bounds the copy.
    per_item: NonZeroU64,
}

impl Writeback {
    /// Start the worker for one driver of a run.
    ///
    /// Its store is the one the launch record's directory configures — discovered there,
    /// exactly as the launch's own plan read discovered it from the directory it ran in —
    /// and it is built afresh for every attempt, so nothing here reads the store yet.
    pub fn start(paths: &RunPaths, launch: &LaunchRecord) -> Option<Self> {
        let pending = Arc::new((Mutex::new(Pending::default()), Condvar::new()));
        let worker_pending = Arc::clone(&pending);
        let run_dir = paths.dir.clone();
        let store = Store::at(launch_dir(launch));
        let per_item = per_item_budget(launch);
        // llmlint: ignore-block[changed_behavior_has_e2e] A host refusing one thread while
        // continuing to run this process is resource exhaustion no real CLI journey can
        // arrange at this boundary. Every reachable worker failure is covered against the
        // real sibling and real store; this compatibility edge deliberately disables only
        // the projection and leaves the run unchanged.
        std::thread::Builder::new()
            .name(format!("writeback-{}", paths.run))
            .spawn(move || worker(store, run_dir, per_item, worker_pending))
            .ok()?;
        // llmlint: ignore-end[changed_behavior_has_e2e]
        let writer = Self { pending, per_item };
        // The project is retained in each snapshot rather than in the worker so a malformed
        // old launch record disables projection without weakening LaunchRecord's compatibility.
        Some(writer)
    }

    /// Replace any queued projection with the newest journal fold.
    ///
    /// Called once per change to what the snapshot is made of rather than once
    /// per reconcile pass: building one folds the run's whole journal twice, and
    /// the loop used to hand over two identical snapshots forty times a second on
    /// a run where nothing was happening. The statuses are handed in for the same
    /// reason — the caller has already derived them.
    pub fn publish(
        &self,
        paths: &RunPaths,
        launch: &LaunchRecord,
        state: &RunState,
        statuses: &BTreeMap<String, NodeStatus>,
    ) {
        if let Some(snapshot) = snapshot_of(paths, launch, state, statuses, Claim::Held) {
            self.queue(snapshot);
        }
    }

    /// The same, for the snapshot a closeout projects: a node that never started is written
    /// `todo`, releasing the claim on every ticket it delivers.
    pub fn publish_closeout(
        &self,
        paths: &RunPaths,
        launch: &LaunchRecord,
        state: &RunState,
        statuses: &BTreeMap<String, NodeStatus>,
    ) {
        if let Some(snapshot) = snapshot_of(paths, launch, state, statuses, Claim::Released) {
            self.queue(snapshot);
        }
    }

    /// Wait for this worker to end one attempt, bounded by the store call deadline for a
    /// whole copy of what is queued.
    ///
    /// What a driver asks before its first dispatch, so the run's claim — and the store's
    /// propagation of it to every delivered ticket — reaches the board before any of the work
    /// it claims starts. An attempt that fails or outlasts the bound ends the wait all the
    /// same: the dispatch goes ahead, and the failure reaches the planner as every failed
    /// projection does.
    pub fn wait_for_first_attempt(&self) {
        let (lock, ready) = &*self.pending;
        let Ok(mut pending) = lock.lock() else { return };
        let deadline = Instant::now() + launch_wait(self.per_item, &pending);
        while pending.attempts == 0 && Instant::now() < deadline {
            let wait = deadline.saturating_duration_since(Instant::now());
            let Ok((next, _)) = ready.wait_timeout(pending, wait) else {
                return;
            };
            pending = next;
        }
    }

    fn queue(&self, snapshot: Snapshot) {
        crate::loopstats::published();
        let (lock, ready) = &*self.pending;
        if let Ok(mut pending) = lock.lock() {
            if pending.queue(snapshot) {
                ready.notify_one();
            }
        }
    }
}

/// The snapshot one publish hands the worker, or `None` for a launch naming no project.
fn snapshot_of(
    paths: &RunPaths,
    launch: &LaunchRecord,
    state: &RunState,
    statuses: &BTreeMap<String, NodeStatus>,
    claim: Claim,
) -> Option<Snapshot> {
    let Ok(project) = launch.project.parse() else {
        return None;
    };
    Some(Snapshot {
        project,
        dir: paths.dir.join("writeback"),
        nodes: all_nodes(paths, state),
        statuses: statuses.clone(),
        outcomes: state.outcomes.clone(),
        landings: state.landings.clone(),
        landing_commits: state.landing_commits.clone(),
        change_urls: state.change_urls.clone(),
        stated_landings: state.stated_landings.clone(),
        settlements: settlements(paths),
        superseded: state.superseded.clone(),
        project_metadata: state
            .plan
            .as_ref()
            .map(|plan| {
                let mut metadata = BTreeMap::from([
                    (
                        "onepipeline.schema_version".into(),
                        json!(plan.schema_version),
                    ),
                    ("onepipeline.concurrency".into(), json!(plan.concurrency)),
                ]);
                if let Some(goal) = &plan.goal {
                    metadata.insert("onepipeline.goal".into(), json!(goal));
                }
                if let Some(name) = &plan.name {
                    metadata.insert("onepipeline.name".into(), json!(name));
                }
                metadata
            })
            .unwrap_or_default(),
        claim,
    })
}

impl Writeback {
    /// Whether the worker has a failed projection the planner has not been told
    /// about.
    ///
    /// What the reconcile loop waits on. The worker runs on a thread of its own,
    /// so a projection fails without anything about this run changing — and a
    /// loop that only woke for its own state would leave a board reported behind
    /// until something else happened to it.
    pub fn has_unprojected(&self) -> bool {
        let (lock, _) = &*self.pending;
        lock.lock()
            .is_ok_and(|pending| !pending.unprojected.is_empty())
    }

    /// Take the projections that failed since this was last asked, clearing them.
    ///
    /// Draining rather than reading: each failure is the planner's to hear once,
    /// and the caller raises it. A worker that recovers does not withdraw one —
    /// what it says is that the board was behind, which stays true.
    pub fn take_unprojected(&self) -> Vec<Unprojected> {
        let (lock, _) = &*self.pending;
        lock.lock()
            .map(|mut pending| std::mem::take(&mut pending.unprojected))
            .unwrap_or_default()
    }

    /// Give the active worker a bounded closeout window for the terminal snapshot.
    pub fn wait_briefly(&self) {
        // Let one already-running real copy reach its own deadline before the process
        // exits. This keeps a completed run from racing a person's next store call,
        // while the hard call limit preserves write-back's latency boundary.
        let deadline = Instant::now() + CLOSEOUT_WAIT;
        let (lock, ready) = &*self.pending;
        let Ok(mut pending) = lock.lock() else { return };
        pending.phase = RunPhase::ClosingOut;
        ready.notify_all();
        while (pending.latest.is_some() || pending.worker == WorkerState::Working)
            && Instant::now() < deadline
        {
            let wait = deadline.saturating_duration_since(Instant::now());
            let Ok((next, _)) = ready.wait_timeout(pending, wait) else {
                return;
            };
            pending = next;
        }
    }

    /// The run is being driven again after a close-out, so the retry schedule the
    /// close-out suspends goes back on.
    ///
    /// A driver that finds a queued edit on its way out applies it and goes on
    /// driving the run, which puts it back in the phase where a failed projection
    /// waits out its growing interval rather than being retried inside a bounded
    /// window that has ended.
    pub fn driving_again(&self) {
        let (lock, ready) = &*self.pending;
        if let Ok(mut pending) = lock.lock() {
            pending.phase = RunPhase::Running;
            ready.notify_all();
        }
    }
}

impl Drop for Writeback {
    fn drop(&mut self) {
        let (lock, ready) = &*self.pending;
        if let Ok(mut pending) = lock.lock() {
            pending.phase = RunPhase::Stopping;
            // Every waiter, because two of them wait on this one condition: the worker
            // idle between snapshots, and the worker waiting out a retry interval that
            // grows into the tens of seconds. Waking one of the two would leave whichever
            // it was not sitting on a stop it has already been told about.
            ready.notify_all();
        }
        // Deliberately no join: a store call is outside the run's failure and latency
        // boundary, and waiting for it here would turn write-back into run settlement.
    }
}

/// How long the copy is allowed per item, as this run's launch chose.
///
/// Off the launch record rather than this process's environment, so a driver a fresh
/// `adopt` starts bounds its copies as the launch resolved them; a record written before
/// the field existed names none, and runs under the shipped default.
fn per_item_budget(launch: &LaunchRecord) -> NonZeroU64 {
    launch
        .item_budget()
        .unwrap_or(DEFAULT_WRITEBACK_ITEM_BUDGET_SECONDS)
}

/// The directory the run's store is configured from: the launch record's own, which is
/// where `onepipeline start` read its plan, and this process's where a record written before
/// the field existed names none.
fn launch_dir(launch: &LaunchRecord) -> PathBuf {
    if launch.dir.as_os_str().is_empty() {
        PathBuf::from(".")
    } else {
        launch.dir.clone()
    }
}

/// How long a driver waits for its first projection before it dispatches: the copy deadline of
/// the snapshot it queued for that projection.
///
/// Counted off what was queued rather than off the queue itself, because the worker may already
/// have taken that snapshot to project it by the time the driver asks. Read off the queue then,
/// the count would be zero and the wait the floor, dispatching inside the deadline a large plan's
/// copy still runs under.
fn launch_wait(per_item: NonZeroU64, pending: &Pending) -> Duration {
    Deadline::Copy {
        per_item,
        items: pending.queued_items,
    }
    .within()
}

/// Release what a run a `stop` ended had claimed and never started: one projection, in the
/// stopping process, carrying exactly the unstarted nodes the landed baseline says are
/// `queued` and writing each of them `todo`.
///
/// A stopped driver takes no closeout of its own — the stop's signal ends it where it stands —
/// so this is the closeout's release, made by the verb that ended it. Best effort like every
/// projection: bounded by the same command deadlines, appended to the same record, and a
/// failure is said on this process's standard error and changes nothing about the stop. A
/// driver that adopts the run claims those nodes again before its first dispatch.
pub(crate) fn release_stopped(paths: &RunPaths, launch: &LaunchRecord) {
    // A launch naming no project has nothing on any store to release.
    if launch.project.parse::<QualifiedId>().is_err() {
        return;
    }
    let store = Store::at(launch_dir(launch));
    let state = crate::checkpoint::Projected::open(paths);
    let statuses = state.statuses();
    let Some(snapshot) = snapshot_of(paths, launch, &state, &statuses, Claim::Released) else {
        return;
    };
    let mut baseline = Baseline::load(&paths.dir, &snapshot.project);
    let at = crate::sys::now_rfc3339();
    let started = Instant::now();
    // llmlint: ignore-block[changed_behavior_has_e2e] a stop whose release outlasts its deadline
    // takes exactly the lines below that a refused release takes: the deadline cancels the copy
    // and answers `Err`, and the attempt is recorded and said on stderr as any failure is. Those
    // lines are driven end to end by
    // `delivers::a_stop_whose_release_the_store_refuses_still_stops_and_says_so`, which asserts
    // the stop's answer, its stderr and the failed record line. The one timeout-specific branch
    // is the deadline's cancellation, driven by
    // `writeback_budget::a_copy_held_past_a_tiny_budget_is_cancelled_and_the_refusal_names_the_arithmetic`
    // and `delivers::a_first_projection_held_past_its_deadline_does_not_hold_back_the_first_dispatch`.
    // A journey holding a `stop` past the sixty-second floor would spend that minute on no line
    // those three do not already reach. A stop that lands inside a rate limit's wait the driver
    // was serving makes its one release attempt all the same: the stopping process cannot see
    // the driver's wait, which lives in the driver's memory, and neither record this run keeps
    // — the landed baseline (entry 93) and the projection record (entry 73), both fixed shapes —
    // has a field to carry it across, so there is no branch here a journey could hold.
    let attempted = project(
        &store,
        per_item_budget(launch),
        &snapshot,
        &mut baseline,
        Scope::Release,
    );
    append_record(
        &paths.dir,
        &ProjectionRecord::of(at, &snapshot.project, started.elapsed(), &attempted),
    );
    if let Err(failed) = attempted.result {
        eprintln!(
            "onetaskgraph write-back could not release the nodes this stopped run never started \
             for '{}': {}",
            snapshot.project,
            failed.said()
        );
    }
    // llmlint: ignore-end[changed_behavior_has_e2e]
}

/// Where the worker's attempts stand, which decides what the next outcome prints.
///
/// One value, because these are mutually exclusive: a projection is landing, or it is part-way
/// through a streak of failures the schedule retries, or the store refused its last attempt.
/// A landing after either of the other two is a recovery, and a retried failure after a
/// refusal starts a streak of its own.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Standing {
    Landing,
    /// The consecutive failures of the retried streak in progress, the first being 1.
    Failing(NonZeroU32),
    Refused,
}

fn worker(
    store: Store,
    run_dir: PathBuf,
    per_item: NonZeroU64,
    pending: Arc<(Mutex<Pending>, Condvar)>,
) {
    let mut standing = Standing::Landing;
    // What this run has already put on the board, which every attempt carries the difference
    // from — read once, off the file the launch seeded or the driver before this one left.
    let mut baseline: Option<Baseline> = None;
    loop {
        let snapshot = {
            let (lock, ready) = &*pending;
            let mut state = match lock.lock() {
                Ok(state) => state,
                Err(_) => return,
            };
            while state.latest.is_none() && state.phase != RunPhase::Stopping {
                state = match ready.wait(state) {
                    Ok(state) => state,
                    Err(_) => return,
                };
            }
            if state.phase == RunPhase::Stopping && state.latest.is_none() {
                return;
            }
            state.worker = WorkerState::Working;
            state.refused = None;
            state
                .latest
                .take()
                .expect("the worker was woken by a snapshot")
        };
        let baseline = baseline.get_or_insert_with(|| Baseline::load(&run_dir, &snapshot.project));
        let at = crate::sys::now_rfc3339();
        let started = Instant::now();
        let attempted = project(&store, per_item, &snapshot, baseline, Scope::Driven);
        append_record(
            &run_dir,
            &ProjectionRecord::of(at, &snapshot.project, started.elapsed(), &attempted),
        );
        let Attempted {
            items,
            result: attempt,
            ..
        } = attempted;
        {
            let (lock, ready) = &*pending;
            if let Ok(mut state) = lock.lock() {
                state.attempts = state.attempts.saturating_add(1);
                ready.notify_all();
            }
        }
        match attempt {
            Ok(_) => {
                if standing != Standing::Landing {
                    eprintln!(
                        "onetaskgraph write-back recovered for '{}'",
                        snapshot.project
                    );
                }
                standing = Standing::Landing;
                let (lock, _) = &*pending;
                if let Ok(mut state) = lock.lock() {
                    state.last_success = Some(snapshot.clone());
                    if state.phase == RunPhase::Stopping && state.latest.is_none() {
                        return;
                    }
                }
            }
            Err(failed) if failed.refused() => {
                // Reported on every refused attempt, which is at most once per graph the run
                // publishes: nothing here asks again on a timer, so there is no streak of
                // retries for a line per attempt to bury. The line says so, because whether
                // to expect another attempt is an operator's next question.
                eprintln!(
                    "onetaskgraph write-back failed for '{}': {}; the store refused it, so it \
                     is not attempted again on a timer — the projection will be attempted again \
                     when the run's graph next changes",
                    snapshot.project,
                    failed.said()
                );
                standing = Standing::Refused;
                let (lock, _) = &*pending;
                if let Ok(mut state) = lock.lock() {
                    state.unprojected.push(Unprojected {
                        project: snapshot.project.clone(),
                        items,
                        reason: failed.reason,
                        classified: failed.classified,
                    });
                    // The same graph published while it was being refused is the same refusal.
                    if state.latest.as_ref() == Some(&snapshot) {
                        state.latest = None;
                    }
                    state.refused = Some(snapshot);
                    if state.phase == RunPhase::Stopping && state.latest.is_none() {
                        return;
                    }
                }
            }
            Err(failed) => {
                let failures = match standing {
                    Standing::Failing(failures) => failures.saturating_add(1),
                    Standing::Landing | Standing::Refused => NonZeroU32::MIN,
                };
                let first = failures == NonZeroU32::MIN;
                standing = Standing::Failing(failures);
                // A store that said how long to leave it alone is left alone that long: no
                // call of any kind reaches it before then — not on the schedule, not for a
                // snapshot published meanwhile, and not at closeout. One that said nothing is
                // asked again on the schedule.
                let asked = failed.wait;
                if first {
                    // The one line on the driver's stderr that ever says a projection is in
                    // trouble, so it says what an operator's next question is: whether to
                    // expect another attempt in a moment or in a minute.
                    match asked {
                        Some(wait) => eprintln!(
                            "onetaskgraph write-back failed for '{}': {}; the store asked to \
                             be left alone for {} seconds, so nothing is asked of it before \
                             then, and further attempts are spaced out to {} seconds apart \
                             while it keeps failing",
                            snapshot.project,
                            failed.said(),
                            wait.as_secs(),
                            RETRY_CEILING.as_secs()
                        ),
                        None => eprintln!(
                            "onetaskgraph write-back failed for '{}': {}; retrying, spacing \
                             further attempts out to {} seconds apart while it keeps failing",
                            snapshot.project,
                            failed.said(),
                            RETRY_CEILING.as_secs()
                        ),
                    }
                }
                // How soon the planner hears of it, which is this sum and no deadline: the
                // surface is recorded one [`FIRST_RETRY_AFTER`] after the call that failed, and
                // the reconcile loop asks for it every `engine::CHANNEL_POLL`. A store that is
                // not there fails the first call outright — `onetaskgraph` cannot resolve a
                // root that is gone, and answers so in milliseconds — so [`COMMAND_FLOOR`] is
                // no part of it: that is spent only by a call that has not returned, which is a
                // store answering slowly.
                let interval = match asked {
                    Some(wait) => Interval::Asked(wait),
                    None => Interval::Scheduled(retry_after(failures.get())),
                };
                if !should_retry_after(&pending, interval) {
                    return;
                }
                let (lock, ready) = &*pending;
                let Ok(mut state) = lock.lock() else { return };
                if first {
                    state.unprojected.push(Unprojected {
                        project: snapshot.project.clone(),
                        items,
                        reason: failed.reason,
                        classified: failed.classified,
                    });
                }
                if state.phase == RunPhase::Stopping {
                    return;
                }
                if state.latest.is_none() {
                    state.latest = Some(snapshot);
                    ready.notify_one();
                }
            }
        }
        let (lock, ready) = &*pending;
        if let Ok(mut state) = lock.lock() {
            state.worker = WorkerState::Idle;
            ready.notify_all();
        }
    }
}

/// How long to wait before retrying the `failures`-th consecutive failure of a streak.
///
/// `failures` counts the failures of the streak in progress, the first being 1, so the
/// interval is [`FIRST_RETRY_AFTER`] after an isolated failure however long an earlier outage
/// lasted, and never longer than [`RETRY_CEILING`] however long this one does.
fn retry_after(failures: u32) -> Duration {
    FIRST_RETRY_AFTER
        .saturating_mul(RETRY_GROWTH.saturating_pow(failures.saturating_sub(1)))
        .min(RETRY_CEILING)
}

/// How long one failure is waited out before the projection is attempted again.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Interval {
    /// The retry schedule's own, which a closeout suspends.
    Scheduled(Duration),
    /// What the store asked for, which nothing shortens: asking a limiter inside the window
    /// it named is asking one that has already said no.
    Asked(Duration),
}

/// Wait out at most one retry interval, answering whether to attempt again at all.
///
/// `false` is [`RunPhase::Stopping`] and the caller returns on it; `true` is the wait
/// having been served, or — for a scheduled interval alone — [`RunPhase::ClosingOut`]. Both
/// phases are read on entry as well as on every wake, so one the run reached while this
/// worker was projecting is honoured rather than missed. A snapshot published meanwhile
/// deliberately does *not* shorten the wait: what the interval spaces is the destination's
/// refusal, and a run that keeps folding new graph state would otherwise retry as fast as it
/// publishes. An interval the store asked for is not shortened by a closeout either, so a
/// closeout inside it projects nothing and ends when its own bounded window does.
fn should_retry_after(pending: &(Mutex<Pending>, Condvar), interval: Interval) -> bool {
    let (lock, ready) = pending;
    let Ok(mut state) = lock.lock() else {
        return false;
    };
    let (wait, binding) = match interval {
        Interval::Scheduled(wait) => (wait, false),
        Interval::Asked(wait) => (wait, true),
    };
    // A wait past what the clock can name is waited out until the run stops: a store asking
    // for longer than a host runs is not asked again, rather than the sum overflowing.
    let due = Instant::now().checked_add(wait);
    loop {
        match state.phase {
            RunPhase::Stopping => return false,
            RunPhase::ClosingOut if !binding => return true,
            RunPhase::ClosingOut | RunPhase::Running => {}
        }
        let left = due.map_or(RETRY_CEILING, |due| {
            due.saturating_duration_since(Instant::now())
        });
        if due.is_some() && left.is_zero() {
            return true;
        }
        let Ok((next, _)) = ready.wait_timeout(state, left) else {
            return false;
        };
        state = next;
    }
}

/// One attempt that landed: what the copy said it did and spent — no report at all for an
/// attempt that had nothing to carry, and so asked the store nothing.
struct Landed {
    actions: Option<ProjectionActions>,
    spent: Option<Map<String, Value>>,
    delivered: Vec<Map<String, Value>>,
}

/// One attempt, landed or not: the lineages it carried, the store calls it made, and how it
/// ended — everything its record line says.
struct Attempted {
    /// The lineage roots the copy carried, which the record and a surface name.
    // llmlint: ignore[invalid_states_unrepresentable] node ids, the plain `String` the
    // snapshot's own maps key by, for the reason `Snapshot::superseded` records.
    items: Vec<String>,
    /// How many times each store operation was called, the one that failed included.
    calls: BTreeMap<StoreCall, u64>,
    result: Result<Landed, Failed>,
}

/// One attempt's store: the engine its configuration describes, with the shadow source
/// declared beside the operator's own, the runtime its calls are driven on, and the count of
/// the calls made through it. Built for the attempt and dropped with it.
struct Attempt {
    runtime: tokio::runtime::Runtime,
    engine: Engine,
    calls: std::cell::RefCell<BTreeMap<StoreCall, u64>>,
}

impl Attempt {
    /// The store the launch directory configures, plus the shadow source this snapshot is
    /// written into — declared as a layer above every other, which is what the store's
    /// `--set` was.
    fn open(store: &Store, snapshot: &Snapshot) -> Result<Self, Failed> {
        let setting = |key: &str, value: Value| -> Result<Setting, Failed> {
            Ok(Setting {
                key: SettingPath::parse(key).map_err(|error| {
                    Failed::classed(error.to_string(), Classified::of_config(&error))
                })?,
                value,
                origin: SettingOrigin::Flag {
                    flag: "the write-back's shadow source".to_owned(),
                },
            })
        };
        // A setting is text, so a shadow root that is not is refused by name rather than
        // handed over lossily — which would point the store at a folder nobody wrote.
        let root = snapshot.dir.to_str().ok_or_else(|| {
            format!(
                "the shadow store {} is not a UTF-8 path, so no store setting can name it",
                snapshot.dir.display()
            )
        })?;
        let flags = Layer::new(vec![
            setting(
                &format!("sources.{SHADOW_SOURCE}.plugin"),
                json!(onetaskgraph_core::PluginKind::LocalMd.as_str()),
            )?,
            setting(&format!("sources.{SHADOW_SOURCE}.config.root"), json!(root))?,
        ]);
        // The shadow root exists before the source over it is built, so the store never
        // reads it as a folder that is not there.
        std::fs::create_dir_all(&snapshot.dir)
            .map_err(|error| format!("cannot create the shadow store: {error}"))?;
        let built = opened_within(store, flags, Deadline::Floor)?.map_err(|error| {
            Failed::classed(
                format!("the store's configuration cannot be read: {error}"),
                Classified::of_config(&error),
            )
        })?;
        Ok(Self {
            runtime: crate::taskgraph::runtime()
                .map_err(|error| format!("the store cannot be called: {error}"))?,
            engine: built.engine,
            calls: std::cell::RefCell::default(),
        })
    }

    /// Drive one store call to its end, or cancel it at its deadline.
    ///
    /// Cancelled, the call's future is dropped where it waits; the engine that drove it goes
    /// when the attempt does, before the attempt is recorded.
    fn call<T>(
        &self,
        call: StoreCall,
        deadline: Deadline,
        future: impl Future<Output = T>,
    ) -> Result<T, Failed> {
        *self.calls.borrow_mut().entry(call).or_default() += 1;
        self.runtime
            .block_on(async { tokio::time::timeout(deadline.within(), future).await })
            .map_err(|_| Failed::from(deadline.refusal(call.as_str())))
    }

    /// Read one destination task by its own id: the one read an attempt makes of an item.
    fn task(&self, id: &GlobalId) -> Result<Qualified<Task>, Failed> {
        let answer = self
            .call(StoreCall::TaskShow, Deadline::Floor, self.engine.task(id))?
            .map_err(|error| Failed::engine(TASK_SHOW, &error))?;
        shown(TASK_SHOW, id, answer)
    }
}

/// Build the attempt's store — its configuration read, each source constructed and a
/// hosted source's handshake answered — within `deadline`, as every call after it is.
///
/// Building runs no future the deadline could cancel, so it runs on a thread of its own and
/// is waited on for as long as the deadline allows. A build that outlasts it is left to
/// finish on that thread and then dropped with whatever it built: constructing a source
/// writes nothing, so nothing it does after the attempt is recorded reaches the board.
fn opened_within(
    store: &Store,
    flags: Layer,
    deadline: Deadline,
) -> Result<Result<crate::taskgraph::Built, ConfigError>, Failed> {
    let (built, arrived) = std::sync::mpsc::channel();
    let store = store.clone();
    std::thread::Builder::new()
        .name("writeback-store".to_owned())
        .spawn(move || {
            // The receiver is gone once the deadline has passed, and the build goes with it.
            let _ = built.send(store.engine(&flags));
        })
        .map_err(|error| format!("the store cannot be opened: {error}"))?;
    opened_by(&arrived, deadline)
}

/// What the thread building an attempt's store sent, or why nothing arrived: the deadline
/// passed, or the thread ended without an answer, which only a panic in the build does and
/// which is not the store being slow.
fn opened_by<T>(arrived: &std::sync::mpsc::Receiver<T>, deadline: Deadline) -> Result<T, Failed> {
    arrived
        .recv_timeout(deadline.within())
        .map_err(|error| match error {
            std::sync::mpsc::RecvTimeoutError::Timeout => {
                Failed::from(deadline.refusal(STORE_OPEN))
            }
            std::sync::mpsc::RecvTimeoutError::Disconnected => Failed::from(format!(
                "{STORE_OPEN} ended without an answer: building the store stopped short"
            )),
        })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Scope {
    /// A driver's: every lineage whose projection differs from what landed.
    Driven,
    /// A stop's release: only the unstarted lineages the baseline says are `queued`.
    Release,
}

/// Project one snapshot, carrying exactly the lineages whose projection differs from what the
/// baseline says landed, and advance the baseline by every item that landed.
///
/// Nothing is read that the baseline already answers: the destination id of every lineage it
/// holds is its own, so no attempt reads the project's page of tasks. A lineage it does not
/// hold — a run an older build started, whose directory holds no baseline — is read once by
/// its own id and compared against that read. Each lineage the copy carries is read once by
/// its own id, for the labels a person may have put on it since, which the copy restates. The
/// project item is read, because every copy carries it.
fn project(
    store: &Store,
    per_item: NonZeroU64,
    snapshot: &Snapshot,
    baseline: &mut Baseline,
    scope: Scope,
) -> Attempted {
    let mut items = Vec::new();
    let lineages = snapshot.lineages();
    let renderings: BTreeMap<String, Rendering> = lineages
        .roots()
        .filter_map(|root| {
            Rendering::of(snapshot, &lineages, root)
                .ok()
                .map(|rendering| (root.clone(), rendering))
        })
        .collect();
    let decided = decide(snapshot, &lineages, &renderings, baseline, scope);
    // Nothing differs from what landed and nothing is unknown: the board already says all of
    // it, so the store is not opened, let alone asked.
    if decided.carried.is_empty()
        && decided.unread.is_empty()
        && !baseline.project_differs(snapshot)
    {
        return Attempted {
            items,
            calls: BTreeMap::new(),
            result: Ok(Landed {
                actions: None,
                spent: None,
                delivered: Vec::new(),
            }),
        };
    }
    // Named before the store is opened, so an attempt whose store never opens still names what
    // it set out to carry; the reads below settle which of the unknown ones it does.
    items.extend(
        decided
            .carried
            .iter()
            .chain(decided.unread.keys())
            .cloned()
            .collect::<BTreeSet<_>>(),
    );
    let (result, calls) = match Attempt::open(store, snapshot) {
        Err(failed) => (Err(failed), BTreeMap::new()),
        Ok(attempt) => {
            let result = carry_the_difference(
                &attempt,
                per_item,
                snapshot,
                (&renderings, decided),
                baseline,
                &mut items,
            );
            (result, attempt.calls.take())
        }
    };
    Attempted {
        items,
        calls,
        result,
    }
}

fn carry_the_difference(
    attempt: &Attempt,
    per_item: NonZeroU64,
    snapshot: &Snapshot,
    (renderings, decided): (&BTreeMap<String, Rendering>, Decided),
    baseline: &mut Baseline,
    items: &mut Vec<String>,
) -> Result<Landed, Failed> {
    let scope = decided.scope;
    // What the run knows of every lineage the baseline holds: where its item is. The labels are
    // read for the lineages the copy carries, below; an unnamed one is neither read nor written.
    let mut origins = baseline.origins();
    let mut carried = decided.carried;
    let mut read: BTreeSet<String> = BTreeSet::new();
    let mut seeded = false;
    for (root, id) in &decided.unread {
        let task = attempt.task(id)?;
        let depends_on = renderings
            .get(root)
            .map(|now| now.item.depends_on.clone())
            .unwrap_or_default();
        let held = LandedItem::read(&task, depends_on);
        origins.insert(root.clone(), Origin::of(task));
        read.insert(root.clone());
        let differs = match scope {
            Scope::Driven => !renderings
                .get(root)
                .is_some_and(|now| held.says_what(&now.item)),
            Scope::Release => held.status == Some(ProjectedStatus::Queued),
        };
        if differs {
            carried.insert(root.clone());
        } else {
            baseline.landed.items.insert(root.clone(), held);
            seeded = true;
        }
    }
    if seeded {
        baseline.save();
    }
    *items = carried.iter().cloned().collect();
    // Every lineage the run did not know already says what the run would write, and no project
    // key moved: the reads were the whole attempt, and there is nothing to copy.
    if carried.is_empty() && !baseline.project_differs(snapshot) {
        return Ok(Landed {
            actions: None,
            spent: None,
            delivered: Vec::new(),
        });
    }
    let destination_project = destination_project(attempt, snapshot)?;
    for root in &carried {
        if read.contains(root) {
            continue;
        }
        let Some(origin) = origins.get_mut(root) else {
            continue;
        };
        *origin = Origin::of(attempt.task(&origin.id.clone())?);
    }
    // A shadow store this worker cannot write is its own failure, classed by nobody and so
    // retried: `store::an_unwritable_shadow_store_is_reported_retried_and_recovered`.
    write_shadow(snapshot, &origins, &destination_project)?;
    let shadow_project = shadow_id(&project_file(&snapshot.project));
    // A member copy names exactly the lineages that changed. Naming none is not a copy of
    // everything: the project item alone carries what changed at the project's level.
    let scope = match CopyItems::new(
        carried
            .iter()
            .map(|root| member_id(snapshot, root))
            .collect(),
    ) {
        Some(members) => CopyScope::Members(members),
        // llmlint: ignore-block[changed_behavior_has_e2e] a copy naming no lineage is reached
        // only when a project key the destination holds changed and no lineage did — every
        // other attempt carrying nothing asks the store nothing — and no edit a CLI accepts
        // changes only the project's metadata. `writeback::tests` holds the decision that names
        // none; a project copy without its tasks is the store's own scope for copying the
        // project item alone.
        None => CopyScope::Projects { tasks: false },
        // llmlint: ignore-end[changed_behavior_has_e2e]
    };
    let request = CopyRequest {
        items: CopyItems::new(vec![shadow_project]).expect("one item is not none"),
        scope,
        destination: snapshot.project.global().source,
        match_by: None,
        recreate: false,
        dry_run: false,
    };
    // The one call that is linear in what it carries, so the one whose deadline is.
    let deadline = Deadline::Copy {
        per_item,
        items: carried.len(),
    };
    let report = attempt
        .call(
            StoreCall::ProjectCopy,
            deadline,
            attempt.engine.copy(&request),
        )?
        .map_err(|error| Failed::engine(PROJECT_COPY, &error))?;
    let delivered: Vec<Map<String, Value>> = report.delivered.iter().map(verbatim).collect();
    // Counted off the origins as the pre-copy read left them, before the report teaches the
    // run where anything moved: a reopen is decided by what the item read as before the copy
    // and what the copy wrote onto it.
    let actions = actions(&report, snapshot, &origins);
    learn(&report, &mut origins, snapshot);
    // A copy whose delivered tickets the store could not keep in step landed every item but
    // those deliverers': their tickets are behind the run, so their lineages stay unlanded and
    // the next attempt carries them again — and nothing that did land.
    let behind: std::collections::HashSet<&GlobalId> = report
        .delivered
        .iter()
        .filter_map(|entry| match &entry.outcome {
            DeliveryOutcome::Failed { .. } => Some(&entry.deliverer),
            DeliveryOutcome::Written { .. }
            | DeliveryOutcome::Unchanged { .. }
            | DeliveryOutcome::Left { .. } => None,
        })
        .collect();
    for root in &carried {
        let (Some(origin), Some(now)) = (origins.get(root), renderings.get(root)) else {
            continue;
        };
        if behind.contains(&origin.id) {
            continue;
        }
        baseline
            .landed
            .items
            .insert(root.clone(), now.item.landed_at(&origin.id));
    }
    baseline.landed.project_metadata =
        owned(&projected_project_metadata(snapshot, &destination_project));
    baseline.save();
    if let Some(tickets) = failed_deliveries(&report.delivered) {
        let mut failed = Failed::from(format!(
            "the copy landed, but the store could not keep every delivered ticket in step: \
             {tickets}"
        ));
        failed.classified =
            Classified::of_all(
                report
                    .delivered
                    .iter()
                    .filter_map(|entry| match &entry.outcome {
                        DeliveryOutcome::Failed { failure, .. } => {
                            Some(Classified::of_delivery(failure))
                        }
                        DeliveryOutcome::Written { .. }
                        | DeliveryOutcome::Unchanged { .. }
                        | DeliveryOutcome::Left { .. } => None,
                    }),
            );
        failed.delivered = delivered;
        failed.wait = report
            .delivered
            .iter()
            .filter_map(|entry| match &entry.outcome {
                DeliveryOutcome::Failed { failure, .. } => failure.retry_after_seconds(),
                DeliveryOutcome::Written { .. }
                | DeliveryOutcome::Unchanged { .. }
                | DeliveryOutcome::Left { .. } => None,
            })
            .max()
            .map(Duration::from_secs);
        return Err(failed);
    }
    Ok(Landed {
        actions: Some(actions),
        spent: summed(report.spent.iter()),
        delivered,
    })
}

/// Every call's reported spend, summed into the one object the record carries: requests
/// added, and each budget added to the one of the same name and unit, a lower bound wherever
/// any summand was. `None` where no call reported any.
fn summed<'a>(
    spends: impl Iterator<Item = &'a onetaskgraph_core::Spent>,
) -> Option<Map<String, Value>> {
    let mut total: Option<onetaskgraph_core::Spent> = None;
    for spent in spends {
        let Some(sum) = total.as_mut() else {
            total = Some(spent.clone());
            continue;
        };
        sum.requests = sum.requests.saturating_add(spent.requests);
        for budget in &spent.budgets {
            match sum
                .budgets
                .iter_mut()
                .find(|held| held.budget == budget.budget && held.unit == budget.unit)
            {
                Some(held) => {
                    held.amount = held.amount.saturating_add(budget.amount);
                    held.lower_bound |= budget.lower_bound;
                }
                None => sum.budgets.push(budget.clone()),
            }
        }
        sum.budgets
            .sort_by(|left, right| (&left.budget, &left.unit).cmp(&(&right.budget, &right.unit)));
    }
    total.as_ref().map(verbatim)
}

/// Which lineages one attempt carries, decided off the baseline before anything is read.
struct Decided {
    /// What the decision was made for.
    scope: Scope,
    /// Roots whose projection differs from what the baseline says landed: the copy's members.
    carried: BTreeSet<String>,
    /// Roots the baseline does not hold whose item the run knows an id for: each read once by
    /// that id, and carried only where the read differs.
    unread: BTreeMap<String, GlobalId>,
}

/// Decide what one attempt carries.
///
/// A lineage the baseline holds is carried when its projection, rendered from the snapshot
/// alone, differs from what the baseline says landed. The one field a copy writes that the
/// baseline does not keep is a node's GitHub repository, and no edit changes that alone: a
/// `retry` moves the lineage's head, so `onepipeline.node` differs, and a `requeue` moves its
/// word — so a lineage whose repository moved differs from the baseline in a field it does
/// keep, whichever driver asks. A lineage the baseline does not hold is read by the id the run
/// knows for it, or, where it knows none, carried: the copy creates it. A stop's release
/// carries only the unstarted lineages the baseline says are `queued`, and creates nothing.
fn decide(
    snapshot: &Snapshot,
    lineages: &Lineages,
    renderings: &BTreeMap<String, Rendering>,
    baseline: &Baseline,
    scope: Scope,
) -> Decided {
    let mut decided = Decided {
        scope,
        carried: BTreeSet::new(),
        unread: BTreeMap::new(),
    };
    for root in lineages.roots() {
        let held = baseline.landed.items.get(root);
        let Some(now) = renderings.get(root) else {
            // A lineage that does not render is carried, so the attempt fails saying why.
            if scope == Scope::Driven {
                decided.carried.insert(root.clone());
            }
            continue;
        };
        match scope {
            Scope::Driven => match held {
                Some(held) => {
                    if !held.says_what(&now.item) {
                        decided.carried.insert(root.clone());
                    }
                }
                None => match known_id(snapshot, lineages, root) {
                    Some(id) => {
                        decided.unread.insert(root.clone(), id);
                    }
                    None => {
                        decided.carried.insert(root.clone());
                    }
                },
            },
            Scope::Release => {
                if now.item.status != Some(ProjectedStatus::Todo) {
                    continue;
                }
                match held {
                    Some(held) => {
                        if held.status == Some(ProjectedStatus::Queued) {
                            decided.carried.insert(root.clone());
                        }
                    }
                    None => {
                        if let Some(id) = known_id(snapshot, lineages, root) {
                            decided.unread.insert(root.clone(), id);
                        }
                    }
                }
            }
        }
    }
    decided
}

/// The destination id the run knows for a lineage the baseline does not hold, furthest along
/// first: the origin the previous driver's shadow document recorded for the head or any
/// attempt before it — which is where a board an older build wrote keeps one item per attempt
/// — and otherwise the task the root was read out of at launch.
fn known_id(snapshot: &Snapshot, lineages: &Lineages, root: &str) -> Option<GlobalId> {
    let destination = snapshot.project.global().source;
    let folder = snapshot
        .dir
        .join("tasks")
        .join(project_file(&snapshot.project));
    let recorded = lineages.chain(root).and_then(|chain| {
        chain.iter().rev().find_map(|node| {
            shadow_origin(&folder.join(format!("{}.md", task_file(node))))
                .filter(|id| id.source == destination)
        })
    });
    recorded.or_else(|| {
        snapshot
            .nodes
            .get(root)
            .and_then(|node| node.task_record.as_ref())
            .map(|record| GlobalId::new(destination, NativeId::from(record.id.as_str())))
    })
}

/// The destination item a shadow document names as its origin: `None` where there is no such
/// document or it names none, and — said on standard error, naming the document and why —
/// where it cannot be read as one, so the id the run falls back to is never chosen silently.
fn shadow_origin(path: &Path) -> Option<GlobalId> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return None,
        // llmlint: ignore-block[changed_behavior_has_e2e] a shadow document that is there and
        // cannot be read is a run directory whose permissions were taken away underneath the
        // run, which no CLI journey arranges; it is said and passed over exactly as a malformed
        // one is below.
        Err(error) => {
            eprintln!(
                "onetaskgraph write-back cannot read {}: {error}; the item it names is not \
                 taken as the lineage's",
                path.display()
            );
            return None;
        } // llmlint: ignore-end[changed_behavior_has_e2e]
    };
    let origin = text
        .strip_prefix("---\n")
        .and_then(|rest| rest.split_once("\n---\n"))
        .ok_or_else(|| "it opens and closes no front matter".to_owned())
        .and_then(|(front, _)| {
            serde_norway::from_str::<Value>(front).map_err(|error| error.to_string())
        })
        .and_then(|front| match &front["metadata"][GlobalId::ORIGIN_KEY] {
            Value::Null => Ok(None),
            Value::String(origin) => origin
                .parse::<GlobalId>()
                .map(Some)
                .map_err(|why| format!("its origin '{origin}' {why}")),
            other => Err(format!("its origin is {other}, not an id")),
        });
    origin.unwrap_or_else(|why| {
        eprintln!(
            "onetaskgraph write-back cannot read {} as a shadow document: {why}; the item it \
             names is not taken as the lineage's",
            path.display()
        );
        None
    })
}

/// One lineage as the snapshot alone renders it: the half of its shadow task the landed
/// baseline keeps.
struct Rendering {
    /// What the baseline would record were this to land, its destination not yet known.
    item: LandedItem,
}

impl Rendering {
    fn of(snapshot: &Snapshot, lineages: &Lineages, root: &str) -> Result<Self, String> {
        let (front, content) = task_document(snapshot, lineages, root, None)?;
        let head = lineages
            .chain(root)
            .and_then(<[String]>::last)
            .ok_or_else(|| format!("no lineage is rooted at '{root}'"))?;
        let node = snapshot
            .nodes
            .get(head)
            .ok_or_else(|| format!("the snapshot holds no node '{head}'"))?;
        let item = LandedItem {
            destination: String::new(),
            title: front["title"].as_str().unwrap_or_default().to_owned(),
            content_sha256: sha256(&content),
            status: Some(snapshot.word_of(head)),
            metadata: owned(
                &front["metadata"]
                    .as_object()
                    .cloned()
                    .unwrap_or_default()
                    .into_iter()
                    .collect(),
            ),
            delivers: node.delivers.clone(),
            depends_on: node
                .deps
                .iter()
                .filter(|dep| !crate::graph::is_cross_dag(dep))
                .map(|dep| lineages.root_of(dep).to_owned())
                .collect(),
        };
        Ok(Self { item })
    }
}

fn sha256(text: &str) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(text.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// The keys of a metadata map this engine owns — `onepipeline.*` — and no other.
fn owned(metadata: &BTreeMap<String, Value>) -> BTreeMap<String, Value> {
    metadata
        .iter()
        .filter(|(key, _)| key.starts_with(OWNED_PREFIX))
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect()
}

const OWNED_PREFIX: &str = "onepipeline.";

/// The project metadata a copy writes: the destination's own, with each engine-owned key it
/// already holds restated from the snapshot.
fn projected_project_metadata(
    snapshot: &Snapshot,
    destination_project: &Project,
) -> BTreeMap<String, Value> {
    let mut metadata: BTreeMap<String, Value> = destination_project
        .metadata
        .iter()
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect();
    for (key, value) in &snapshot.project_metadata {
        if metadata.contains_key(key) {
            metadata.insert(key.clone(), value.clone());
        }
    }
    metadata
}

/// What this run has put on the board: the landed baseline, and the file it is kept in.
struct Baseline {
    path: PathBuf,
    landed: LandedBaseline,
}

impl Baseline {
    /// The run's baseline, or an empty one where its directory holds none — a run an older
    /// build started — or holds one this build cannot read, which is said on standard error:
    /// either way every lineage is then read once by its own id rather than trusted.
    fn load(run_dir: &Path, project: &QualifiedId) -> Self {
        let path = run_dir.join(WRITEBACK_LANDED_FILE);
        let landed = match std::fs::read_to_string(&path) {
            Ok(text) => match serde_json::from_str::<LandedBaseline>(&text)
                .map_err(|error| error.to_string())
                .and_then(|landed| landed.checked(project).map(|()| landed))
            {
                Ok(landed) => landed,
                Err(why) => {
                    eprintln!(
                        "onetaskgraph write-back cannot read {}: {why}; each item it would have \
                         named is read from the store once instead",
                        path.display()
                    );
                    LandedBaseline::empty(project)
                }
            },
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                LandedBaseline::empty(project)
            }
            // llmlint: ignore-block[changed_behavior_has_e2e] a baseline file that is there and
            // cannot be read is a run directory whose permissions were taken away underneath
            // the run, which no CLI journey arranges; it takes the same empty baseline a
            // missing file does, which `writeback_projections` drives end to end.
            Err(error) => {
                eprintln!(
                    "onetaskgraph write-back cannot read {}: {error}; each item it would have \
                     named is read from the store once instead",
                    path.display()
                );
                LandedBaseline::empty(project)
            } // llmlint: ignore-end[changed_behavior_has_e2e]
        };
        Self { path, landed }
    }

    /// Whether the snapshot restates a project key the destination holds with another value
    /// than the one last projected — the one project-level change a copy carries. A key the
    /// destination does not hold is never written, so it is no change.
    fn project_differs(&self, snapshot: &Snapshot) -> bool {
        snapshot.project_metadata.iter().any(|(key, value)| {
            self.landed
                .project_metadata
                .get(key)
                .is_some_and(|held| held != value)
        })
    }

    /// Where the run knows each lineage's item to be: the baseline's own destination ids.
    fn origins(&self) -> BTreeMap<String, Origin> {
        self.landed
            .items
            .iter()
            .filter_map(|(root, item)| {
                item.destination.parse::<GlobalId>().ok().map(|id| {
                    (
                        root.clone(),
                        Origin {
                            id,
                            labels: Vec::new(),
                            category: None,
                        },
                    )
                })
            })
            .collect()
    }

    /// Rewrite the file atomically. Best effort, like the projection it records: a file that
    /// cannot be written is said on standard error, and the next driver reads each item once.
    fn save(&self) {
        // llmlint: ignore-block[changed_behavior_has_e2e] writing a file in the run's own
        // directory fails only on a host whose run directory has been made unwritable, which
        // no CLI journey arranges; every journey reading the baseline drives the write that
        // lands.
        let written = serde_json::to_vec_pretty(&self.landed)
            .map_err(|error| error.to_string())
            .and_then(|bytes| {
                crate::ledger::write_atomic(&self.path, &bytes, crate::ledger::Durability::Record)
                    .map_err(|error| error.to_string())
            });
        if let Err(error) = written {
            eprintln!(
                "onetaskgraph write-back could not record what landed at {}: {error}",
                self.path.display()
            );
        }
        // llmlint: ignore-end[changed_behavior_has_e2e]
    }
}

/// Seed the run's landed baseline from the launch's own read of its project, before the
/// first projection: every task's destination id, title, content, engine-owned metadata,
/// `delivers` and edges, as the store answered them, and no status — the one thing this
/// engine did not write itself.
pub(crate) fn seed_landed(run_dir: &Path, project: &QualifiedId, read: &crate::taskgraph::Read) {
    let mut landed = LandedBaseline::empty(project);
    landed.project_metadata = owned(&read.project_metadata);
    for node in &read.plan.tasks {
        let Some(task) = read.tasks.get(&node.id) else {
            continue;
        };
        let depends_on = node
            .deps
            .iter()
            .filter(|dep| !crate::graph::is_cross_dag(dep))
            .cloned()
            .collect();
        let mut item = LandedItem::read(task, depends_on);
        item.status = None;
        landed.items.insert(node.id.clone(), item);
    }
    Baseline {
        path: run_dir.join(WRITEBACK_LANDED_FILE),
        landed,
    }
    .save();
}

/// The write-back's landed baseline, `writeback-landed.json` in the run's directory: what
/// this run last put on the board, per lineage — the engine's own run state, written from its
/// own writes and the launch's own read, and never a cache of the store. It never feeds
/// scheduling. Entry 93 of `docs/contract-divergences.md` states the shape and is the one
/// source for it; only engine-owned `onepipeline.*` keys are kept, and no labels, no foreign
/// metadata and no project title or description.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct LandedBaseline {
    pub schema_version: u32,
    // llmlint: ignore-block[invalid_states_unrepresentable] the stored shape is JSON strings
    // by contract — a qualified project id, and node ids keying the items — each checked where
    // the file is read by `LandedBaseline::checked`, which refuses a project other than the
    // run's and an item naming no node.
    pub project: String,
    pub project_metadata: BTreeMap<String, Value>,
    pub items: BTreeMap<String, LandedItem>,
    // llmlint: ignore-end[invalid_states_unrepresentable]
}

/// One lineage as it last landed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct LandedItem {
    // llmlint: ignore-block[invalid_states_unrepresentable] the stored shape is JSON strings by
    // contract; `LandedBaseline::checked` refuses a destination that is not a qualified id and
    // a digest that is not lowercase SHA-256 hex, and node ids are the plain string every
    // identifier in this crate is.
    /// The qualified id of the destination item.
    pub destination: String,
    pub title: String,
    /// The lowercase hex SHA-256 of the content's UTF-8 bytes.
    pub content_sha256: String,
    /// The word last landed, or `None` for an item seeded from a read.
    pub status: Option<ProjectedStatus>,
    /// Engine-owned keys only.
    pub metadata: BTreeMap<String, Value>,
    /// The qualified tickets the item delivers.
    pub delivers: Vec<String>,
    /// The lineage roots the item depends on.
    pub depends_on: Vec<String>,
    // llmlint: ignore-end[invalid_states_unrepresentable]
}

impl LandedBaseline {
    fn empty(project: &QualifiedId) -> Self {
        Self {
            schema_version: WRITEBACK_LANDED_SCHEMA_VERSION,
            project: project.as_str().to_owned(),
            project_metadata: BTreeMap::new(),
            items: BTreeMap::new(),
        }
    }

    /// Refuse a file this build did not write for this run: another version, another project,
    /// a destination that is not a qualified id, a digest that is not one, or a key the engine
    /// does not own.
    // llmlint: ignore-block[changed_behavior_has_e2e] every refusal below is one arm of this one
    // function, and they share one consequence, which is the only behaviour a driver shows: the
    // file is said on standard error with the arm's reason and the baseline is taken as empty,
    // each lineage then read once by its own id. That consequence is driven end to end by
    // `writeback_projections::a_landed_baseline_the_driver_cannot_read_is_said_and_each_item_read_by_its_id`
    // through the foreign-key arm, and
    // `writeback::tests::entry_93s_example_is_the_landed_baseline_the_worker_writes_and_reads`
    // holds every arm's refusal against the entry's own example. A journey per arm would drive
    // the same lines with a different sentence in the log.
    fn checked(&self, project: &QualifiedId) -> Result<(), String> {
        if self.schema_version != WRITEBACK_LANDED_SCHEMA_VERSION {
            return Err(format!(
                "`schema_version` {} is not one this build reads \
                 ({WRITEBACK_LANDED_SCHEMA_VERSION})",
                self.schema_version
            ));
        }
        if self.project != project.as_str() {
            return Err(format!(
                "it records project '{}', and this run projects onto '{project}'",
                self.project
            ));
        }
        let foreign = |metadata: &BTreeMap<String, Value>| {
            metadata
                .keys()
                .find(|key| !key.starts_with(OWNED_PREFIX))
                .cloned()
        };
        if let Some(key) = foreign(&self.project_metadata) {
            return Err(format!(
                "`project_metadata` holds `{key}`, which is not the engine's"
            ));
        }
        for (root, item) in &self.items {
            if root.is_empty() {
                return Err("`items` holds an item under no node id".to_owned());
            }
            let destination = item
                .destination
                .parse::<GlobalId>()
                .map_err(|why| format!("item '{root}': `destination` {why}"))?;
            // llmlint: ignore[boundary_inputs_validated] the destination's source is what an id's
            // spelling decides on every store and is refused here; its project is not — a GitHub
            // issue's id names no project, and a `local-md` one is a file path — so no check of
            // the file alone could hold that line without refusing every hosted board's
            // baseline. The file is this engine's own record of its own writes, written only by
            // `Baseline::save`, and everything its spelling can refuse is refused in this function.
            if destination.source != project.global().source {
                return Err(format!(
                    "item '{root}': `destination` '{}' is not an item of this run's destination \
                     '{}'",
                    item.destination,
                    project.global().source
                ));
            }
            let digest = &item.content_sha256;
            if digest.len() != 64
                || !digest
                    .chars()
                    .all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c))
            {
                return Err(format!(
                    "item '{root}': `content_sha256` is not lowercase SHA-256 hex"
                ));
            }
            if let Some(key) = foreign(&item.metadata) {
                return Err(format!(
                    "item '{root}': `metadata` holds `{key}`, which is not the engine's"
                ));
            }
            for ticket in &item.delivers {
                ticket.parse::<GlobalId>().map_err(|why| {
                    format!("item '{root}': `delivers` names '{ticket}', which {why}")
                })?;
            }
            if item.depends_on.iter().any(String::is_empty) {
                return Err(format!(
                    "item '{root}': `depends_on` names an empty node id"
                ));
            }
        }
        Ok(())
    }
    // llmlint: ignore-end[changed_behavior_has_e2e]
}

impl LandedItem {
    /// One destination task as the store answered it: its content trimmed as the plan reader
    /// trims a body, its engine-owned keys, its tickets qualified in its own source, and the
    /// word it holds where it is one this engine writes. The edges are the caller's, because a
    /// read of one task does not answer them.
    fn read(task: &Qualified<Task>, depends_on: Vec<String>) -> Self {
        Self {
            destination: task.id.to_string(),
            title: task.item.title.clone(),
            content_sha256: sha256(task.item.content.as_deref().unwrap_or_default().trim()),
            // A word this engine never writes — a person's own, or a board's mapped option — is
            // no word it landed, so it reads as not known, which differs from every word a
            // rendering has and so is carried rather than trusted.
            status: serde_json::from_value(json!(task.item.status.name)).ok(),
            metadata: owned(&task.item.metadata.clone().into_iter().collect()),
            delivers: task
                .item
                .delivers
                .iter()
                .map(|entry| entry.in_source(&task.id.source).to_string())
                .collect(),
            depends_on,
        }
    }

    /// Whether this says on the board what `other` would: every field but where it is.
    fn says_what(&self, other: &Self) -> bool {
        self.title == other.title
            && self.content_sha256 == other.content_sha256
            && self.status == other.status
            && self.metadata == other.metadata
            && self.delivers == other.delivers
            && self.depends_on == other.depends_on
    }

    /// This rendering, landed on the item at `destination`.
    fn landed_at(&self, destination: &GlobalId) -> Self {
        Self {
            destination: destination.to_string(),
            ..self.clone()
        }
    }
}

/// One of the store's values, as the object the projection record carries it as: the
/// store's own serialisation, verbatim, so a reader of the record is told what the store
/// said.
fn verbatim(value: &impl Serialize) -> Map<String, Value> {
    match serde_json::to_value(value) {
        Ok(Value::Object(object)) => object,
        // Every value handed here is a struct the store serialises as an object.
        _ => Map::new(),
    }
}

/// Each ticket a copy report says the store failed to keep in step, named with its deliverer
/// and the store's own words about it — or `None` where it failed none.
fn failed_deliveries(delivered: &[Delivered]) -> Option<String> {
    let failed: Vec<String> = delivered
        .iter()
        .filter_map(|entry| match &entry.outcome {
            DeliveryOutcome::Failed { failure, .. } => Some(format!(
                "ticket {} (delivered by {}): {}",
                entry.ticket,
                entry.deliverer,
                failure
                    .message()
                    .split_whitespace()
                    .collect::<Vec<_>>()
                    .join(" ")
            )),
            DeliveryOutcome::Written { .. }
            | DeliveryOutcome::Unchanged { .. }
            | DeliveryOutcome::Left { .. } => None,
        })
        .collect();
    (!failed.is_empty()).then(|| failed.join("; "))
}

/// The one item a `show` of `id` answered with, under the rules its answer is held to.
///
/// Nothing, with no failure beside it, is the item not being there — the store's own reading
/// of an empty `show`, which no retry changes. Nothing because a source failed is that
/// failure, classed by the sources that could not answer.
fn shown<T>(
    call: &str,
    id: &GlobalId,
    answer: QueryResponse<Qualified<T>>,
) -> Result<Qualified<T>, Failed> {
    if !answer.errors.is_empty() {
        return Err(Failed::partial(call, &answer.errors));
    }
    // llmlint: ignore-block[changed_behavior_has_e2e] These refusals defend the linked
    // store's own answer: `Engine::project` and `Engine::task` answer one page holding at most
    // one item, under the id asked for, by construction, so no configuration a journey can
    // write reaches them.
    if answer.next.is_some() {
        return Err(format!("{call} of '{id}' claims another page").into());
    }
    // llmlint: ignore-end[changed_behavior_has_e2e]
    let mut items = answer.items.into_iter();
    let Some(found) = items.next() else {
        return Err(Failed::classed(
            format!("{call} of '{id}' found nothing: it is not in the configured sources"),
            Classified::no_such_item(),
        ));
    };
    // llmlint: ignore-block[changed_behavior_has_e2e] as above.
    if items.next().is_some() || found.id != *id {
        return Err(format!("{call} returned the wrong item for '{id}'").into());
    }
    // llmlint: ignore-end[changed_behavior_has_e2e]
    Ok(found)
}

fn destination_project(attempt: &Attempt, snapshot: &Snapshot) -> Result<Project, Failed> {
    let id = snapshot.project.global();
    let answer = attempt
        .call(
            StoreCall::ProjectShow,
            Deadline::Floor,
            attempt.engine.project(&id),
        )?
        .map_err(|error| Failed::engine(PROJECT_SHOW, &error))?;
    Ok(shown(PROJECT_SHOW, &id, answer)?.item)
}

/// What the destination already holds for one lineage, keyed by the lineage's root.
///
/// The id is what a projection writes back onto; the labels are what it carries forward
/// unchanged, because no plan models them; the category is what the item read as *before*
/// the copy, which is the half of a reopen the copy report cannot say. Each is the store's
/// own type, exactly as the store answered it.
#[derive(Clone)]
struct Origin {
    id: GlobalId,
    labels: Vec<Label>,
    /// The item's normalised status category as the attempt's own pre-copy read reported
    /// it, or `None` where the run learned the item from a copy report rather than a read.
    category: Option<StatusCategory>,
}

impl Origin {
    /// Whether the item read as closed — `done` or `cancelled` — before the copy, so a copy
    /// that rewrote it onto an open word reopened it.
    fn closed(&self) -> bool {
        self.category.is_some_and(|category| {
            matches!(category, StatusCategory::Done | StatusCategory::Cancelled)
        })
    }

    /// What one destination task says about itself.
    fn of(task: Qualified<Task>) -> Self {
        Self {
            id: task.id,
            labels: task.item.labels,
            category: Some(task.item.status.category),
        }
    }
}

/// Build the shadow project a `project copy` then projects onto the destination.
///
/// Every field written here is decided by the module's ownership rule: a plan-declared
/// field is restated from the snapshot, and a field no plan models is carried over from
/// the destination item this was read against.
fn write_shadow(
    snapshot: &Snapshot,
    origins: &BTreeMap<String, Origin>,
    destination_project: &Project,
) -> Result<(), String> {
    let projects = snapshot.dir.join("projects");
    let tasks = snapshot
        .dir
        .join("tasks")
        .join(project_file(&snapshot.project));
    std::fs::create_dir_all(&projects).map_err(|e| e.to_string())?;
    std::fs::create_dir_all(&tasks).map_err(|e| e.to_string())?;
    let mut project_metadata = destination_project.metadata.clone();
    for (key, value) in &snapshot.project_metadata {
        if project_metadata.contains_key(key) {
            project_metadata.insert(key.clone(), value.clone());
        }
    }
    project_metadata.insert(
        "onetaskgraph.origin".into(),
        json!(snapshot.project.as_str()),
    );
    document(
        &projects.join(format!("{}.md", project_file(&snapshot.project))),
        &json!({
            "title": destination_project.title,
            "labels": destination_project.labels,
            "metadata": project_metadata
        }),
        destination_project.content.as_deref().unwrap_or_default(),
    )?;
    let lineages = snapshot.lineages();
    let mut written: BTreeSet<PathBuf> = BTreeSet::new();
    for root in lineages.roots() {
        let (front, content) = task_document(snapshot, &lineages, root, origins.get(root))?;
        let path = tasks.join(format!("{}.md", task_file(root)));
        document(&path, &front, &content)?;
        written.insert(path);
    }
    // A superseded node has no shadow task of its own, and neither has anything an earlier
    // build left here: the copy reads every task the shadow store holds as a member of the
    // project, so a document this snapshot did not write — keyed by no lineage, and read by
    // `known_id` before this runs — would stand as a member the run knows nothing of.
    for entry in std::fs::read_dir(&tasks).map_err(|e| e.to_string())? {
        let path = entry.map_err(|e| e.to_string())?.path();
        if !written.contains(&path) {
            std::fs::remove_file(&path).map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

/// One lineage's shadow task document — its front matter and its body — as the snapshot
/// renders it against what the destination holds for that lineage.
///
/// Keyed by the lineage's `root` and saying what its head says: the head's title (the root
/// id where it carries none), word, body, edges, `delivers` and fields, with `onepipeline.id`
/// the root's, `onepipeline.node` the head's and `onepipeline.supersedes` the ids between.
/// Rendered with no origin, it is the half the run alone decides, which is what tells a
/// lineage whose projection changed from what landed from one whose did not: see [`decide`].
fn task_document(
    snapshot: &Snapshot,
    lineages: &Lineages,
    root: &str,
    origin: Option<&Origin>,
) -> Result<(Value, String), String> {
    let chain = lineages
        .chain(root)
        .ok_or_else(|| format!("no lineage is rooted at '{root}'"))?;
    let (head, superseded) = chain
        .split_last()
        .ok_or_else(|| format!("the lineage rooted at '{root}' holds no node"))?;
    let node = snapshot
        .nodes
        .get(head)
        .ok_or_else(|| format!("the snapshot holds no node '{head}'"))?;
    let mut wire = serde_json::to_value(node)
        .map_err(|e| e.to_string())?
        .as_object()
        .cloned()
        .ok_or_else(|| "node did not serialize as a mapping".to_owned())?;
    let title = wire
        .remove("title")
        .and_then(|v| v.as_str().map(str::to_owned))
        .filter(|title| !title.trim().is_empty())
        .unwrap_or_else(|| root.to_owned());
    let content = wire
        .remove("task")
        .and_then(|v| v.as_str().map(str::to_owned))
        .unwrap_or_default();
    let deps = wire
        .remove("deps")
        .and_then(|v| v.as_array().cloned())
        .unwrap_or_default();
    let repo = wire
        .remove("repo")
        .and_then(|v| v.as_str().map(str::to_owned));
    // The store's own field, never a reserved key.
    let delivers = wire
        .remove(DELIVERS_FIELD)
        .and_then(|v| v.as_array().cloned())
        .unwrap_or_default();
    wire.remove("id");
    // What the node was read out of rather than anything it says: the destination task's
    // own id, key and title are that task's, and a relaunch of a project this run projected
    // onto refuses `onepipeline.task_record` as a key no plan may state.
    wire.remove("task_record");
    let mut metadata = Map::new();
    metadata.insert(ID_KEY.into(), json!(root));
    metadata.insert(NODE_KEY.into(), json!(head));
    if !superseded.is_empty() {
        metadata.insert(SUPERSEDES_KEY.into(), json!(superseded));
    }
    if let Some(origin) = origin {
        metadata.insert(GlobalId::ORIGIN_KEY.into(), json!(origin.id.to_string()));
    }
    for (key, value) in wire {
        metadata.insert(format!("onepipeline.{key}"), value);
    }
    // The settlement, the landing and the change keys are the head's own, absent where the
    // head recorded none: what a superseded attempt closed on is the run journal's to say.
    let id = head.as_str();
    if let Some(settlement) = snapshot.settlements.get(id) {
        metadata.insert(crate::taskgraph::SETTLEMENT_KEY.into(), settlement.clone());
    }
    // What closed the node, beside the word that says it closed. Each is
    // written only where the run *observed* one, so a node with no change of
    // its own — a direct agent node, a human action, a branch its base
    // already carried — carries none of these keys at all rather than an
    // empty value a reader would have to interpret.
    if let Some(landing) = snapshot.landings.get(id) {
        metadata.insert(LANDING_KEY.into(), json!(landing.as_str()));
    }
    // A landing an operator stated is the node's landing, and the reference it was
    // stated at is the one written: the commit or the change request the run
    // recorded for the dispatch it corrected is exactly what the statement
    // superseded, so an item carrying both would say two things about one landing.
    match snapshot.stated_landings.get(id) {
        Some(stated) => {
            metadata.insert(LANDING_EVIDENCE_KEY.into(), json!(stated.tier()));
            let key = match stated {
                crate::edits::StatedLanding::Commit(_) => LANDING_COMMIT_KEY,
                crate::edits::StatedLanding::ChangeRequest(_) => CHANGE_URL_KEY,
            };
            metadata.insert(key.into(), json!(stated.reference()));
        }
        None => {
            if let Some(commit) = snapshot.landing_commits.get(id) {
                metadata.insert(LANDING_COMMIT_KEY.into(), json!(commit));
            }
            if let Some(url) = snapshot.change_urls.get(id) {
                metadata.insert(CHANGE_URL_KEY.into(), json!(url));
            }
        }
    }
    // Each edge names the far shadow task the way that shadow store names its
    // own — `<project>/<task>` — and not by its file alone. What a copy does with
    // an edge is decided by whether its far end is a member of the copied set: a
    // far end that is one is rewritten to the destination's own id for that node,
    // and one that is not is carried through as the source's own qualified id. A
    // bare file name resolves to `<shadow-source>:<file>`, which is a member of
    // nothing, so every edge between two nodes of this plan reached the
    // destination naming the run's scratch store — and a plan whose edges name a
    // source the store does not contain cannot be read back at all. Each far end
    // is the shadow task of *its* lineage's root: a dependency may itself have
    // been retried, and its item is the one its root keys.
    let local_deps: Vec<String> = deps
        .iter()
        .filter_map(Value::as_str)
        .filter(|dep| !crate::graph::is_cross_dag(dep))
        .map(|dep| {
            format!(
                "{}/{}",
                project_file(&snapshot.project),
                task_file(lineages.root_of(dep))
            )
        })
        .collect();
    let cross: Vec<String> = deps
        .iter()
        .filter_map(Value::as_str)
        .filter(|dep| crate::graph::is_cross_dag(dep))
        .map(str::to_owned)
        .collect();
    if !cross.is_empty() {
        metadata.insert("onepipeline.deps".into(), json!(cross));
    }
    let mut front = Map::new();
    front.insert("title".into(), json!(title));
    front.insert("project".into(), json!(project_file(&snapshot.project)));
    front.insert("status".into(), json!(snapshot.word_of(id)));
    // Plan-declared, so owned: the copy is a total replacement, and a node delivering
    // nothing writes none, which is the plan saying so. Every entry is already qualified —
    // `graph::check_node` refused any that was not — so the store carries each one through
    // as the ticket it names rather than as an id of the shadow source.
    if !delivers.is_empty() {
        front.insert(DELIVERS_FIELD.into(), json!(delivers));
    }
    // A node the plan has just added has no destination item yet, so there is nothing
    // to preserve and the created task starts with none.
    front.insert(
        "labels".into(),
        json!(origin.map(|origin| origin.labels.as_slice()).unwrap_or(&[])),
    );
    front.insert("depends_on".into(), json!(local_deps));
    front.insert("metadata".into(), Value::Object(metadata));
    if let Some(repo) = repo {
        if repo.starts_with("github.com/") {
            front.insert("repositories".into(), json!([repo]));
        } else if let Some(Value::Object(metadata)) = front.get_mut("metadata") {
            metadata.insert("onepipeline.repo".into(), json!(repo));
        }
    }
    Ok((Value::Object(front), content))
}

/// Atomic because the shadow store is a directory a reader *lists* while this writes it:
/// the copy reads it as a `local-md` source, and a document replaced in
/// place is truncated first, so a listing arriving in that window parses an empty file and
/// reports the run's own board as malformed.
fn document(path: &Path, front: &Value, body: &str) -> Result<(), String> {
    let yaml = serde_norway::to_string(front).map_err(|e| e.to_string())?;
    crate::ledger::write_atomic(
        path,
        format!("---\n{yaml}---\n{body}").as_bytes(),
        crate::ledger::Durability::Projection,
    )
    .map_err(|e| e.to_string())
}

/// The word one node's state is written onto its destination item's status.
///
/// A onetaskgraph status is a **name** and a normalised **category**, and this is
/// the name — four of these are a category's own word and the rest are words that
/// vocabulary has none of. `docs/contract-divergences.md` records what that costs
/// and why it is the cheaper of the two.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum ProjectedStatus {
    #[serde(rename = "in progress")]
    InProgress,
    #[serde(rename = "done")]
    Done,
    #[serde(rename = "failed")]
    Failed,
    #[serde(rename = "provider-failed")]
    ProviderFailed,
    #[serde(rename = "cancelled")]
    Cancelled,
    #[serde(rename = "parked")]
    Parked,
    #[serde(rename = "skipped")]
    Skipped,
    #[serde(rename = "todo")]
    Todo,
    #[serde(rename = "queued")]
    Queued,
}

/// Mirror one node's settlement onto the word its destination item reads under.
///
/// The outcome is read for one distinction alone, and it is the one no status
/// carries: a dispatch its provider killed and a task its agent failed both
/// settle [`NodeStatus::Failed`], and a reader who cannot tell them apart goes
/// looking for what the work got wrong when nothing was wrong with the work.
fn projected(status: NodeStatus, outcome: Option<&str>, claim: Claim) -> ProjectedStatus {
    match status {
        // The one node here projected under a word that is not its own: a draft
        // has not settled, so `done` and `cancelled` would both be false, and
        // the run is still on it.
        NodeStatus::Running | NodeStatus::CompleteDraft => ProjectedStatus::InProgress,
        NodeStatus::Done => ProjectedStatus::Done,
        NodeStatus::Failed if outcome == Some(crate::engine::PROVIDER_FAILED) => {
            ProjectedStatus::ProviderFailed
        }
        NodeStatus::Failed => ProjectedStatus::Failed,
        NodeStatus::Cancelled => ProjectedStatus::Cancelled,
        NodeStatus::Parked => ProjectedStatus::Parked,
        NodeStatus::Skipped => ProjectedStatus::Skipped,
        NodeStatus::Pending | NodeStatus::Ready | NodeStatus::Waiting | NodeStatus::Blocked => {
            match claim {
                Claim::Held => ProjectedStatus::Queued,
                Claim::Released => ProjectedStatus::Todo,
            }
        }
    }
}

fn all_nodes(paths: &RunPaths, state: &RunState) -> BTreeMap<String, Node> {
    let mut nodes: BTreeMap<String, Node> = state
        .plan
        .as_ref()
        .into_iter()
        .flat_map(|plan| plan.tasks.iter())
        .map(|node| (node.id.clone(), node.clone()))
        .collect();
    for event in crate::journal::read(&paths.journal()) {
        if event.source != Source::Pipeline
            || event.kind.0 != crate::journal::PipelineKind::EditCommitted.as_str()
        {
            continue;
        }
        let Some(operations) = event
            .payload
            .get("operations")
            .and_then(|v| serde_json::from_value::<Vec<Operation>>(v.clone()).ok())
        else {
            continue;
        };
        for operation in operations {
            if let Operation::NodeAdded { node, .. } = operation {
                nodes.insert(node.id.clone(), *node);
            }
        }
    }
    for node in state.graph.iter() {
        nodes.insert(node.id.clone(), node.clone());
    }
    nodes
}

fn settlements(paths: &RunPaths) -> BTreeMap<String, Value> {
    let mut found = BTreeMap::new();
    for event in crate::journal::read(&paths.journal()) {
        if event.source == Source::Pipeline
            && event.kind.0 == crate::journal::PipelineKind::NodeSettled.as_str()
        {
            if let Some(node) = event.labels.node.as_ref() {
                found.insert(node.clone(), Value::Object(event.payload));
            }
        }
    }
    found
}

fn project_file(project: &QualifiedId) -> String {
    encoded(project.as_str())
}
fn task_file(id: &str) -> String {
    encoded(id)
}
fn encoded(value: &str) -> String {
    value
        .as_bytes()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// The shadow store's own qualified id for one node's task, which is what a member copy
/// names.
fn member_id(snapshot: &Snapshot, id: &str) -> GlobalId {
    shadow_id(&format!(
        "{}/{}",
        project_file(&snapshot.project),
        task_file(id)
    ))
}

fn shadow_id(native: &str) -> GlobalId {
    GlobalId::new(
        SourceName::new(SHADOW_SOURCE).expect("the shadow source's name is a source name"),
        NativeId::from(native),
    )
}

/// How many items a copy report says the copy did each thing to, the project item included —
/// and, derived here rather than reported by the store, how many it reopened.
///
/// An item was reopened when the store says it `updated` it, the attempt's own pre-copy read
/// (`before`, off the page of tasks for a whole copy and each member's own read for a member
/// copy) reported its category `done` or `cancelled`, and the word this snapshot projects its
/// lineage under is neither. The store's copy-report vocabulary is not extended for it: the
/// store's report says what it wrote, and a reopen is a fact about the run.
///
/// A destination the copy `updated` is never also counted `orphaned`. An item an older build
/// wrote can still carry that build's origin after an adoption reuses it for a lineage, so the
/// store reports it once as the lineage it rewrote and again as a counterpart the source no
/// longer holds — one item, and the second report is not true of it: it was rewritten, not
/// left as it was.
fn actions(
    report: &CopyReport,
    snapshot: &Snapshot,
    before: &BTreeMap<String, Origin>,
) -> ProjectionActions {
    let lineages = snapshot.lineages();
    let members = shadow_members(snapshot, &lineages);
    let rewritten: std::collections::HashSet<&GlobalId> = report
        .items
        .iter()
        .filter_map(|item| match &item.action {
            CopyAction::Updated { destination } => Some(destination),
            CopyAction::Created { .. }
            | CopyAction::Unchanged { .. }
            | CopyAction::Orphaned { .. } => None,
        })
        .collect();
    let mut actions = ProjectionActions::default();
    for item in &report.items {
        let count = match &item.action {
            CopyAction::Orphaned { destination } if rewritten.contains(destination) => continue,
            CopyAction::Created { .. } => &mut actions.created,
            CopyAction::Updated { .. } => &mut actions.updated,
            CopyAction::Unchanged { .. } => &mut actions.unchanged,
            CopyAction::Orphaned { .. } => &mut actions.orphaned,
        };
        *count = count.saturating_add(1);
        if !matches!(item.action, CopyAction::Updated { .. }) {
            continue;
        }
        let reopened = members.get(&item.source).is_some_and(|root| {
            before.get(*root).is_some_and(Origin::closed)
                && lineages
                    .chain(root)
                    .and_then(<[String]>::last)
                    .is_some_and(|head| {
                        !matches!(
                            snapshot.word_of(head),
                            ProjectedStatus::Done | ProjectedStatus::Cancelled
                        )
                    })
        });
        if reopened {
            actions.reopened = actions.reopened.saturating_add(1);
        }
    }
    actions
}

/// Fold where the copy says each carried lineage landed into what the run knows.
///
/// A lineage the copy created has a destination item nobody has read yet, and without this
/// the next member copy naming it would carry no origin and create it a second time. An item
/// that is not one of this snapshot's shadow tasks says nothing about a lineage.
fn learn(report: &CopyReport, origins: &mut BTreeMap<String, Origin>, snapshot: &Snapshot) {
    let lineages = snapshot.lineages();
    let members = shadow_members(snapshot, &lineages);
    for item in &report.items {
        let destination = match &item.action {
            CopyAction::Orphaned { .. } => continue,
            action => action.destination(),
        };
        let (Some(node), Some(destination)) = (members.get(&item.source), destination) else {
            continue;
        };
        match origins.get_mut(*node) {
            Some(origin) if origin.id == *destination => {}
            Some(origin) => {
                origin.id = destination.clone();
                origin.labels = Vec::new();
                origin.category = None;
            }
            None => {
                origins.insert(
                    (*node).clone(),
                    Origin {
                        id: destination.clone(),
                        labels: Vec::new(),
                        category: None,
                    },
                );
            }
        }
    }
}

/// Each lineage's shadow member id, mapped back to its root: what a copy report's `source`
/// names.
fn shadow_members<'a>(
    snapshot: &Snapshot,
    lineages: &'a Lineages,
) -> std::collections::HashMap<GlobalId, &'a String> {
    lineages
        .roots()
        .map(|root| (member_id(snapshot, root), root))
        .collect()
}

/// Append one attempt to the run's [`WRITEBACK_PROJECTIONS_FILE`].
///
/// Best effort, like the projection it records: a line that cannot be written is said on the
/// driver's standard error and changes nothing about the attempt or the run.
fn append_record(run_dir: &Path, record: &ProjectionRecord) {
    let path = run_dir.join(WRITEBACK_PROJECTIONS_FILE);
    // llmlint: ignore-block[changed_behavior_has_e2e] appending to a file in the run's own
    // directory fails only on a host whose run directory has been made unwritable, which no
    // CLI journey arranges; every journey reading the record drives the write that lands.
    let written = serde_json::to_string(record)
        .map_err(|error| error.to_string())
        .and_then(|line| {
            let mut file = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&path)
                .map_err(|error| error.to_string())?;
            std::io::Write::write_all(&mut file, format!("{line}\n").as_bytes())
                .map_err(|error| error.to_string())
        });
    if let Err(error) = written {
        eprintln!(
            "onetaskgraph write-back could not record a projection attempt at {}: {error}",
            path.display()
        );
    }
    // llmlint: ignore-end[changed_behavior_has_e2e]
}

/// One write-back projection attempt, as a run's
/// [`WRITEBACK_PROJECTIONS_FILE`](crate::cli::WRITEBACK_PROJECTIONS_FILE) holds it: one JSON
/// object per line, appended as the attempt ends and never rewritten.
///
/// What a projection's cost is read off: what it carried, how it ended, how long it took,
/// which store calls it made, and what the store said it spent. Entry 73 of
/// `docs/contract-divergences.md` states the shape and is the one source for it. The wire form
/// is flat — every key written, `null` where it says nothing — and this is the shape of it that
/// cannot say a contradiction: a member copy has no reason to be whole, and a failed attempt
/// names neither `actions` nor `spent`, the halves of a copy report only a landed one has.
/// `delivered` is the half a partial copy reports as it fails, so it is kept either way.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "ProjectionWire", into = "ProjectionWire")]
pub struct ProjectionRecord {
    // llmlint: ignore-block[invalid_states_unrepresentable] `at` is written by
    // `sys::now_rfc3339`, the crate's one timestamp writer, and is a string on every record
    // this crate keeps; `project` is the qualified id the worker parsed at its boundary, and the
    // type it was parsed into is private to the store mapping — a reader of this record only
    // names it; and each item is a plan node id, the plain string every identifier here is.
    /// When the attempt started, as an RFC 3339 UTC timestamp.
    pub at: String,
    /// The qualified onetaskgraph project the attempt projected onto.
    pub project: String,
    /// Whether the copy carried the whole project or named members of it, and why whole.
    pub scope: ProjectionScope,
    /// The plan node ids the copy carried: every node, for a whole copy.
    pub items: Vec<String>,
    // llmlint: ignore-end[invalid_states_unrepresentable]
    /// Wall-clock milliseconds of the whole attempt, its reads included.
    pub duration_ms: u64,
    /// How the attempt ended, and what the store said about it.
    pub ended: ProjectionEnded,
    /// The copy report's `delivered` entries, verbatim: what the store did to each ticket a
    /// carried task delivers, whether the attempt landed or not. Empty where the report carried
    /// none, and then left off the line.
    pub delivered: Vec<Map<String, Value>>,
    /// Which store operations the attempt called, and how many items its targeted updates
    /// wrote each field on. `None` only on a line an earlier build wrote, which named none;
    /// version 4 added it, and every line this build writes names it.
    pub calls: Option<ProjectionCalls>,
}

/// How many times one projection attempt called each store operation — the one that failed
/// included, an operation not called left off — and, for an attempt that made targeted
/// updates, how many items each field was written on.
///
/// One value, so a record cannot name field updates without the `task-update` calls that wrote
/// them: [`ProjectionCalls::new`] refuses that, and is the only way to build one.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProjectionCalls {
    counts: BTreeMap<StoreCall, NonZeroU64>,
    updated_fields: Option<BTreeMap<UpdatedField, NonZeroU64>>,
}

impl ProjectionCalls {
    /// The calls one attempt made, and what its targeted updates wrote where it made any.
    ///
    /// # Errors
    ///
    /// Refuses `updated_fields` beside no `task-update` call — fields no update wrote — and a
    /// `task-update` call beside no `updated_fields`, which every attempt that made one names.
    pub fn new(
        counts: BTreeMap<StoreCall, NonZeroU64>,
        updated_fields: Option<BTreeMap<UpdatedField, NonZeroU64>>,
    ) -> Result<Self, String> {
        match (
            updated_fields.is_some(),
            counts.contains_key(&StoreCall::TaskUpdate),
        ) {
            (true, false) => {
                return Err(
                    "`updated_fields` is named without a `task-update` call to have written them"
                        .to_owned(),
                )
            }
            (false, true) => {
                return Err(
                    "`task-update` was called and no `updated_fields` says what it wrote"
                        .to_owned(),
                )
            }
            (true, true) | (false, false) => {}
        }
        Ok(Self {
            counts,
            updated_fields,
        })
    }

    /// How many times each store operation was called; one not called is not named.
    #[must_use]
    pub fn counts(&self) -> &BTreeMap<StoreCall, NonZeroU64> {
        &self.counts
    }

    /// How many items each field was written on, where the attempt made a targeted update.
    #[must_use]
    pub fn updated_fields(&self) -> Option<&BTreeMap<UpdatedField, NonZeroU64>> {
        self.updated_fields.as_ref()
    }
}

/// One store operation a projection attempt can call, by the name the record and its
/// refusals spell it with. A closed set, so a call this build does not name is a compile error
/// rather than a word a reader has never seen.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum StoreCall {
    /// Reading the destination project.
    ProjectShow,
    /// Reading a page of the destination project's tasks. **Read, never written**: a line an
    /// earlier build wrote may count it, and no attempt this build makes calls it.
    TaskList,
    /// Reading one destination task by its own id.
    TaskShow,
    /// The copy.
    ProjectCopy,
    /// A targeted update of one task. Named for the record's reader; this build calls none.
    TaskUpdate,
    /// Setting one project metadata key. Named for the record's reader; this build calls none.
    ProjectMetadataSet,
}

impl StoreCall {
    /// The name a refusal of this call carries, which is its word in the record.
    fn as_str(self) -> &'static str {
        match self {
            Self::ProjectShow => PROJECT_SHOW,
            Self::TaskList => "task-list",
            Self::TaskShow => TASK_SHOW,
            Self::ProjectCopy => PROJECT_COPY,
            Self::TaskUpdate => "task-update",
            Self::ProjectMetadataSet => "project-metadata-set",
        }
    }
}

/// One field a targeted update writes, as the record's `updated_fields` keys it. Defined by
/// version 4 of the record; this build writes no targeted update, so it never writes one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum UpdatedField {
    /// The task's title.
    Title,
    /// The task's body.
    Content,
    /// The task's status.
    Status,
    /// The task's priority.
    Priority,
    /// The task's metadata.
    Metadata,
    /// The tickets the task delivers.
    Delivers,
    /// The task's forward dependency edges.
    DependsOn,
}

/// What one projection attempt carried.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProjectionScope {
    /// Every node of the plan, for the reason given — read off a line an earlier build wrote.
    Whole(WholeBecause),
    /// Only the lineages whose projection differs from what landed: every attempt this build
    /// makes.
    Members,
}

/// Why a projection attempt carried the whole project.
///
/// **Read, never written**: every attempt this build makes carries the lineages whose
/// projection differs from what landed, so a line naming one of these is a line an earlier
/// build wrote.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum WholeBecause {
    /// Nothing had landed in that driver yet: its first projection, including one an `adopt`
    /// started.
    First,
    /// The attempt before that one failed.
    AfterFailure,
    /// The store reported a version older than the first release offering a member copy.
    ///
    /// **Read, never written.** An engine that drove the store's binary decided this off the
    /// version the binary reported, so lines it wrote carry the reason and still read. The
    /// linked store always offers a member copy, so this build never gives it.
    StoreLacksMembers,
}

/// How one projection attempt ended.
#[derive(Debug, Clone, PartialEq)]
pub enum ProjectionEnded {
    /// The copy landed.
    Projected {
        /// How many items the copy report says it did each thing to, or `None` where no
        /// report was read — which a line an older engine wrote may say, and this one never
        /// does: the linked store always answers a copy with its report.
        actions: Option<ProjectionActions>,
        /// The copy report's `spent`, verbatim, or `None` where the report carried none.
        spent: Option<Map<String, Value>>,
    },
    /// The attempt failed, at whichever of its commands failed first.
    Failed {
        /// The store's class and kind, where its failure document gave them.
        classified: Option<ProjectionFailure>,
        // llmlint: ignore[invalid_states_unrepresentable] the store's own words, or the
        // worker's, carried to a reader exactly as the `Unprojected` surface carries them.
        /// What the store, or the worker, said went wrong.
        reason: String,
    },
}

/// What the store classed a failed attempt as.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectionFailure {
    /// Whether asking again unchanged could change the store's answer.
    pub class: FailureClass,
    // llmlint: ignore[invalid_states_unrepresentable] open by the store's own contract, for
    // the reason `Classified::kind` records; only ever named to a reader.
    /// The store's kind of failure, verbatim.
    pub kind: String,
}

/// How many items a copy report says the copy did each thing to, its project item included.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectionActions {
    /// Items the destination held no counterpart for, so the copy created one.
    pub created: u64,
    /// Items whose counterpart the copy rewrote.
    pub updated: u64,
    /// Items whose counterpart already read as the source does.
    pub unchanged: u64,
    /// Counterparts the source no longer holds, left as they were.
    pub orphaned: u64,
    /// Carried items the copy rewrote from a closed category — `done` or `cancelled`, as the
    /// attempt's own pre-copy read reported it — onto a word that is neither: a retry or a
    /// requeue of a cancelled node, which the store pairs with reopening the item. Derived by
    /// this worker, never the store's word; version 3 of the record added it.
    pub reopened: u64,
}

impl ProjectionRecord {
    fn of(at: String, project: &QualifiedId, took: Duration, attempted: &Attempted) -> Self {
        Self {
            at,
            project: project.as_str().to_owned(),
            scope: ProjectionScope::Members,
            items: attempted.items.clone(),
            duration_ms: u64::try_from(took.as_millis()).unwrap_or(u64::MAX),
            ended: match &attempted.result {
                Ok(landed) => ProjectionEnded::Projected {
                    actions: landed.actions,
                    spent: landed.spent.clone(),
                },
                Err(failed) => ProjectionEnded::Failed {
                    classified: failed
                        .classified
                        .as_ref()
                        .map(|classified| ProjectionFailure {
                            class: classified.class,
                            kind: classified.kind.clone(),
                        }),
                    reason: failed.reason.clone(),
                },
            },
            delivered: match &attempted.result {
                Ok(landed) => landed.delivered.clone(),
                Err(failed) => failed.delivered.clone(),
            },
            calls: Some(ProjectionCalls {
                counts: attempted
                    .calls
                    .iter()
                    .filter_map(|(call, count)| NonZeroU64::new(*count).map(|count| (*call, count)))
                    .collect(),
                updated_fields: None,
            }),
        }
    }
}

/// The flat line [`ProjectionRecord`] is written as and read from, key for key as entry 73
/// states it.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ProjectionWire {
    /// Written on every line as [`WRITEBACK_PROJECTIONS_SCHEMA_VERSION`]; a line an earlier build
    /// wrote names none, and is version 1.
    #[serde(default = "unversioned_projection_line")]
    schema_version: u32,
    at: String,
    project: String,
    scope: ScopeWord,
    whole_because: Option<WholeBecause>,
    items: Vec<String>,
    outcome: OutcomeWord,
    class: Option<FailureClass>,
    kind: Option<String>,
    reason: Option<String>,
    duration_ms: u64,
    actions: Option<ActionsWire>,
    spent: Option<Map<String, Value>>,
    /// The one key a line may leave off: absent where no ticket was reported, so a line that
    /// reached none reads exactly as it did before tickets were. Held as an `Option` because
    /// version 1 is refused by this key's own name — so a line that *names* it, even as an
    /// empty list, has to be told apart from one that leaves it off.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    delivered: Option<Vec<Map<String, Value>>>,
    /// Every version 4 line names it; an earlier one names none, and is refused naming it.
    /// Held twice optional, so a line naming it as `null` is told apart from one leaving it off
    /// and refused by the key's own name rather than read as never having named it.
    #[serde(
        default,
        deserialize_with = "named",
        skip_serializing_if = "Option::is_none"
    )]
    calls: Option<Option<BTreeMap<StoreCall, NonZeroU64>>>,
    /// Only a version 4 line that made a targeted update names it; held as `calls` is.
    #[serde(
        default,
        deserialize_with = "named",
        skip_serializing_if = "Option::is_none"
    )]
    updated_fields: Option<Option<BTreeMap<UpdatedField, NonZeroU64>>>,
}

/// A key that is on the line, whatever it holds — `null` included — as `Some`; the key left
/// off stays `None` through `#[serde(default)]`.
fn named<'de, D, T>(deserializer: D) -> Result<Option<Option<T>>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(deserializer).map(Some)
}

/// The version of a projection line that names none: the shape before `delivered` existed.
fn unversioned_projection_line() -> u32 {
    1
}

/// The version that added `delivered`, and the last that named no `actions.reopened`.
const PROJECTION_LINE_WITH_DELIVERED: u32 = 2;

/// The version that added `actions.reopened`, and the last that named no `calls` — which is
/// the version a line an earlier build wrote is written back at, since it counts no calls.
const PROJECTION_LINE_WITH_REOPENED: u32 = 3;

/// [`ProjectionActions`] as a line carries it. `reopened` is held as an `Option` for the
/// reason `delivered` is: a version 1 or 2 line is refused by the key's own name, so a line
/// that names it — even as zero — has to be told apart from one that leaves it off.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ActionsWire {
    created: u64,
    updated: u64,
    unchanged: u64,
    orphaned: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    reopened: Option<u64>,
}

impl From<ProjectionActions> for ActionsWire {
    fn from(actions: ProjectionActions) -> Self {
        Self {
            created: actions.created,
            updated: actions.updated,
            unchanged: actions.unchanged,
            orphaned: actions.orphaned,
            reopened: Some(actions.reopened),
        }
    }
}

impl From<ActionsWire> for ProjectionActions {
    fn from(wire: ActionsWire) -> Self {
        Self {
            created: wire.created,
            updated: wire.updated,
            unchanged: wire.unchanged,
            orphaned: wire.orphaned,
            reopened: wire.reopened.unwrap_or_default(),
        }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
enum ScopeWord {
    Whole,
    Members,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
enum OutcomeWord {
    Projected,
    Failed,
}

impl TryFrom<ProjectionWire> for ProjectionRecord {
    type Error = String;

    fn try_from(wire: ProjectionWire) -> Result<Self, String> {
        let names_reopened = wire
            .actions
            .as_ref()
            .is_some_and(|actions| actions.reopened.is_some());
        let names_calls = wire.calls.is_some() || wire.updated_fields.is_some();
        match wire.schema_version {
            WRITEBACK_PROJECTIONS_SCHEMA_VERSION | PROJECTION_LINE_WITH_REOPENED
                if wire.actions.is_some() && !names_reopened =>
            {
                return Err(format!(
                    "a version {} line names `actions` without `actions.reopened`, which every \
                     landed attempt at that version names",
                    wire.schema_version
                ));
            }
            WRITEBACK_PROJECTIONS_SCHEMA_VERSION => {
                let Some(Some(calls)) = &wire.calls else {
                    return Err(format!(
                        "a version {WRITEBACK_PROJECTIONS_SCHEMA_VERSION} line names no `calls` \
                         object, which every line at that version names"
                    ));
                };
                if matches!(wire.updated_fields, Some(None)) {
                    return Err("a line names `updated_fields` as null".to_owned());
                }
                ProjectionCalls::new(calls.clone(), wire.updated_fields.clone().flatten())?;
            }
            1 | PROJECTION_LINE_WITH_DELIVERED | PROJECTION_LINE_WITH_REOPENED if names_calls => {
                return Err(format!(
                    "a version {} line names `calls` or `updated_fields`, which version \
                     {WRITEBACK_PROJECTIONS_SCHEMA_VERSION} added",
                    wire.schema_version
                ))
            }
            1 if wire.delivered.is_some() => {
                return Err(format!(
                    "a version 1 line names `delivered`, which version \
                     {PROJECTION_LINE_WITH_DELIVERED} added"
                ))
            }
            1 | PROJECTION_LINE_WITH_DELIVERED if names_reopened => {
                return Err(format!(
                    "a version {} line names `actions.reopened`, which version \
                     {PROJECTION_LINE_WITH_REOPENED} added",
                    wire.schema_version
                ))
            }
            1 | PROJECTION_LINE_WITH_DELIVERED | PROJECTION_LINE_WITH_REOPENED => {}
            other => {
                return Err(format!(
                    "`schema_version` {other} is not one this build reads (1, \
                     {PROJECTION_LINE_WITH_DELIVERED}, {PROJECTION_LINE_WITH_REOPENED}, \
                     {WRITEBACK_PROJECTIONS_SCHEMA_VERSION})"
                ))
            }
        }
        if !(crate::watchers::is_rfc3339(&wire.at) && wire.at.ends_with(['Z', 'z'])) {
            return Err(format!("`at` is not an RFC 3339 UTC time: {:?}", wire.at));
        }
        QualifiedId::try_from(wire.project.clone()).map_err(|why| format!("`project`: {why}"))?;
        if wire.items.iter().any(String::is_empty) {
            return Err("`items` names an empty node id".to_owned());
        }
        let scope = match (wire.scope, wire.whole_because) {
            (ScopeWord::Whole, Some(because)) => ProjectionScope::Whole(because),
            (ScopeWord::Members, None) => ProjectionScope::Members,
            (ScopeWord::Whole, None) => {
                return Err("a whole projection names no `whole_because`".to_owned())
            }
            (ScopeWord::Members, Some(_)) => {
                return Err(
                    "a member projection names a `whole_because`, which only a whole one has"
                        .to_owned(),
                )
            }
        };
        let ended = match wire.outcome {
            OutcomeWord::Projected => {
                if wire.class.is_some() || wire.kind.is_some() || wire.reason.is_some() {
                    return Err(
                        "a projected attempt names a failure's `class`, `kind` or `reason`"
                            .to_owned(),
                    );
                }
                ProjectionEnded::Projected {
                    actions: wire.actions.map(ProjectionActions::from),
                    spent: wire.spent,
                }
            }
            OutcomeWord::Failed => {
                if wire.actions.is_some() || wire.spent.is_some() {
                    return Err(
                        "a failed attempt names a copy report's `actions` or `spent`".to_owned(),
                    );
                }
                let classified =
                    match (wire.class, wire.kind) {
                        (Some(class), Some(kind)) => Some(ProjectionFailure { class, kind }),
                        (None, None) => None,
                        _ => return Err(
                            "a failed attempt names one of `class` and `kind` without the other"
                                .to_owned(),
                        ),
                    };
                ProjectionEnded::Failed {
                    classified,
                    reason: wire
                        .reason
                        .ok_or_else(|| "a failed attempt names no `reason`".to_owned())?,
                }
            }
        };
        Ok(Self {
            at: wire.at,
            project: wire.project,
            scope,
            items: wire.items,
            duration_ms: wire.duration_ms,
            ended,
            delivered: wire.delivered.unwrap_or_default(),
            calls: match wire.calls.flatten() {
                Some(counts) => Some(ProjectionCalls::new(counts, wire.updated_fields.flatten())?),
                None => None,
            },
        })
    }
}

impl From<ProjectionRecord> for ProjectionWire {
    fn from(record: ProjectionRecord) -> Self {
        let (scope, whole_because) = match record.scope {
            ProjectionScope::Whole(because) => (ScopeWord::Whole, Some(because)),
            ProjectionScope::Members => (ScopeWord::Members, None),
        };
        let (outcome, class, kind, reason, actions, spent) = match record.ended {
            ProjectionEnded::Projected { actions, spent } => (
                OutcomeWord::Projected,
                None,
                None,
                None,
                actions.map(ActionsWire::from),
                spent,
            ),
            ProjectionEnded::Failed { classified, reason } => {
                let (class, kind) = classified
                    .map(|failure| (Some(failure.class), Some(failure.kind)))
                    .unwrap_or_default();
                (OutcomeWord::Failed, class, kind, Some(reason), None, None)
            }
        };
        Self {
            // A record that counts its calls is written at the version that added them; one an
            // earlier build wrote counts none, and is written back at the last version that
            // named none rather than as a version 4 line missing a key every one names.
            schema_version: if record.calls.is_some() {
                WRITEBACK_PROJECTIONS_SCHEMA_VERSION
            } else {
                PROJECTION_LINE_WITH_REOPENED
            },
            at: record.at,
            project: record.project,
            scope,
            whole_because,
            items: record.items,
            outcome,
            class,
            kind,
            reason,
            duration_ms: record.duration_ms,
            actions,
            spent,
            delivered: (!record.delivered.is_empty()).then_some(record.delivered),
            calls: record
                .calls
                .as_ref()
                .map(|calls| Some(calls.counts.clone())),
            updated_fields: record
                .calls
                .and_then(|calls| calls.updated_fields)
                .map(Some),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        per_item_budget, projected, write_shadow, Claim, Classified, Deadline, FailureClass,
        Landing, Origin, Pending, ProjectedStatus, Snapshot, WorkerState, Writeback,
        CHANGE_URL_KEY, COMMAND_FLOOR, DELIVERS_FIELD, ID_KEY, LANDING_COMMIT_KEY,
        LANDING_EVIDENCE_KEY, LANDING_KEY, NODE_KEY, SUPERSEDES_KEY,
    };
    use crate::cli::DEFAULT_WRITEBACK_ITEM_BUDGET_SECONDS;
    use crate::graph::NodeStatus;
    use crate::ledger::{LaunchRecord, RunPaths};
    use crate::plan::Node;
    use crate::projection::RunState;
    use onetaskgraph_core::{EngineError, GlobalId};
    use onetaskgraph_plugin_api::{Label, Project, SourceError, StatusCategory};
    use serde_json::{json, Map, Value};
    use std::collections::BTreeMap;
    use std::num::NonZeroU64;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::{Arc, Condvar, Mutex};
    use std::time::{Duration, Instant};

    fn snapshot(status: NodeStatus) -> Snapshot {
        Fixture::new("queue").snapshot_with(|snapshot| {
            snapshot.project = "plans:deduplication".parse().expect("a qualified project");
            snapshot.dir = PathBuf::from("writeback");
            snapshot.statuses = BTreeMap::from([("node".to_owned(), status)]);
        })
    }

    /// The copy's deadline is the floor plus the per-item budget multiplied by the item count;
    /// the refusal says what it was computed from.
    ///
    /// The incident: a seven-item copy onto the `plans` board measured 71 to 72 seconds, and
    /// the larger of the floor and ten seconds per item allowed it 70 (#521). Under the
    /// shipped budget that copy is allowed the floor and seven twelve-second items besides.
    #[test]
    fn the_copy_deadline_is_the_floor_plus_the_budget_times_the_items() {
        let shipped = DEFAULT_WRITEBACK_ITEM_BUDGET_SECONDS;
        let copy = |items: usize| Deadline::Copy {
            per_item: shipped,
            items,
        };
        assert_eq!(copy(7).within(), Duration::from_secs(144));
        assert_eq!(
            copy(7).refusal("project-copy"),
            "project-copy exceeded 144 seconds (the 60 second floor + 7 items × 12 seconds \
             per item)"
        );
        // Seven items at eleven seconds apiece — slower than the board measured — land.
        assert!(copy(7).within() > Duration::from_secs(7 * 11));
        // And a copy at the measured 10.2 seconds per item lands with a sixth to spare
        // however many items it carries, which the floor alone could never promise.
        for items in [1_usize, 7, 34, 1_000] {
            let measured = Duration::from_millis(10_200 * items as u64);
            assert!(
                copy(items).within() > measured + measured / 6,
                "{items} items at the measured pace outrun their deadline"
            );
        }

        // No items is the floor alone, and said as the arithmetic it is.
        assert_eq!(copy(0).within(), COMMAND_FLOOR);
        assert_eq!(
            copy(0).refusal("project-copy"),
            "project-copy exceeded 60 seconds (the 60 second floor + 0 items × 12 seconds \
             per item)"
        );

        // One of each, in the singular, so the line reads as a sentence.
        let one = Deadline::Copy {
            per_item: NonZeroU64::MIN,
            items: 1,
        };
        assert_eq!(
            one.refusal("project-copy"),
            "project-copy exceeded 61 seconds (the 60 second floor + 1 item × 1 second per \
             item)"
        );

        // The reads are the floor alone, and their refusal is the line it always was.
        assert_eq!(Deadline::Floor.within(), COMMAND_FLOOR);
        assert_eq!(
            Deadline::Floor.refusal("project-show"),
            "project-show exceeded 60 seconds"
        );
        assert_eq!(
            Deadline::Floor.refusal("task-show"),
            "task-show exceeded 60 seconds"
        );

        // The product is exact for every count a `u64` of seconds can carry — well past
        // the four billion a narrower multiplication would have capped at — and saturates
        // to `u64::MAX` seconds beyond that, rather than wrapping to a figure the floor
        // would then govern alone.
        let vast = Deadline::Copy {
            per_item: NonZeroU64::MIN,
            items: usize::MAX / 2,
        };
        assert_eq!(
            vast.within(),
            Duration::from_secs(usize::MAX as u64 / 2 + COMMAND_FLOOR.as_secs())
        );
        let saturated = Deadline::Copy {
            per_item: NonZeroU64::MAX,
            items: 2,
        };
        assert_eq!(saturated.within(), Duration::MAX);
        assert_eq!(
            saturated.refusal("project-copy"),
            format!(
                "project-copy exceeded {} seconds (the 60 second floor + 2 items × {} seconds \
                 per item)",
                u64::MAX,
                u64::MAX
            )
        );
    }

    /// Every worked example entry 71 of the divergence record states is the deadline
    /// [`Deadline`] enforces and the refusal it gives, and the formula the entry states is
    /// the one `within` computes.
    ///
    /// The record's block is the one source every copy of this setting is held to, and
    /// `tests/contract.rs` holds it against the public types; the deadline itself is
    /// private, so this is where the block's arithmetic meets what the worker runs under.
    #[test]
    fn the_divergence_records_examples_are_the_deadlines_the_copy_runs_under() {
        let record = std::fs::read_to_string(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("docs/contract-divergences.md"),
        )
        .expect("the divergence record ships");
        let entry = record
            .split("\n## ")
            .find(|entry| entry.starts_with("71."))
            .expect("the record still carries entry 71");
        let block = entry
            .split("```json")
            .nth(1)
            .and_then(|rest| rest.split("```").next())
            .expect("entry 71 carries its json block");
        let budget = serde_json::from_str::<Value>(block).expect("entry 71's block is JSON")
            ["budget"]
            .clone();

        assert_eq!(
            budget["floor_seconds"].as_u64().map(Duration::from_secs),
            Some(COMMAND_FLOOR),
            "entry 71 states a floor other than the one the copy runs under"
        );
        assert_eq!(
            budget["deadline"].as_str(),
            Some("floor_seconds + budget × items"),
            "entry 71 states a deadline formula other than the one `Deadline::within` computes"
        );
        let examples = budget["examples"]
            .as_array()
            .expect("entry 71 works an example");
        assert!(!examples.is_empty(), "{budget}");
        for example in examples {
            let items = usize::try_from(example["items"].as_u64().expect("an item count"))
                .expect("a count");
            let per_item = NonZeroU64::new(example["budget_seconds"].as_u64().expect("a budget"))
                .expect("entry 71 works an example under a budget of zero");
            let copy = Deadline::Copy { per_item, items };
            assert_eq!(
                copy.within(),
                Duration::from_secs(example["deadline_seconds"].as_u64().expect("a deadline")),
                "entry 71's example is not the deadline the copy runs under: {example}"
            );
            assert_eq!(
                Some(copy.refusal("project-copy").as_str()),
                example["refusal"].as_str(),
                "entry 71's example is not the refusal the copy is killed with: {example}"
            );
        }
    }

    /// A rate-limited failure carries the wait its source asked for, read off the store's own
    /// typed value wherever the failure arrives — a call the engine failed, wrapped once more
    /// in a copy it could not undo, or one source of a partial answer, where the longest wait
    /// any of them asked for is the one taken — and every other failure, a rate limit naming
    /// no wait included, carries none, so the schedule decides.
    #[test]
    fn a_rate_limited_failure_carries_the_wait_its_source_asked_for() {
        use super::Failed;
        let limited = |seconds: Option<u64>| SourceError::RateLimited {
            retry_after_seconds: seconds,
            message: None,
        };
        let failed = |error: SourceError| EngineError::SourceFailed {
            name: "plans".to_owned(),
            error,
        };
        assert_eq!(
            Failed::engine("project-show", &failed(limited(Some(7)))).wait,
            Some(Duration::from_secs(7))
        );
        assert_eq!(
            Failed::engine("project-show", &failed(limited(None))).wait,
            None
        );
        let unreachable = SourceError::Unavailable {
            message: "gone".to_owned(),
        };
        assert_eq!(
            Failed::engine("project-show", &failed(unreachable.clone())).wait,
            None
        );
        let undone = EngineError::CopyNotUndone {
            error: Box::new(failed(limited(Some(30)))),
            left_behind: onetaskgraph_core::LeftBehind::new(
                "plans:board/1".parse::<GlobalId>().expect("an id"),
            ),
            refusal: SourceError::Refused {
                message: "no".to_owned(),
            },
        };
        assert_eq!(
            Failed::engine("project-copy", &undone).wait,
            Some(Duration::from_secs(30))
        );
        let source = |error: SourceError| onetaskgraph_core::SourceFailure {
            source: onetaskgraph_plugin_api::SourceName::new("plans").expect("a source name"),
            error,
        };
        assert_eq!(
            Failed::partial(
                "task-show",
                &[
                    source(limited(Some(4))),
                    source(unreachable),
                    source(limited(Some(9)))
                ]
            )
            .wait,
            Some(Duration::from_secs(9))
        );
        assert_eq!(Failed::from("cancelled".to_owned()).wait, None);
    }

    /// The name a refusal gives each store call is the word the record serializes it as, and
    /// the three an attempt makes are the calls `cli` publishes, so the record and a refusal
    /// cannot name one call two ways.
    #[test]
    fn every_store_call_is_named_as_the_record_writes_it() {
        use super::StoreCall;
        for call in [
            StoreCall::ProjectShow,
            StoreCall::TaskList,
            StoreCall::TaskShow,
            StoreCall::ProjectCopy,
            StoreCall::TaskUpdate,
            StoreCall::ProjectMetadataSet,
        ] {
            assert_eq!(
                json!(call.as_str()),
                serde_json::to_value(call).expect("a call serializes"),
                "{call:?}"
            );
        }
        assert_eq!(
            crate::cli::WRITEBACK_CLASSIFIED_COMMANDS,
            [
                StoreCall::ProjectShow.as_str(),
                StoreCall::TaskShow.as_str(),
                StoreCall::ProjectCopy.as_str()
            ]
        );
    }

    /// The budget the worker runs under is the one the launch record retained, and a
    /// record written before the field existed runs under the shipped default rather than
    /// under a budget of zero.
    #[test]
    fn the_per_item_budget_is_the_launch_records_or_the_shipped_default() {
        let paths = RunPaths {
            run: "budget".to_owned(),
            dir: PathBuf::from("budget"),
        };
        let chosen = LaunchRecord {
            writeback_item_budget: 25,
            success_hook: String::new(),
            failure_hook: String::new(),
            hook_timeout: 0,
            ..a_launch(&paths)
        };
        assert_eq!(
            per_item_budget(&chosen),
            NonZeroU64::new(25).expect("a budget")
        );

        let older: LaunchRecord = serde_json::from_value(json!({
            "run_id": "older",
            "project": "plans:older",
            "launcher": "test",
        }))
        .expect("a record predating the field reads");
        assert_eq!(older.item_budget(), None);
        assert_eq!(
            per_item_budget(&older),
            DEFAULT_WRITEBACK_ITEM_BUDGET_SECONDS
        );
    }

    fn divergence_block(number: &str) -> Value {
        let record = std::fs::read_to_string(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("docs/contract-divergences.md"),
        )
        .expect("the divergence record ships");
        let entry = record
            .split("\n## ")
            .find(|entry| entry.starts_with(number))
            .unwrap_or_else(|| panic!("the record still carries entry {number}"));
        let block = entry
            .split("```json")
            .nth(1)
            .and_then(|rest| rest.split("```").next())
            .unwrap_or_else(|| panic!("entry {number} carries its json block"));
        serde_json::from_str(block).unwrap_or_else(|e| panic!("entry {number}'s block: {e}"))
    }

    /// A snapshot the store refused is not attempted again when it is published again, and
    /// any snapshot that differs from it is.
    ///
    /// The store would refuse the same projection the same way, so the only thing worth
    /// asking it about again is a graph that has changed.
    #[test]
    fn a_refused_snapshot_published_again_is_not_queued_and_a_different_one_is() {
        let refused = snapshot(NodeStatus::Failed);
        let mut pending = Pending {
            refused: Some(refused.clone()),
            ..Pending::default()
        };

        assert!(
            !pending.queue(refused.clone()),
            "the snapshot the store refused was queued to be refused again"
        );
        assert!(pending.latest.is_none());

        let changed = snapshot(NodeStatus::Done);
        assert!(
            pending.queue(changed.clone()),
            "a changed graph was not queued after a refusal"
        );
        assert!(pending.latest.as_ref() == Some(&changed));
    }

    /// Entry 72's rule is the one the worker classifies by: the class it names stops the
    /// timer, the calls it names are the three an attempt makes, a source error is classed by
    /// the store's own classifier — the kinds it names transient and every other refused —
    /// and a partial answer, like a copy's delivered tickets, is refused only where every
    /// failure is.
    ///
    /// Each failure is built as the store's own typed value and read through the functions
    /// the worker reads it through; no message is read to decide, which the pair of refusals
    /// below with the same words and different classes holds.
    #[test]
    fn the_divergence_records_refusal_rule_is_the_one_the_worker_classifies_by() {
        let block = divergence_block("72.");
        let rule = &block["failure"];
        assert_eq!(
            rule["commands"],
            json!([super::PROJECT_SHOW, super::TASK_SHOW, super::PROJECT_COPY]),
            "entry 72 names other calls than the three an attempt makes"
        );
        assert_eq!(
            rule["stops_the_timer"].as_str(),
            Some(FailureClass::Refused.as_str())
        );
        let transient: Vec<&str> = rule["transient"]
            .as_array()
            .expect("entry 72 names the kinds a wait can change")
            .iter()
            .filter_map(Value::as_str)
            .collect();
        let words = "the store's own words".to_owned();
        let every_source_error = [
            SourceError::Config {
                message: words.clone(),
            },
            SourceError::Auth {
                message: words.clone(),
            },
            SourceError::Refused {
                message: words.clone(),
            },
            SourceError::RateLimited {
                retry_after_seconds: Some(30),
                message: Some(words.clone()),
            },
            SourceError::Unavailable {
                message: words.clone(),
            },
            SourceError::Malformed {
                message: words.clone(),
            },
        ];
        for error in &every_source_error {
            let classified = Classified::of_source(error);
            // The kind is the store's own wire tag for the error.
            assert_eq!(
                json!(classified.kind),
                serde_json::to_value(error).expect("a source error serializes")["kind"],
                "{error:?}"
            );
            let expected = if transient.contains(&classified.kind.as_str()) {
                FailureClass::Transient
            } else {
                FailureClass::Refused
            };
            assert_eq!(classified.class, expected, "{error:?}");
        }

        // An engine failure wrapping a source's is that source's; one the engine decided on
        // its own no retry changes.
        let rate_limited = SourceError::RateLimited {
            retry_after_seconds: None,
            message: None,
        };
        let failed = EngineError::SourceFailed {
            name: "plans".to_owned(),
            error: rate_limited,
        };
        assert_eq!(
            Classified::of_engine(&failed),
            Classified {
                class: FailureClass::Transient,
                kind: "rate-limited".to_owned()
            }
        );
        let refused = EngineError::SourceRefused {
            name: "plans".to_owned(),
            error: SourceError::Refused {
                message: words.clone(),
            },
        };
        assert_eq!(Classified::of_engine(&refused).class, FailureClass::Refused);
        let undone = EngineError::CopyNotUndone {
            error: Box::new(failed.clone()),
            left_behind: onetaskgraph_core::LeftBehind::new(
                "plans:board/1".parse::<GlobalId>().expect("an id"),
            ),
            refusal: SourceError::Refused {
                message: words.clone(),
            },
        };
        assert_eq!(
            Classified::of_engine(&undone).class,
            FailureClass::Transient,
            "a copy that could not be undone is classed by the failure it wraps"
        );
        let stale = EngineError::StaleOrigin {
            item: "a".to_owned(),
            origin: "b".to_owned(),
        };
        assert_eq!(
            Classified::of_engine(&stale),
            Classified {
                class: FailureClass::Refused,
                kind: "stale-origin".to_owned()
            }
        );

        let partial = &rule["partial_answer"];
        assert_eq!(
            partial["refused_when"].as_str(),
            Some("every"),
            "entry 72 states a partial-answer rule other than the one the worker applies"
        );
        let of =
            |errors: &[SourceError]| Classified::of_all(errors.iter().map(Classified::of_source));
        assert_eq!(
            of(&[SourceError::Config {
                message: words.clone()
            }]),
            Some(Classified {
                class: FailureClass::Refused,
                kind: "config".to_owned()
            })
        );
        assert_eq!(
            of(&[
                SourceError::Config {
                    message: words.clone()
                },
                SourceError::Unavailable {
                    message: words.clone()
                },
            ]),
            Some(Classified {
                class: FailureClass::Transient,
                kind: "config, unavailable".to_owned(),
            }),
            "a partial answer with one entry a wait could change was read as refused"
        );
        assert_eq!(of(&[]), None, "an answer naming no failure was classified");
        assert_eq!(
            block["failure"]["delivered"]["refused_when"].as_str(),
            Some("every")
        );
    }

    /// Every failure the engine can answer with is named, and its kind is the one the store's
    /// own failure document gives it: the worker's exhaustive match and the store's own
    /// rendering cannot drift apart without this failing.
    #[test]
    fn every_engine_failure_is_named_as_the_store_names_it() {
        let words = || "the store's own words".to_owned();
        let source = SourceError::Unavailable { message: words() };
        let id = || "plans:board/1".parse::<GlobalId>().expect("an id");
        let failures = [
            EngineError::UnknownSource {
                name: "x".into(),
                configured: "plans".into(),
            },
            EngineError::Token { message: words() },
            EngineError::NoSources,
            EngineError::NotWritable {
                name: "plans".into(),
                kind: "in-memory".into(),
            },
            EngineError::NoDocuments {
                name: "plans".into(),
                kind: "in-memory".into(),
            },
            EngineError::NoComments {
                name: "plans".into(),
                kind: "in-memory".into(),
            },
            EngineError::CommentsNotWritable {
                name: "plans".into(),
                kind: "in-memory".into(),
            },
            EngineError::StatusNotWritable {
                name: "plans".into(),
                kind: "in-memory".into(),
            },
            EngineError::NoSuchProject {
                id: "plans:a".into(),
            },
            EngineError::NoSuchDocument {
                id: "plans:a".into(),
            },
            EngineError::NoSuchTask {
                id: "plans:a".into(),
            },
            EngineError::NoSuchComment {
                task: "plans:a".into(),
                comment: "c".into(),
            },
            EngineError::SourceUnavailable {
                name: "plans".into(),
                error: source.clone(),
            },
            EngineError::SourceFailed {
                name: "plans".into(),
                error: source.clone(),
            },
            EngineError::DestinationUnavailable {
                name: "plans".into(),
                error: source.clone(),
            },
            EngineError::NoSuchItem {
                id: "plans:a".into(),
            },
            EngineError::StaleOrigin {
                item: "a".into(),
                origin: "b".into(),
            },
            EngineError::NotAMember {
                id: id(),
                projects: vec![id()],
            },
            EngineError::SourceRefused {
                name: "plans".into(),
                error: source.clone(),
            },
            EngineError::MetadataNotWritable {
                name: "plans".into(),
                kind: "in-memory".into(),
                record: onetaskgraph_plugin_api::MetadataRecord::Task,
            },
            EngineError::PriorityNotWritable {
                name: "plans".into(),
                kind: "in-memory".into(),
            },
            EngineError::ContentNotWritable {
                name: "plans".into(),
                kind: "in-memory".into(),
            },
            EngineError::NoPriority {
                name: "plans".into(),
                kind: "in-memory".into(),
                task: "plans:a".into(),
                priority: onetaskgraph_plugin_api::Priority::High,
            },
            EngineError::UnrecordedMember {
                item: id(),
                member: id(),
                destination: onetaskgraph_plugin_api::SourceName::new("plans")
                    .expect("a source name"),
            },
            // A copy that could not be undone takes the class and kind of the failure it wraps.
            EngineError::CopyNotUndone {
                error: Box::new(EngineError::SourceRefused {
                    name: "plans".into(),
                    error: SourceError::Refused { message: words() },
                }),
                left_behind: onetaskgraph_core::LeftBehind::new(id()),
                refusal: source.clone(),
            },
        ];
        for failure in &failures {
            let classified = Classified::of_engine(failure);
            let store = onetaskgraph_core::Failure::from(failure);
            assert_eq!(classified.kind, store.kind(), "{failure:?}");
            assert_eq!(
                classified.class,
                store.class().into(),
                "{failure:?} is classed otherwise than the store classes it"
            );
        }
    }

    /// A build thread that ends without sending — which only a panic does — is its own
    /// failure, answered at once, and never reported as the store outlasting the deadline.
    #[test]
    fn a_store_build_that_ends_without_an_answer_is_not_a_timeout() {
        let (built, arrived) = std::sync::mpsc::channel::<()>();
        std::thread::spawn(move || {
            let _held = built;
            panic!("the build stopped short");
        })
        .join()
        .expect_err("the build panicked");
        let started = std::time::Instant::now();
        let failed = super::opened_by(&arrived, Deadline::Floor).expect_err("nothing arrived");
        assert!(
            started.elapsed() < super::COMMAND_FLOOR,
            "the floor was waited out"
        );
        assert!(
            failed.reason.contains("ended without an answer")
                && !failed.reason.contains("exceeded"),
            "{}",
            failed.reason
        );
        assert_eq!(failed.classified, None);

        let (built, arrived) = std::sync::mpsc::channel();
        built.send(7).expect("the receiver is held");
        assert_eq!(super::opened_by(&arrived, Deadline::Floor).ok(), Some(7));
    }

    #[test]
    fn returning_to_the_last_success_supersedes_a_different_pending_snapshot() {
        let first = snapshot(NodeStatus::Pending);
        let superseded = snapshot(NodeStatus::Running);
        let mut pending = Pending {
            latest: Some(superseded),
            last_success: Some(first.clone()),
            refused: None,
            attempts: 0,
            queued_items: 0,
            worker: WorkerState::Working,
            phase: super::RunPhase::Running,
            unprojected: Vec::new(),
        };

        assert!(pending.queue(first.clone()));
        assert!(pending.latest.as_ref() == Some(&first));
    }

    /// A driver that goes on driving after a close-out puts the retry schedule back.
    ///
    /// The close-out suspends it so a terminal snapshot is not left sitting out a
    /// minute's backoff inside a two-second window, which is right for a run that is
    /// ending and wrong for one that is not: an edit claimed on the way out sends the
    /// loop round again, and a projection that keeps failing under a suspended schedule
    /// is retried as fast as the store can refuse it. Asserted on the phase itself
    /// because that is the whole of what the two schedules differ by.
    #[test]
    fn the_close_out_phase_is_lifted_when_the_run_is_driven_again() {
        let writeback = Writeback {
            pending: Arc::new((Mutex::new(Pending::default()), Condvar::new())),
            per_item: DEFAULT_WRITEBACK_ITEM_BUDGET_SECONDS,
        };
        writeback.wait_briefly();
        assert!(
            writeback.pending.0.lock().expect("the state").phase == super::RunPhase::ClosingOut,
            "the close-out did not put the run into its closing-out phase"
        );

        writeback.driving_again();
        assert!(
            writeback.pending.0.lock().expect("the state").phase == super::RunPhase::Running,
            "a run driven on after a close-out is still in its closing-out phase, so a \
             failing projection is retried with no interval at all"
        );
    }

    /// A run's own end, arriving while the worker is waiting out a retry interval, is
    /// honoured then rather than once the interval nobody is waiting for has run out.
    ///
    /// In this process on purpose. The stop an operator types signals the whole process
    /// tree, so the journey beside this one — `a_stop_during_a_long_retry_interval_is_...`
    /// in `tests/e2e/store.rs` — cannot tell a worker that left because it was told from
    /// one the kill took with it whatever it was doing. Proving the worker's own half means
    /// watching it *after* its stop is requested, and only something holding this process
    /// open across that request can.
    ///
    /// What it watches is real: the thread [`Writeback::start`] spawns, the schedule that
    /// thread keeps, and its own reference to the state it shares — which it lets go of
    /// when, and only when, `worker` returns. The destination fails the way an unreachable
    /// one does: the run's own `onetaskgraph.yaml` names a source whose plugin program is
    /// not installed, which the linked store answers `unavailable` — a failure a wait can
    /// change, so the worker keeps retrying it on the schedule.
    #[test]
    fn a_stop_reaches_a_worker_that_is_waiting_out_a_retry_interval() {
        let dir = scratch("stop-mid-wait");
        std::fs::write(
            dir.join("onetaskgraph.yaml"),
            format!(
                "sources:\n  plans:\n    plugin: subprocess\n    config:\n      command: {:?}\n",
                dir.join("onetaskgraph-plugin-nobody-installed")
            ),
        )
        .expect("the run's store configuration is written");
        let paths = RunPaths {
            run: "stopmidwait".to_owned(),
            dir: dir.to_path_buf(),
        };
        let launch = a_launch(&paths);
        let writeback = Writeback::start(&paths, &launch).expect("a write-back worker");
        // One node, so there is something to carry: a run with nothing to project asks the
        // store nothing at all.
        let plan = crate::plan::Plan {
            schema_version: crate::plan::PLAN_SCHEMA_VERSION,
            goal: None,
            name: Some("stop-mid-wait".into()),
            concurrency: 1,
            tasks: vec![serde_json::from_value(json!({"id": "node"})).expect("a node")],
        };
        let state = RunState {
            graph: crate::graph::Graph::from_plan(&plan),
            plan: Some(plan),
            ..RunState::default()
        };
        writeback.publish(&paths, &launch, &state, &BTreeMap::new());

        // Four attempts in, the interval the worker is now waiting out is longer than every
        // one before it — so the last one this test actually watched is a lower bound on
        // what a stop would have to sit through if it were not honoured.
        let waited = intervals_between_attempts(&paths, 4);
        let outstanding = *waited.last().expect("an interval between attempts");
        assert!(
            outstanding >= Duration::from_secs(2),
            "the streak is not deep enough for a stop to have anything to wait out: {waited:?}"
        );
        let recorded = attempts_recorded(&paths);
        assert!(
            std::fs::read_to_string(paths.dir.join(crate::cli::WRITEBACK_PROJECTIONS_FILE))
                .expect("the record")
                .lines()
                .all(|line| line.contains("\"class\":\"transient\"")),
            "an unreachable store was not classed transient"
        );

        let shared = Arc::clone(&writeback.pending);
        let asked = Instant::now();
        drop(writeback);
        while Arc::strong_count(&shared) > 1 {
            assert!(
                asked.elapsed() < outstanding,
                "the worker was still running {:?} after its stop was requested, with an \
                 interval of at least {outstanding:?} outstanding",
                asked.elapsed()
            );
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(
            attempts_recorded(&paths),
            recorded,
            "the worker asked the destination again on its way out"
        );
    }

    /// How many attempts the worker has recorded: one line of the projection record each,
    /// appended as the attempt ends.
    fn attempts_recorded(paths: &RunPaths) -> usize {
        std::fs::read_to_string(paths.dir.join(crate::cli::WRITEBACK_PROJECTIONS_FILE))
            .map(|record| record.lines().count())
            .unwrap_or(0)
    }

    /// The wall-clock intervals between the worker's next `count` attempts, each read off
    /// the line the attempt appended to the run's projection record.
    fn intervals_between_attempts(paths: &RunPaths, count: usize) -> Vec<Duration> {
        let deadline = Instant::now() + Duration::from_secs(60);
        let mut at: Vec<Instant> = Vec::new();
        let mut seen = attempts_recorded(paths);
        while at.len() < count {
            assert!(
                Instant::now() < deadline,
                "the worker made {} attempts, not the {count} this test reads its intervals \
                 off",
                at.len()
            );
            let now = attempts_recorded(paths);
            if now > seen {
                at.push(Instant::now());
                seen = now;
            } else {
                std::thread::sleep(Duration::from_millis(5));
            }
        }
        at.windows(2).map(|pair| pair[1] - pair[0]).collect()
    }

    /// A launch record naming a project to project into, and nothing else this worker reads.
    fn a_launch(paths: &RunPaths) -> LaunchRecord {
        serde_json::from_value(json!({
            "run_id": paths.run,
            "project": "plans:stop-mid-wait",
            "dir": paths.dir,
            "launcher": "test",
            "session": "test",
            "pid": crate::sys::pid(),
            "host": "test",
            "started": "linux-proc-stat:1",
            "started_at": "2026-09-01T00:00:00Z",
            "heartbeat_interval": 1_800,
        }))
        .expect("a launch record")
    }

    /// The launch wait is bounded by the copy deadline of the snapshot the driver queued, even
    /// once the worker has taken that snapshot to project it — which it may do before the driver
    /// asks how long to wait. A bound read off the queue after that take counts no nodes and
    /// falls back to the floor, so a plan large enough to lift its copy's deadline would be
    /// dispatched while its claim was still inside it.
    #[test]
    fn the_launch_wait_is_bounded_by_what_was_queued_after_the_worker_takes_it() {
        let per_item = NonZeroU64::new(10).expect("a budget");
        let nodes: BTreeMap<String, Node> = (0..12)
            .map(|index| {
                let id = format!("n{index}");
                let node = serde_json::from_value(json!({"id": id})).expect("a node");
                (id, node)
            })
            .collect();
        let snapshot = Fixture::new("launch-wait").snapshot_with(|snapshot| snapshot.nodes = nodes);
        let mut pending = Pending::default();
        assert!(pending.queue(snapshot), "the first snapshot is queued");
        // The worker takes it to project it before the driver asks how long to wait.
        assert!(pending.latest.take().is_some());
        assert_eq!(
            super::launch_wait(per_item, &pending),
            Duration::from_secs(60 + 120),
            "the launch wait did not follow the floor + 12 × 10 second deadline of the queued \
             copy"
        );
    }

    /// A driven unstarted node is written `queued` and its shadow task carries the tickets it
    /// delivers in the store's own `delivers` field; a closeout writes it `todo`. No reserved
    /// key carries the tickets on either side: the linked store always offers both.
    #[test]
    fn an_unstarted_node_is_queued_while_driven_and_carries_what_it_delivers() {
        for (claim, word) in [(Claim::Held, "queued"), (Claim::Released, "todo")] {
            let snapshot = Fixture::new("delivers").snapshot_with(|snapshot| {
                snapshot.claim = claim;
                snapshot
                    .statuses
                    .insert("design".to_owned(), NodeStatus::Ready);
                snapshot
                    .nodes
                    .get_mut("design")
                    .expect("the fixture holds the node")
                    .delivers = vec!["tickets:t-1".to_owned()];
            });
            let (front, _) = super::task_document(&snapshot, &snapshot.lineages(), "design", None)
                .expect("the shadow task renders");
            assert_eq!(front["status"], word, "{claim:?}");
            assert_eq!(front.get("delivers").cloned(), Some(json!(["tickets:t-1"])));
            assert!(
                front["metadata"].get("onepipeline.delivers").is_none(),
                "a reserved key carries the tickets: {front}"
            );
        }
    }

    /// A copy report's failed tickets are named with their deliverer and the store's words on
    /// one line, a report failing no ticket describes none, and the class of each is the
    /// store's own: refused only where every failed ticket's is.
    #[test]
    fn a_delivered_report_names_each_failed_ticket_and_is_classed_by_every_one() {
        use super::failed_deliveries;
        // Built through the store's own types, so each failure's class is the one the store
        // decides for its cause rather than one written into a document here.
        let failure = |error: SourceError| {
            onetaskgraph_core::Failure::from(&EngineError::SourceRefused {
                name: "tickets".into(),
                error,
            })
        };
        let a_refusal = || {
            failure(SourceError::Refused {
                message: "cannot write\nnext: fix it".to_owned(),
            })
        };
        let rate_limited = || {
            failure(SourceError::RateLimited {
                retry_after_seconds: None,
                message: Some("slow down".to_owned()),
            })
        };
        let id = |id: &str| id.parse::<GlobalId>().expect("an id");
        let entry =
            |ticket: &str, failure: onetaskgraph_core::Failure| onetaskgraph_core::Delivered {
                ticket: id(ticket),
                deliverer: id("plans:p/a"),
                outcome: onetaskgraph_core::DeliveryOutcome::Failed {
                    from: Some(StatusCategory::Queued),
                    failure,
                },
                pruned: Vec::new(),
            };
        let refused = vec![entry("tickets:t/one", a_refusal())];
        let named = failed_deliveries(&refused).expect("a failed ticket is named");
        assert!(
            named.starts_with("ticket tickets:t/one (delivered by plans:p/a): ")
                && named.contains("cannot write next: fix it")
                && !named.contains('\n'),
            "the failed ticket is not named on one line with the store's words: {named}"
        );
        let classed = |entries: &[onetaskgraph_core::Delivered]| {
            Classified::of_all(entries.iter().filter_map(|entry| match &entry.outcome {
                onetaskgraph_core::DeliveryOutcome::Failed { failure, .. } => {
                    Some(Classified::of_delivery(failure))
                }
                _ => None,
            }))
        };
        assert_eq!(
            classed(&refused),
            Some(Classified {
                class: FailureClass::Refused,
                kind: "refused".to_owned()
            })
        );
        let mixed = vec![
            entry("tickets:t/one", a_refusal()),
            entry("tickets:t/two", rate_limited()),
        ];
        assert_eq!(
            classed(&mixed),
            Some(Classified {
                class: FailureClass::Transient,
                kind: "refused, rate-limited".to_owned(),
            })
        );
        let written = vec![onetaskgraph_core::Delivered {
            ticket: id("tickets:t/two"),
            deliverer: id("plans:p/a"),
            outcome: onetaskgraph_core::DeliveryOutcome::Written {
                from: StatusCategory::Todo,
                to: StatusCategory::Queued,
            },
            pruned: Vec::new(),
        }];
        assert_eq!(
            failed_deliveries(&written),
            None,
            "a report failing no ticket described one"
        );
    }

    /// Every word this projection writes, in the one arrangement that states them:
    /// the list, and an exhaustive match over it, so a ninth status stops this
    /// compiling until it is named here too.
    fn every_projected_word() -> Vec<String> {
        [
            ProjectedStatus::Todo,
            ProjectedStatus::Queued,
            ProjectedStatus::InProgress,
            ProjectedStatus::Done,
            ProjectedStatus::Failed,
            ProjectedStatus::ProviderFailed,
            ProjectedStatus::Cancelled,
            ProjectedStatus::Parked,
            ProjectedStatus::Skipped,
        ]
        .into_iter()
        .inspect(|status| match status {
            ProjectedStatus::Todo
            | ProjectedStatus::Queued
            | ProjectedStatus::InProgress
            | ProjectedStatus::Done
            | ProjectedStatus::Failed
            | ProjectedStatus::ProviderFailed
            | ProjectedStatus::Cancelled
            | ProjectedStatus::Parked
            | ProjectedStatus::Skipped => {}
        })
        .map(word)
        .collect()
    }

    /// The projection writes a vocabulary wider than the approved contract fixes,
    /// and four reserved keys beside the settlement it already carried. Both are
    /// recorded as a divergence, and this is the gate that keeps the record and
    /// the code from parting: a word or a key the document does not name is one
    /// nobody proposed.
    #[test]
    fn every_word_and_key_this_projection_writes_is_named_by_the_divergence() {
        let divergence = include_str!("../docs/contract-divergences.md");
        let words = every_projected_word();
        for word in &words {
            assert!(
                divergence.contains(&format!("`{word}`")),
                "docs/contract-divergences.md does not name the projected status `{word}`"
            );
        }
        let keys = [
            LANDING_KEY,
            LANDING_COMMIT_KEY,
            CHANGE_URL_KEY,
            LANDING_EVIDENCE_KEY,
        ];
        for key in keys {
            assert!(
                divergence.contains(&format!("`{key}`")),
                "docs/contract-divergences.md does not name the reserved key `{key}`"
            );
        }
        // The task fields a shadow task carries at its top level because the plan declares
        // them, beyond the title, body, status and edges the contract already names.
        let fields = [DELIVERS_FIELD];
        for field in fields {
            assert!(
                divergence.contains(&format!("`{field}`")),
                "docs/contract-divergences.md does not name the task field `{field}`"
            );
        }
        // The keys that say which attempt of a lineage an item carries, beside the
        // `onepipeline.id` the contract already names — entry 80's.
        let lineage = [NODE_KEY, SUPERSEDES_KEY];
        for key in lineage {
            assert!(
                divergence.contains(&format!("`{key}`")),
                "docs/contract-divergences.md does not name the lineage key `{key}`"
            );
        }
        // And how many of each, because naming them all is not the same as
        // counting them: a document that lists eight words and then says six has
        // one sentence a reader trusts and one that is wrong.
        for stated in [
            format!("{} words where the contract names 5", words.len()),
            format!("the {} reserved keys beside the settlement", keys.len()),
            format!("the {} task field the write-back owns", fields.len()),
            format!("the {} lineage keys beside `{ID_KEY}`", lineage.len()),
        ] {
            assert!(
                divergence.contains(&stated),
                "docs/contract-divergences.md does not say \"{stated}\""
            );
        }
    }

    /// The four words that *are* onetaskgraph's own normalised categories stay
    /// the words the approved contract names them with.
    ///
    /// The other four are deliberately not here: they are names that vocabulary
    /// has none of, which `docs/contract-divergences.md` records as a divergence
    /// rather than the contract naming them.
    #[test]
    fn the_projected_categories_remain_named_by_the_approved_contract() {
        let contract = include_str!("../docs/contract.md");
        for status in [
            ProjectedStatus::Todo,
            ProjectedStatus::Queued,
            ProjectedStatus::InProgress,
            ProjectedStatus::Done,
            ProjectedStatus::Cancelled,
        ] {
            let native = word(status).replace(' ', "-");
            assert!(
                contract.contains(&format!("`{native}`")),
                "docs/contract.md no longer names the projected status category `{native}`"
            );
        }
    }

    /// Each settlement is projected under a word of its own.
    ///
    /// The defect this replaces put `done` on a node whose work was thrown away
    /// — the same word a merged node got, with only a nested `outcome` to
    /// disagree — and made a parked node indistinguishable from a cancelled one.
    /// So the assertion is that no two of these share a word, which is a
    /// property a mapping that collapsed any pair could not have.
    #[test]
    fn every_settlement_is_projected_under_a_word_of_its_own() {
        let settlements = [
            ("done", NodeStatus::Done, None),
            ("a task the agent failed", NodeStatus::Failed, None),
            (
                "a dispatch its provider killed",
                NodeStatus::Failed,
                Some(crate::engine::PROVIDER_FAILED),
            ),
            ("cancelled", NodeStatus::Cancelled, None),
            ("parked", NodeStatus::Parked, None),
        ];
        let mut seen: BTreeMap<String, &str> = BTreeMap::new();
        for (settlement, status, outcome) in settlements {
            let projected = word(projected(status, outcome, Claim::Held));
            if let Some(shared) = seen.insert(projected.clone(), settlement) {
                panic!(
                    "'{settlement}' and '{shared}' are both projected as `{projected}`, so a \
                     board cannot tell them apart"
                );
            }
        }
        assert_eq!(
            seen.keys().cloned().collect::<Vec<_>>(),
            ["cancelled", "done", "failed", "parked", "provider-failed"],
        );
    }

    /// The word one projected status is written as, taken through the
    /// serializer that writes it rather than restated beside it.
    fn word(status: ProjectedStatus) -> String {
        match serde_json::to_value(status).expect("a projected status serializes") {
            Value::String(word) => word,
            other => panic!("a projected status is a string, not {other}"),
        }
    }

    static NEXT_SCRATCH: AtomicU64 = AtomicU64::new(0);

    struct Scratch(PathBuf);

    impl std::ops::Deref for Scratch {
        type Target = Path;

        fn deref(&self) -> &Self::Target {
            &self.0
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn scratch(name: &str) -> Scratch {
        let dir = std::env::temp_dir().join(format!(
            "onepipeline-writeback-{name}-{}-{}",
            crate::sys::pid(),
            NEXT_SCRATCH.fetch_add(1, Ordering::Relaxed),
        ));
        std::fs::create_dir(&dir).expect("a unique scratch directory");
        Scratch(dir)
    }

    #[test]
    fn concurrent_scratch_directories_with_the_same_name_are_independent() {
        let first = std::thread::spawn(|| scratch("concurrent-same-name"));
        let second = std::thread::spawn(|| scratch("concurrent-same-name"));
        let first = first.join().expect("the first scratch directory");
        let second = second.join().expect("the second scratch directory");

        assert_ne!(&*first, &*second, "parallel invocations shared a directory");
        drop(first);
        assert!(
            second.is_dir(),
            "removing one invocation removed the other's directory"
        );
    }

    /// The reserved keys this worker owns and overwrites on a projected item.
    ///
    /// Everything else on a destination item is the complement — what the
    /// projection has to carry through unchanged — and [`preserved`] is that
    /// complement read off either side of the write.
    fn owned(key: &str) -> bool {
        key.starts_with("onepipeline.") || key.starts_with("onetaskgraph.")
    }

    /// Everything on a projected document that the writer does **not** own.
    ///
    /// The complement of what it declares, taken as one value rather than field
    /// by field: "did the new thing arrive?" and "did anything else leave?" are
    /// different questions, and only an assertion shaped like this one can fail
    /// on the second.
    fn preserved(front: &Value, body: &str) -> Value {
        let metadata: Map<String, Value> = front["metadata"]
            .as_object()
            .expect("a projected document carries metadata")
            .iter()
            .filter(|(key, _)| !owned(key))
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect();
        json!({
            "title": front["title"],
            "labels": front["labels"],
            "metadata": metadata,
            "body": body,
        })
    }

    /// The same complement, read off the destination the projection was written
    /// against.
    fn preserved_of(destination: &Project) -> Value {
        let metadata: Map<String, Value> = destination
            .metadata
            .iter()
            .filter(|(key, _)| !owned(key))
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect();
        json!({
            "title": destination.title,
            "labels": destination.labels,
            "metadata": metadata,
            "body": destination.content.clone().unwrap_or_default(),
        })
    }

    fn written(path: &Path) -> (Value, String) {
        let document = std::fs::read_to_string(path)
            .unwrap_or_else(|error| panic!("{} was not written: {error}", path.display()));
        let (front, body) = document
            .strip_prefix("---\n")
            .expect("a projected document opens its front matter")
            .split_once("---\n")
            .expect("a projected document closes its front matter");
        (
            serde_norway::from_str(front).expect("the front matter is YAML"),
            body.to_owned(),
        )
    }

    /// One store fixture: a destination project, the snapshot projected onto it,
    /// and what that destination already holds for each node.
    ///
    /// Held together rather than assembled per test because the properties that
    /// make it able to fail are properties of the three *together* — see
    /// [`undiscriminating`].
    struct Fixture {
        name: &'static str,
        dir: Scratch,
        snapshot: Snapshot,
        origins: BTreeMap<String, Origin>,
        destination: Project,
    }

    impl Fixture {
        /// Two nodes, of which exactly one has a destination task; a project
        /// whose title is not the identifier the store holds it under; and
        /// authored labels, content and metadata on both sides.
        fn new(name: &'static str) -> Self {
            let dir = scratch(name);
            let node = |id: &str, deps: &[&str]| -> Node {
                serde_json::from_value(json!({
                    "id": id,
                    "title": format!("feat: {id} it"),
                    "task": format!("## What\n{id} it."),
                    "persona": "engineer",
                    "deps": deps,
                }))
                .expect("a plan node")
            };
            Self {
                name,
                snapshot: Snapshot {
                    project: "plans:board".parse().expect("a qualified project"),
                    dir: dir.to_path_buf(),
                    nodes: BTreeMap::from([
                        ("build".to_owned(), node("build", &["design"])),
                        ("design".to_owned(), node("design", &[])),
                    ]),
                    statuses: BTreeMap::from([
                        ("build".to_owned(), NodeStatus::Done),
                        ("design".to_owned(), NodeStatus::Done),
                    ]),
                    outcomes: BTreeMap::new(),
                    landings: BTreeMap::new(),
                    landing_commits: BTreeMap::new(),
                    change_urls: BTreeMap::new(),
                    stated_landings: BTreeMap::new(),
                    settlements: BTreeMap::new(),
                    superseded: BTreeMap::new(),
                    project_metadata: BTreeMap::from([(
                        "onepipeline.concurrency".into(),
                        json!(4),
                    )]),
                    claim: Claim::Held,
                },
                // Only `build`. A node the plan has just added has no destination
                // task at all, and holding both kinds in one fixture is what makes
                // "carried through" and "invented none" separable answers.
                origins: BTreeMap::from([(
                    "build".to_owned(),
                    Origin {
                        id: "plans:board/002-build".parse().expect("a qualified task"),
                        labels: labels(&[("needs-review", Some("d73a4a"))]),
                        category: Some(StatusCategory::Done),
                    },
                )]),
                destination: destination("A person's own board", &["planning", "q3"]),
                dir,
            }
        }

        /// The same fixture with one field of the snapshot restated.
        fn snapshot_with(mut self, edit: impl FnOnce(&mut Snapshot)) -> Snapshot {
            edit(&mut self.snapshot);
            self.snapshot.clone()
        }

        /// Project it, refusing a fixture that could not tell a right answer from
        /// a wrong one.
        fn project(&self) {
            if let Some(missing) =
                undiscriminating(self.name, &self.destination, &self.snapshot, &self.origins)
            {
                panic!("{missing}");
            }
            write_shadow(&self.snapshot, &self.origins, &self.destination)
                .expect("the shadow project is written");
        }

        /// The project document and one node's task document, as written.
        fn project_document(&self) -> (Value, String) {
            written(&self.dir.join("projects").join(format!(
                "{}.md",
                super::project_file(&self.snapshot.project)
            )))
        }

        fn task_document(&self, node: &str) -> (Value, String) {
            written(
                &self
                    .dir
                    .join("tasks")
                    .join(super::project_file(&self.snapshot.project))
                    .join(format!("{}.md", super::task_file(node))),
            )
        }
    }

    /// Why a fixture standing in for a store could not fail, or `None` when it
    /// can.
    ///
    /// Two rules, enforced where the fixture is built rather than trusted to be
    /// remembered per test, because each of them is a defect that shipped
    /// through a worker, a judge, an observer and a manager. Under a store whose
    /// project's native id *is* its title, writing the identifier as the title
    /// was byte-identical to preserving it; under a fixture holding one of
    /// everything the code filters on, an ignored filter and an honoured one
    /// produced the same bytes.
    fn undiscriminating(
        fixture: &str,
        destination: &Project,
        snapshot: &Snapshot,
        origins: &BTreeMap<String, Origin>,
    ) -> Option<String> {
        let missing = if snapshot.project.native() == destination.title {
            "its destination project's identifier is its own title, so writing the \
             identifier and preserving the title are the same bytes"
        } else if snapshot.nodes.len() < 2 {
            "it holds one node, so a projection that wrote the wrong one is \
             indistinguishable from one that wrote the right one"
        } else if !snapshot.nodes.keys().any(|id| origins.contains_key(id))
            || !snapshot.nodes.keys().any(|id| !origins.contains_key(id))
        {
            "every node has a destination task or none does, so a projection that \
             ignored what the destination already holds cannot be told from one that \
             read it"
        } else {
            return None;
        };
        Some(format!("fixture '{fixture}': {missing}"))
    }

    /// The rule the fixtures are built to, held against fixtures that break it.
    ///
    /// A check that passed a one-project, id-equals-title fixture would be the
    /// thing it exists to prevent, so each degenerate shape is stated here and
    /// the check has to name it.
    #[test]
    fn a_fixture_that_could_not_tell_a_right_answer_from_a_wrong_one_is_refused() {
        let sound = Fixture::new("discrimination");
        assert_eq!(
            undiscriminating(
                sound.name,
                &sound.destination,
                &sound.snapshot,
                &sound.origins
            ),
            None,
            "the fixture every test here is built from does not meet its own rule"
        );

        let named = |missing: Option<String>, property: &str| {
            let said = missing.unwrap_or_else(|| {
                panic!("a fixture whose {property} could prove nothing was accepted")
            });
            assert!(
                said.contains("fixture 'degenerate'"),
                "the refusal does not name the fixture: {said}"
            );
            assert!(
                said.contains(property),
                "the refusal does not name the missing property: {said}"
            );
        };

        // A project whose title is the store's own identifier for it.
        named(
            undiscriminating(
                "degenerate",
                &destination("board", &[]),
                &sound.snapshot,
                &sound.origins,
            ),
            "identifier is its own title",
        );
        // One node, so an ignored project or node filter is invisible.
        let one = Fixture::new("one-node").snapshot_with(|snapshot| {
            snapshot.nodes.retain(|id, _| id == "build");
        });
        named(
            undiscriminating("degenerate", &sound.destination, &one, &sound.origins),
            "holds one node",
        );
        // Every node already known to the destination, so a projection that never
        // read it looks the same.
        named(
            undiscriminating(
                "degenerate",
                &sound.destination,
                &sound.snapshot,
                &BTreeMap::new(),
            ),
            "every node has a destination task or none does",
        );
    }

    /// The destination project a projection is written against.
    ///
    /// Built as the store's own `Project` value, which is what the projection reads.
    fn destination(title: &str, labels: &[&str]) -> Project {
        serde_json::from_value(json!({
            "id": "board",
            "title": title,
            "content": "A person's own description.",
            "status": {"category": "backlog", "name": "backlog"},
            "labels": labels
                .iter()
                .map(|name| json!({"id": name, "name": name, "color": null}))
                .collect::<Vec<_>>(),
            "url": null,
            "created_at": null,
            "updated_at": null,
            "metadata": {"authored.note": "keep this value", "authored.owner": "a person"},
            "repositories": [],
        }))
        .expect("the store's own project")
    }

    fn labels(named: &[(&str, Option<&str>)]) -> Vec<Label> {
        serde_json::from_value(json!(named
            .iter()
            .map(|(name, color)| json!({"id": name, "name": name, "color": color}))
            .collect::<Vec<_>>()))
        .expect("the store's own labels")
    }

    /// The rule's first consequence: everything a plan declares is replaced,
    /// which is what the projection is for.
    #[test]
    fn a_projection_replaces_every_field_the_plan_declares() {
        let mut fixture = Fixture::new("declared");
        fixture
            .snapshot
            .statuses
            .insert("build".to_owned(), NodeStatus::Running);
        fixture.project();

        let (front, body) = fixture.task_document("build");
        assert_eq!(front["title"], "feat: build it");
        assert_eq!(body, "## What\nbuild it.");
        assert_eq!(front["status"], "in progress");
        assert_eq!(
            front["depends_on"],
            json!([format!(
                "{}/{}",
                super::project_file(&fixture.snapshot.project),
                super::task_file("design")
            )]),
            "the projection lost the plan's dependency edge, or named its far end \
             the way no store resolves to a task of this project"
        );
        assert_eq!(front["metadata"]["onepipeline.id"], "build");
        assert_eq!(front["metadata"]["onepipeline.persona"], "engineer");
        // And the node the destination has no task for is written too, under its
        // own title rather than the other node's.
        let (design, _) = fixture.task_document("design");
        assert_eq!(design["title"], "feat: design it");
        assert_eq!(design["metadata"]["onepipeline.id"], "design");
    }

    /// A write-back pass publishes each shadow document by its rename alone: the
    /// journal rebuilds the shadow store, and a sync per document is what pushed the
    /// loopcost scale journey past its bound.
    #[test]
    fn a_shadow_projection_asks_the_disk_for_renames_and_no_syncs() {
        use crate::ledger::disk::{watching, Step};

        let fixture = Fixture::new("unsynced");
        let watch = watching(None);
        fixture.project();
        let asked = watch.asked();
        drop(watch);

        // The project document and one task document per node.
        let documents = 1 + fixture.snapshot.nodes.len();
        assert_eq!(
            asked,
            vec![Step::Publish; documents],
            "the shadow projection synced a document it only has to publish"
        );
        assert_eq!(fixture.task_document("build").0["title"], "feat: build it");
    }

    /// A node carrying no title is written under its **id**, and a blank one is
    /// the same absence spelled differently.
    #[test]
    fn a_node_with_no_title_is_projected_under_its_id() {
        let mut fixture = Fixture::new("untitled");
        for (id, title) in [("build", Value::Null), ("design", json!("   "))] {
            let node = fixture
                .snapshot
                .nodes
                .get_mut(id)
                .expect("the fixture holds it");
            node.title = title.as_str().map(str::to_owned);
        }
        fixture.project();

        for id in ["build", "design"] {
            let (front, _) = fixture.task_document(id);
            assert_eq!(
                front["title"],
                json!(id),
                "an untitled node reached the board with no label a destination would take"
            );
            assert_eq!(
                front["metadata"]["onepipeline.id"],
                json!(id),
                "the derived title moved which node this item is"
            );
        }
    }

    /// The rule's second, third and fourth consequences at once, as one
    /// complement: the projection is a total replacement of the destination
    /// item, so everything it does not own has to come back unchanged.
    ///
    /// Asserted as the whole complement rather than field by field, and held
    /// against a destination missing one of those fields: an assertion that
    /// could not fail on a deletion is the defect this replaces.
    #[test]
    fn a_projection_carries_through_everything_the_plan_does_not_declare() {
        let fixture = Fixture::new("preserved");
        fixture.project();

        let (front, body) = fixture.project_document();
        assert_eq!(
            preserved(&front, &body),
            preserved_of(&fixture.destination),
            "the projection changed something the plan does not declare"
        );
        assert_ne!(
            front["title"],
            json!(fixture.snapshot.project.native()),
            "the projection wrote the project's native identifier as its title"
        );
        assert_eq!(
            front["metadata"]["onetaskgraph.origin"],
            json!(fixture.snapshot.project.as_str()),
            "the projection lost the destination project it writes onto"
        );

        // The same assertion, against a destination one unowned field lighter.
        // It has to fail, or it was never asking whether anything left.
        let mut deleted = destination("A person's own board", &["planning", "q3"]);
        deleted.metadata.remove("authored.note");
        assert_ne!(
            preserved(&front, &body),
            preserved_of(&deleted),
            "the complement assertion cannot fail on a deleted field, so it proves nothing"
        );
        let mut unlabelled = destination("A person's own board", &["planning"]);
        unlabelled.metadata = fixture.destination.metadata.clone();
        assert_ne!(
            preserved(&front, &body),
            preserved_of(&unlabelled),
            "the complement assertion cannot fail on a dropped label, so it proves nothing"
        );
    }

    /// The same consequence for a task: labels are read off the destination task
    /// the node projects onto, and a node the destination has no task for gets
    /// none invented for it.
    #[test]
    fn a_projection_carries_a_destination_tasks_labels_through_and_invents_none() {
        let fixture = Fixture::new("task-labels");
        fixture.project();

        let (build, _) = fixture.task_document("build");
        assert_eq!(
            build["labels"],
            json!([{"id": "needs-review", "name": "needs-review", "color": "d73a4a"}]),
            "the projection dropped the destination task's labels"
        );
        assert_eq!(
            build["metadata"]["onetaskgraph.origin"], "plans:board/002-build",
            "the projection lost the destination task it writes onto"
        );

        let (design, _) = fixture.task_document("design");
        assert_eq!(
            design["labels"],
            json!([]),
            "the projection invented labels for a task the destination does not hold"
        );
        assert_eq!(
            design["metadata"].get("onetaskgraph.origin"),
            None,
            "the projection claimed a destination task for a node the destination has none for"
        );
    }

    /// A settled node's item says which settlement closed it, and what the
    /// change that closed it was.
    ///
    /// The two halves are one fact: a status alone cannot say whether the work
    /// reached anybody, and a reader closing work on the word `done` would close
    /// it on a change request nobody has merged.
    #[test]
    fn a_settled_node_carries_the_change_that_closed_it_and_a_node_without_one_carries_none() {
        let mut fixture = Fixture::new("landing");
        fixture
            .snapshot
            .landings
            .insert("build".to_owned(), Landing::Landed);
        fixture
            .snapshot
            .landing_commits
            .insert("build".to_owned(), "d3adb33f".to_owned());
        fixture.snapshot.change_urls.insert(
            "build".to_owned(),
            "https://example.invalid/pull/7".to_owned(),
        );
        fixture.project();

        let (build, _) = fixture.task_document("build");
        assert_eq!(build["status"], "done", "a node that landed is not closed");
        assert_eq!(build["metadata"]["onepipeline.landing"], "landed");
        assert_eq!(build["metadata"]["onepipeline.landing_commit"], "d3adb33f");
        assert_eq!(
            build["metadata"]["onepipeline.change_url"],
            "https://example.invalid/pull/7"
        );
        // A landing the run observed names no tier beside it: the key is for a
        // landing resting on something else, and its absence is the observation.
        assert_eq!(build["metadata"].get(LANDING_EVIDENCE_KEY), None);

        // A node with no change of its own claims none, rather than claiming one
        // with an empty value a reader would have to interpret.
        let (design, _) = fixture.task_document("design");
        for absent in [
            "onepipeline.landing",
            "onepipeline.landing_commit",
            "onepipeline.change_url",
            LANDING_EVIDENCE_KEY,
        ] {
            assert_eq!(
                design["metadata"].get(absent),
                None,
                "a node with no change of its own was recorded as having {absent}"
            );
        }
    }

    /// Each settlement reaches the destination document under its own word, and
    /// the settlement itself is beside it.
    #[test]
    fn each_settlement_reaches_the_document_under_its_own_word() {
        let mut seen: BTreeMap<String, &str> = BTreeMap::new();
        for (settlement, status, outcome) in [
            ("done", NodeStatus::Done, None),
            ("failed", NodeStatus::Failed, None),
            (
                "provider-failed",
                NodeStatus::Failed,
                Some(crate::engine::PROVIDER_FAILED),
            ),
            ("cancelled", NodeStatus::Cancelled, None),
            ("parked", NodeStatus::Parked, None),
        ] {
            let mut fixture = Fixture::new("settlement-words");
            fixture.snapshot.statuses.insert("build".to_owned(), status);
            if let Some(outcome) = outcome {
                fixture
                    .snapshot
                    .outcomes
                    .insert("build".to_owned(), outcome.to_owned());
            }
            fixture.snapshot.settlements.insert(
                "build".to_owned(),
                json!({"status": status.as_str(), "outcome": outcome}),
            );
            fixture.project();

            let (build, _) = fixture.task_document("build");
            let word = build["status"]
                .as_str()
                .expect("a projected status is a string")
                .to_owned();
            assert_eq!(
                build["metadata"][crate::taskgraph::SETTLEMENT_KEY]["status"],
                json!(status.as_str()),
                "the settlement did not reach the item beside its word"
            );
            if let Some(shared) = seen.insert(word.clone(), settlement) {
                panic!("'{settlement}' and '{shared}' both reached the board as `{word}`");
            }
        }
        assert_eq!(seen.len(), 5, "two settlements shared a word: {seen:?}");
    }

    /// A draft-complete node reaches the board as `in progress`, and why it is
    /// still open is beside the word.
    ///
    /// It is the one node projected under a word that is not its settlement's,
    /// because it has not settled: its work is finished and the change it
    /// published cannot land until the release it awaits happens, so the run is
    /// still on it. `done` would tell a reader the change landed and `cancelled`
    /// would tell them nobody is coming back to it, so both halves are held —
    /// the word it takes, and the two it must not share with the settlements
    /// that do mean those things.
    #[test]
    fn a_draft_complete_node_reaches_the_board_as_in_progress_and_says_why() {
        let mut fixture = Fixture::new("draft-complete");
        fixture
            .snapshot
            .statuses
            .insert("build".to_owned(), NodeStatus::CompleteDraft);
        fixture
            .snapshot
            .outcomes
            .insert("build".to_owned(), crate::vcs::DRAFTED.to_owned());
        fixture
            .snapshot
            .landings
            .insert("build".to_owned(), Landing::Unlanded);
        fixture.snapshot.change_urls.insert(
            "build".to_owned(),
            "https://example.invalid/pull/9".to_owned(),
        );
        fixture.snapshot.settlements.insert(
            "build".to_owned(),
            json!({
                "status": NodeStatus::CompleteDraft.as_str(),
                "outcome": crate::vcs::DRAFTED,
                "detail": "awaiting the crate release of github.com/owner/engine",
            }),
        );
        fixture.project();

        let (build, _) = fixture.task_document("build");
        assert_eq!(
            build["status"], "in progress",
            "a node whose change cannot land yet did not reach the board as work still in hand"
        );
        // The two words it must not take, read off the projection that writes
        // them rather than restated beside it: a draft is neither closed nor
        // abandoned.
        for settled in [NodeStatus::Done, NodeStatus::Cancelled] {
            assert_ne!(
                build["status"].as_str(),
                Some(word(projected(settled, None, Claim::Held)).as_str()),
                "a draft-complete node took the board word `{}` settles under",
                settled.as_str()
            );
        }

        // Why it is still open, beside the word that cannot say it. The
        // settlement's own reason is on the reserved key, and the draft a person
        // opens to read it is beside that.
        assert_eq!(
            build["metadata"][crate::taskgraph::SETTLEMENT_KEY]["detail"],
            json!("awaiting the crate release of github.com/owner/engine"),
            "the draft reached the board with nothing on it saying why it is still open"
        );
        assert_eq!(
            build["metadata"]["onepipeline.change_url"],
            "https://example.invalid/pull/9"
        );
        assert_eq!(build["metadata"]["onepipeline.landing"], "unlanded");
    }

    use super::{
        decide, member_id, Baseline, LandedBaseline, ProjectionActions, Rendering, Scope,
        WholeBecause,
    };

    /// A baseline saying every lineage of `snapshot` landed as it renders, each on an item of
    /// its own.
    fn landed_as(snapshot: &Snapshot) -> Baseline {
        let lineages = snapshot.lineages();
        let mut landed = LandedBaseline::empty(&snapshot.project);
        for root in lineages.roots() {
            let rendering = Rendering::of(snapshot, &lineages, root).expect("a lineage renders");
            let destination: GlobalId = format!("plans:board/{root}").parse().expect("an id");
            landed
                .items
                .insert(root.clone(), rendering.item.landed_at(&destination));
        }
        Baseline {
            path: snapshot.dir.join(crate::cli::WRITEBACK_LANDED_FILE),
            landed,
        }
    }

    /// What a driver carries of `now` against a baseline that says `last` landed, refusing a
    /// lineage it would read.
    fn carried(last: &Snapshot, now: &Snapshot) -> Vec<String> {
        let renderings = renderings(now);
        let decided = decide(
            now,
            &now.lineages(),
            &renderings,
            &landed_as(last),
            Scope::Driven,
        );
        assert!(decided.unread.is_empty(), "{:?}", decided.unread);
        decided.carried.into_iter().collect()
    }

    fn renderings(snapshot: &Snapshot) -> BTreeMap<String, Rendering> {
        let lineages = snapshot.lineages();
        lineages
            .roots()
            .map(|root| {
                (
                    root.clone(),
                    Rendering::of(snapshot, &lineages, root).expect("a lineage renders"),
                )
            })
            .collect()
    }

    /// A projection carries exactly the lineages whose projection differs from what the
    /// baseline says landed: a node whose word on the board moved, a node the baseline does not
    /// hold, and nothing for a change that renders no differently or that only the project item
    /// carries.
    #[test]
    fn a_projection_carries_exactly_the_lineages_that_differ_from_the_baseline() {
        let fixture = Fixture::new("carry");
        let last = fixture.snapshot.clone();

        assert_eq!(
            carried(&last, &last),
            Vec::<String>::new(),
            "a snapshot the baseline already holds carried a node"
        );

        let mut one = last.clone();
        one.statuses.insert("build".to_owned(), NodeStatus::Running);
        assert_eq!(carried(&last, &one), ["build"]);

        let mut both = one.clone();
        both.settlements
            .insert("design".to_owned(), json!({"status": "done"}));
        assert_eq!(carried(&last, &both), ["build", "design"]);

        // Pending and ready are both `queued` on the board, so moving between them is no change
        // to what a copy would write.
        let mut pending = last.clone();
        pending
            .statuses
            .insert("design".to_owned(), NodeStatus::Pending);
        let mut ready = last.clone();
        ready
            .statuses
            .insert("design".to_owned(), NodeStatus::Ready);
        assert_eq!(carried(&pending, &ready), Vec::<String>::new());

        let mut added = last.clone();
        let mut verify = added.nodes["design"].clone();
        verify.id = "verify".to_owned();
        verify.task_record = None;
        added.nodes.insert("verify".to_owned(), verify);
        assert_eq!(carried(&last, &added), ["verify"]);

        let mut project_level = last.clone();
        project_level
            .project_metadata
            .insert("onepipeline.goal".to_owned(), json!("a goal restated"));
        assert_eq!(
            carried(&last, &project_level),
            Vec::<String>::new(),
            "a change only the project item carries named a task"
        );

        // A repository is the one field a copy writes that the baseline does not keep, and the
        // edits that change one move the lineage's word too — a requeue of a parked node — so
        // the lineage is carried on a field the baseline keeps.
        let mut parked = last.clone();
        parked
            .statuses
            .insert("build".to_owned(), NodeStatus::Parked);
        let mut requeued = parked.clone();
        requeued
            .statuses
            .insert("build".to_owned(), NodeStatus::Ready);
        requeued.nodes.get_mut("build").expect("a node").repo =
            Some("github.com/owner/elsewhere".to_owned());
        assert_eq!(
            carried(&parked, &requeued),
            ["build"],
            "a requeue that moved the repository was not carried"
        );
    }

    /// No attempt is whole. A driver's first decision against the baseline the launch seeded
    /// carries the lineages whose projection differs from it — every one, for a fresh launch,
    /// since the seed records no word — and a lineage the baseline does not hold is read by the
    /// id the run knows for it rather than carried blind. A stop's release carries only the
    /// unstarted lineages the baseline says are `queued`, and reads rather than creates one it
    /// does not hold.
    #[test]
    fn no_attempt_is_whole_and_a_lineage_the_baseline_does_not_hold_is_read_by_its_id() {
        let fixture = Fixture::new("baseline-decide");
        let snapshot = fixture.snapshot.clone();
        let lineages = snapshot.lineages();
        let now = renderings(&snapshot);

        // Seeded: every item's word unknown, so the claim carries every lineage.
        let mut seeded = landed_as(&snapshot);
        for item in seeded.landed.items.values_mut() {
            item.status = None;
        }
        let decided = decide(&snapshot, &lineages, &now, &seeded, Scope::Driven);
        assert_eq!(
            decided.carried.iter().cloned().collect::<Vec<_>>(),
            lineages.roots().cloned().collect::<Vec<_>>()
        );

        // A baseline holding nothing reads each lineage the run knows an id for, by that id.
        let mut known = snapshot.clone();
        known.nodes.get_mut("build").expect("a node").task_record = Some(crate::plan::TaskRecord {
            id: "board/002-build".to_owned(),
            key: None,
            title: "Build".to_owned(),
        });
        let empty = Baseline {
            path: known.dir.join(crate::cli::WRITEBACK_LANDED_FILE),
            landed: LandedBaseline::empty(&known.project),
        };
        let decided = decide(
            &known,
            &known.lineages(),
            &renderings(&known),
            &empty,
            Scope::Driven,
        );
        assert_eq!(
            decided
                .unread
                .get("build")
                .map(ToString::to_string)
                .as_deref(),
            Some("plans:board/002-build")
        );
        assert!(!decided.carried.contains("build"));

        // The release: only a lineage whose word is `todo` now and `queued` in the baseline.
        let mut released = snapshot.clone();
        released.claim = Claim::Released;
        for status in released.statuses.values_mut() {
            *status = NodeStatus::Ready;
        }
        released
            .statuses
            .insert("build".to_owned(), NodeStatus::Running);
        let mut claimed = landed_as(&snapshot);
        for (root, item) in &mut claimed.landed.items {
            item.status = Some(if root == "design" {
                ProjectedStatus::Queued
            } else {
                ProjectedStatus::Todo
            });
        }
        let decided = decide(
            &released,
            &released.lineages(),
            &renderings(&released),
            &claimed,
            Scope::Release,
        );
        assert_eq!(
            decided.carried.into_iter().collect::<Vec<_>>(),
            ["design"],
            "the release carried other than the unstarted lineage the baseline says is queued"
        );
        assert!(decided.unread.is_empty());
    }

    /// Entry 93's example is the landed baseline's shape exactly: it deserializes through the
    /// type that writes the file, is a file this build reads for its project, and serializes
    /// back as itself — so the entry and the type cannot part. Every refusal the entry names is
    /// one the read makes, and the file's name and version are the constants the worker uses.
    #[test]
    fn entry_93s_example_is_the_landed_baseline_the_worker_writes_and_reads() {
        let block = divergence_block("93.");
        let landed = &block["landed"];
        assert_eq!(
            landed["file"].as_str(),
            Some(format!("<run dir>/{}", crate::cli::WRITEBACK_LANDED_FILE).as_str())
        );
        assert_eq!(
            landed["schema_version"].as_u64(),
            Some(u64::from(crate::cli::WRITEBACK_LANDED_SCHEMA_VERSION))
        );
        assert_eq!(landed["feeds_scheduling"], json!(false));
        let example = &landed["example"];
        let read: LandedBaseline = serde_json::from_value(example.clone())
            .unwrap_or_else(|error| panic!("entry 93's example is not a baseline: {error}"));
        assert_eq!(
            serde_json::to_value(&read).expect("a baseline serializes"),
            *example,
            "entry 93's example does not write back as itself"
        );
        let project: crate::taskgraph::QualifiedId = example["project"]
            .as_str()
            .expect("a project")
            .parse()
            .expect("a qualified project");
        read.checked(&project)
            .unwrap_or_else(|why| panic!("entry 93's example is refused: {why}"));
        assert!(
            read.items.values().any(|item| item.status.is_none())
                && read.items.values().any(|item| item.status.is_some()),
            "the example shows no seeded item beside a landed one"
        );

        let other: crate::taskgraph::QualifiedId = "plans:another".parse().expect("a project");
        assert!(read.checked(&other).is_err(), "another run's file was read");
        for (refused, patch) in [
            ("another version", json!({"schema_version": 2})),
            (
                "a foreign project key",
                json!({"project_metadata": {"team": "x"}}),
            ),
            ("a key the entry does not name", json!({"labels": []})),
        ] {
            let mut file = example.clone();
            for (key, value) in patch.as_object().expect("a patch") {
                file[key] = value.clone();
            }
            let accepted = serde_json::from_value::<LandedBaseline>(file.clone())
                .map_err(|error| error.to_string())
                .and_then(|landed| landed.checked(&project));
            assert!(accepted.is_err(), "{refused} was read: {file}");
        }
        for (refused, key, value) in [
            (
                "a foreign item key",
                "metadata",
                json!({"authored.note": "x"}),
            ),
            (
                "a destination that is not a qualified id",
                "destination",
                json!("bare"),
            ),
            (
                "a destination in another source",
                "destination",
                json!("elsewhere:writeback-quota-plan/002-build"),
            ),
            ("a digest that is not one", "content_sha256", json!("ABC")),
            ("a word the engine never writes", "status", json!("blocked")),
            (
                "a ticket that is not a qualified id",
                "delivers",
                json!(["bare"]),
            ),
            ("an edge naming no node", "depends_on", json!([""])),
            ("labels", "labels", json!([])),
        ] {
            let mut file = example.clone();
            file["items"]["build"][key] = value;
            let accepted = serde_json::from_value::<LandedBaseline>(file.clone())
                .map_err(|error| error.to_string())
                .and_then(|landed| landed.checked(&project));
            assert!(accepted.is_err(), "{refused} was read: {file}");
        }
    }

    /// A lineage the baseline does not hold on a board an older build wrote — one item per
    /// attempt — is read at the furthest-along attempt the previous driver's shadow documents
    /// recorded an item for, and at the root's own task only where they record none.
    #[test]
    fn a_lineage_the_baseline_does_not_hold_is_read_at_its_furthest_along_recorded_item() {
        let mut fixture = Fixture::new("baseline-known-id");
        retried(&mut fixture, true);
        let snapshot = fixture.snapshot.clone();
        let lineages = snapshot.lineages();
        let chain = lineages.chain("build").expect("the lineage").to_vec();
        assert!(chain.len() >= 3, "{chain:?}");
        snapshot_task_record(&mut fixture.snapshot, "build", "board/000-root");
        let snapshot = fixture.snapshot.clone();
        let folder = snapshot
            .dir
            .join("tasks")
            .join(super::project_file(&snapshot.project));
        std::fs::create_dir_all(&folder).expect("the shadow folder");
        assert_eq!(
            super::known_id(&snapshot, &lineages, "build").map(|id| id.to_string()),
            Some("plans:board/000-root".to_owned()),
            "with no shadow document the root's own task is not the one read"
        );
        let write = |node: &str, origin: &str| {
            super::document(
                &folder.join(format!("{}.md", super::task_file(node))),
                &json!({"title": node, "metadata": {"onetaskgraph.origin": origin}}),
                "",
            )
            .expect("a shadow document");
        };
        write(&chain[0], "plans:board/000-root");
        write(&chain[1], "plans:board/older-attempt");
        // A document naming an item in another source names nothing on this destination.
        write(&chain[2], "elsewhere:board/stray");
        assert_eq!(
            super::known_id(&snapshot, &lineages, "build").map(|id| id.to_string()),
            Some("plans:board/older-attempt".to_owned())
        );
    }

    fn snapshot_task_record(snapshot: &mut Snapshot, node: &str, id: &str) {
        snapshot.nodes.get_mut(node).expect("a node").task_record = Some(crate::plan::TaskRecord {
            id: id.to_owned(),
            key: None,
            title: node.to_owned(),
        });
    }

    /// What a copy report counts against what the attempt read before it, as the worker
    /// counts it.
    trait ActionsAgainst {
        fn actions_against(
            &self,
            snapshot: &Snapshot,
            before: &BTreeMap<String, Origin>,
        ) -> ProjectionActions;
    }

    impl ActionsAgainst for onetaskgraph_core::CopyReport {
        fn actions_against(
            &self,
            snapshot: &Snapshot,
            before: &BTreeMap<String, Origin>,
        ) -> ProjectionActions {
            super::actions(self, snapshot, before)
        }
    }

    /// The reason an engine that drove the store's binary gave for a store older than the
    /// member copy is one this build reads and never gives, and entry 73 says every reason is
    /// read-only now.
    #[test]
    fn every_reason_a_projection_was_whole_is_read_and_never_written() {
        let block = divergence_block("73.");
        let read_only = block["projection"]["whole_because_read_only"]
            .as_object()
            .expect("entry 73 names the reasons it reads and never writes");
        let every: std::collections::BTreeSet<String> = [
            WholeBecause::First,
            WholeBecause::AfterFailure,
            WholeBecause::StoreLacksMembers,
        ]
        .into_iter()
        .map(|reason| {
            serde_json::to_value(reason)
                .expect("serializes")
                .as_str()
                .expect("a word")
                .to_owned()
        })
        .collect();
        assert_eq!(
            read_only
                .keys()
                .cloned()
                .collect::<std::collections::BTreeSet<_>>(),
            every
        );
        assert_eq!(
            block["projection"]["fields"]["whole_because"]["written"],
            json!([]),
            "entry 73 says a reason to be whole is still written"
        );
    }

    /// A copy report is counted item by item, the project item included, and teaches the run
    /// where a node the copy created landed — without which the next member copy naming it would
    /// create it again. An item that is no shadow task of this snapshot teaches nothing.
    #[test]
    fn a_copy_report_is_counted_and_teaches_the_run_where_a_created_node_landed() {
        let fixture = Fixture::new("report");
        let snapshot = &fixture.snapshot;
        let report: onetaskgraph_core::CopyReport = serde_json::from_value(json!({
            "items": [
                {"source": format!("onepipeline-writeback:{}", super::project_file(&snapshot.project)),
                 "action": "unchanged", "destination": "plans:board"},
                {"source": member_id(snapshot, "build"), "action": "updated",
                 "destination": "plans:board/002-build"},
                {"source": member_id(snapshot, "design"), "action": "created",
                 "destination": "plans:board/003-design"},
                {"source": "elsewhere:board/gone", "action": "orphaned",
                 "destination": "plans:board/009-gone"},
            ],
            "spent": {"requests": 3, "budgets": []},
        }))
        .expect("the store's own report reads");
        assert_eq!(
            super::actions(&report, snapshot, &fixture.origins),
            ProjectionActions {
                created: 1,
                updated: 1,
                unchanged: 1,
                orphaned: 1,
                // `build` read `done` before the copy and is projected `done`: rewritten,
                // not reopened.
                reopened: 0,
            }
        );
        assert_eq!(
            report.spent.as_ref().map(super::verbatim),
            Some(
                json!({"requests": 3, "budgets": []})
                    .as_object()
                    .cloned()
                    .expect("an object")
            ),
            "the record's `spent` is not the store's own, verbatim"
        );

        let mut origins = fixture.origins.clone();
        super::learn(&report, &mut origins, snapshot);
        assert_eq!(
            origins.len(),
            2,
            "an item that is no node's shadow task taught a node"
        );
        assert_eq!(origins["design"].id.to_string(), "plans:board/003-design");
        assert!(origins["design"].labels.is_empty());
        assert_eq!(origins["build"].id.to_string(), "plans:board/002-build");
        assert_eq!(
            origins["build"].labels.len(),
            1,
            "an updated node the run already knew lost the labels it was read with"
        );
    }

    /// A destination the store reports both `updated` and `orphaned` in one copy — an item an
    /// older build wrote, reused for a lineage while it still carried that build's origin — is
    /// counted under `updated` alone. An orphan at a destination the copy did not rewrite still
    /// counts. (The store's own `CopyAction` gives every orphan a destination, so an orphan
    /// naming none is no longer a report this build can be handed.)
    #[test]
    fn a_destination_the_copy_updated_is_not_also_counted_orphaned() {
        let fixture = Fixture::new("reused");
        let snapshot = &fixture.snapshot;
        let report: onetaskgraph_core::CopyReport = serde_json::from_value(json!({
            "items": [
                {"source": member_id(snapshot, "build"), "action": "updated",
                 "destination": "plans:board/002-build"},
                {"source": "onepipeline-writeback:older/build-2", "action": "orphaned",
                 "destination": "plans:board/002-build"},
                {"source": "elsewhere:board/gone", "action": "orphaned",
                 "destination": "plans:board/009-gone"},
            ],
        }))
        .expect("the store's own report reads");
        assert_eq!(
            super::actions(&report, snapshot, &fixture.origins),
            ProjectionActions {
                created: 0,
                updated: 1,
                unchanged: 0,
                orphaned: 1,
                reopened: 0,
            }
        );
    }

    /// The fixture with `build` retried under `build-2`, and `build-2` under `build-3` where
    /// `twice` — a lineage of two or three, read off `superseded` alone. The head states its own
    /// title, body, persona and `delivers`, keeps `design` as its dependency, and is ready; every
    /// superseded attempt is recorded cancelled. `ship` depends on the head, so an edge onto a
    /// retried node has somewhere to be rewritten to.
    fn retried(fixture: &mut Fixture, twice: bool) -> Vec<String> {
        let mut chain = vec!["build".to_owned(), "build-2".to_owned()];
        if twice {
            chain.push("build-3".to_owned());
        }
        let head = chain.last().expect("a head").clone();
        for (superseded, replacement) in chain.iter().zip(chain.iter().skip(1)) {
            fixture
                .snapshot
                .superseded
                .insert(superseded.clone(), replacement.clone());
            fixture
                .snapshot
                .statuses
                .insert(superseded.clone(), NodeStatus::Cancelled);
            fixture.snapshot.settlements.insert(
                superseded.clone(),
                json!({"status": "cancelled", "outcome": "superseded"}),
            );
            let mut node: Node = serde_json::from_value(json!({
                "id": replacement,
                "title": format!("feat: {replacement} it again"),
                "task": format!("## What\n{replacement} it, again."),
                "persona": "reviewer",
                "deps": ["design"],
                "delivers": ["tickets:board/build"],
            }))
            .expect("a replacement node");
            node.branch = Some(format!("topic/{replacement}"));
            fixture.snapshot.nodes.insert(replacement.clone(), node);
            fixture
                .snapshot
                .statuses
                .insert(replacement.clone(), NodeStatus::Ready);
        }
        let ship: Node = serde_json::from_value(json!({
            "id": "ship", "title": "feat: ship it", "task": "## What\nShip it.",
            "persona": "engineer", "deps": [head],
        }))
        .expect("a dependent node");
        fixture.snapshot.nodes.insert("ship".to_owned(), ship);
        fixture
            .snapshot
            .statuses
            .insert("ship".to_owned(), NodeStatus::Pending);
        chain
    }

    /// One shadow task per lineage, under the **root's** file, saying what the **head** says:
    /// its title, body, word, fields, `delivers` and edges, with `onepipeline.id` the root,
    /// `onepipeline.node` the head and `onepipeline.supersedes` every id between, root first.
    /// The superseded attempts have no shadow task of their own — a stale one left in the
    /// shadow store is taken away — an edge onto the head names the root's file, the root's
    /// destination item is the one written onto, and a head with no title is written under the
    /// root's id. The settlement keys are the head's own, so a superseded attempt's cancellation
    /// does not reach the item.
    #[test]
    fn one_shadow_task_per_lineage_is_keyed_by_the_root_and_says_what_the_head_says() {
        let mut fixture = Fixture::new("lineage");
        let chain = retried(&mut fixture, true);
        let stale = fixture
            .dir
            .join("tasks")
            .join(super::project_file(&fixture.snapshot.project))
            .join(format!("{}.md", super::task_file("build-2")));
        std::fs::create_dir_all(stale.parent().expect("a directory")).expect("the tasks dir");
        std::fs::write(&stale, "---\ntitle: left by an earlier build\n---\n")
            .expect("a stale task");
        fixture.project();

        let (build, body) = fixture.task_document("build");
        assert_eq!(build["title"], "feat: build-3 it again");
        assert_eq!(body, "## What\nbuild-3 it, again.");
        assert_eq!(
            build["status"], "queued",
            "the lineage carries a superseded attempt's word"
        );
        assert_eq!(build["metadata"][ID_KEY], "build");
        assert_eq!(build["metadata"][NODE_KEY], "build-3");
        assert_eq!(
            build["metadata"][SUPERSEDES_KEY],
            json!(["build", "build-2"])
        );
        assert_eq!(build["metadata"]["onepipeline.persona"], "reviewer");
        assert_eq!(build["metadata"]["onepipeline.branch"], "topic/build-3");
        assert_eq!(build["delivers"], json!(["tickets:board/build"]));
        assert_eq!(
            build["metadata"].get(crate::taskgraph::SETTLEMENT_KEY),
            None,
            "a superseded attempt's settlement reached the lineage's item"
        );
        assert_eq!(
            build["metadata"]["onetaskgraph.origin"], "plans:board/002-build",
            "the lineage was not written onto the item its root already holds"
        );
        assert_eq!(
            build["labels"],
            json!([{"id": "needs-review", "name": "needs-review", "color": "d73a4a"}]),
        );
        assert_eq!(
            build["depends_on"],
            json!([format!(
                "{}/{}",
                super::project_file(&fixture.snapshot.project),
                super::task_file("design")
            )])
        );
        for attempt in &chain[1..] {
            let own = fixture
                .dir
                .join("tasks")
                .join(super::project_file(&fixture.snapshot.project))
                .join(format!("{}.md", super::task_file(attempt)));
            assert!(
                !own.exists(),
                "a retry attempt was projected as a shadow task of its own: {attempt}"
            );
        }
        assert!(
            !stale.exists(),
            "a shadow task this snapshot did not write was left behind"
        );

        // An edge onto the head names the root's shadow task.
        let (ship, _) = fixture.task_document("ship");
        assert_eq!(
            ship["depends_on"],
            json!([format!(
                "{}/{}",
                super::project_file(&fixture.snapshot.project),
                super::task_file("build")
            )]),
            "an edge onto a retried node names something other than its lineage's root"
        );
        assert_eq!(ship["metadata"][NODE_KEY], "ship");
        assert_eq!(
            ship["metadata"].get(SUPERSEDES_KEY),
            None,
            "a node nothing superseded lists supersessions"
        );

        // A head with no title takes the root's id, not its own.
        fixture
            .snapshot
            .nodes
            .get_mut("build-3")
            .expect("the head")
            .title = None;
        fixture.project();
        let (untitled, _) = fixture.task_document("build");
        assert_eq!(untitled["title"], "build");

        // One lineage per root, in order: what a copy names and the record carries.
        assert_eq!(
            fixture
                .snapshot
                .lineages()
                .roots()
                .cloned()
                .collect::<Vec<_>>(),
            ["build", "design", "ship"]
        );
    }

    /// A retry changes the **root's** member rather than adding one: the member copy after it
    /// names the root, never the replacement, and a second retry of the same lineage names the
    /// same root again.
    #[test]
    fn a_retry_is_carried_as_a_change_to_the_root_member() {
        let mut fixture = Fixture::new("lineage-carry");
        let last = fixture.snapshot.clone();
        retried(&mut fixture, false);
        let once = fixture.snapshot.clone();
        assert_eq!(
            carried(&last, &once),
            ["build", "ship"],
            "a retry named the replacement as a member of its own, or missed the root"
        );

        let mut fixture = Fixture::new("lineage-carry-twice");
        retried(&mut fixture, true);
        let twice = fixture.snapshot.clone();
        assert_eq!(
            carried(&once, &twice),
            ["build"],
            "a second retry of one lineage named something other than its root"
        );
        assert_eq!(carried(&twice, &twice), Vec::<String>::new());
    }

    /// Entry 80's block names exactly the keys a lineage's shadow task is written under, the
    /// reads its category comes off, and the record key the reopen count lands on — held here
    /// because the keys are private to this module and the block is their one source.
    #[test]
    fn entry_80_names_the_lineage_keys_and_the_reopen_rule_the_projection_writes() {
        let block = divergence_block("80.");
        let keys: Vec<String> = block["lineage"]["keys"]
            .as_object()
            .expect("entry 80 names the lineage keys")
            .keys()
            .cloned()
            .collect();
        assert_eq!(keys, [ID_KEY, NODE_KEY, SUPERSEDES_KEY]);
        assert_eq!(block["lineage"]["keys"][ID_KEY], "root");
        assert_eq!(block["lineage"]["keys"][NODE_KEY], "head");
        assert_eq!(
            block["lineage"]["keys"][SUPERSEDES_KEY]["written_when"],
            "the head is not the root"
        );
        assert_eq!(block["lineage"]["shadow_tasks_per_lineage"], 1);
        assert_eq!(
            block["destination"]["category_read_by"],
            json!({"members": super::TASK_SHOW})
        );
        assert_eq!(block["reopened"]["store_vocabulary_extended"], false);
        assert_eq!(
            block["reopened"]["record"]["from_schema_version"].as_u64(),
            Some(u64::from(super::PROJECTION_LINE_WITH_REOPENED))
        );
        assert_eq!(block["reopened"]["record"]["key"], "actions.reopened");
        assert!(
            serde_json::to_value(ProjectionActions::default())
                .expect("counts serialize")
                .get("reopened")
                .is_some(),
            "the record has no `reopened` for entry 80 to name"
        );
    }

    /// `reopened` is derived from the pre-copy category and the projected word, item by item:
    /// an item the store `updated` that read `cancelled` or `done` before the copy and is
    /// projected under a word that is neither is one reopen. An item rewritten from `done` to
    /// `done`, one created, one whose category the read did not answer, and one that is no
    /// shadow task of this snapshot count nothing. A report naming a reopen an older build never
    /// counted still reads.
    #[test]
    fn reopened_is_counted_off_the_pre_copy_category_and_the_projected_word() {
        let mut fixture = Fixture::new("reopened");
        retried(&mut fixture, false);
        // `design` is done on both sides; `build`'s lineage is projected `queued`; `ship` has
        // no item yet and is created.
        let snapshot = &fixture.snapshot;
        let before = BTreeMap::from([
            (
                "build".to_owned(),
                Origin {
                    id: "plans:board/002-build".parse().expect("a qualified task"),
                    labels: Vec::new(),
                    category: Some(StatusCategory::Cancelled),
                },
            ),
            (
                "design".to_owned(),
                Origin {
                    id: "plans:board/003-design".parse().expect("a qualified task"),
                    labels: Vec::new(),
                    category: Some(StatusCategory::Done),
                },
            ),
        ]);
        let item = |root: &str, action: &str| {
            json!({"source": member_id(snapshot, root), "action": action,
                   "destination": format!("plans:board/00x-{root}")})
        };
        let report = |items: Vec<Value>| -> onetaskgraph_core::CopyReport {
            serde_json::from_value(json!({"items": items})).expect("the store's report reads")
        };
        let counted = report(vec![
            item("build", "updated"),
            item("design", "updated"),
            item("ship", "created"),
            json!({"source": "elsewhere:board/gone", "action": "updated",
                   "destination": "plans:board/009-gone"}),
        ])
        .actions_against(snapshot, &before);
        assert_eq!(
            counted,
            ProjectionActions {
                created: 1,
                updated: 3,
                unchanged: 0,
                orphaned: 0,
                reopened: 1,
            }
        );

        // The same copy, where the pre-copy read answered no category for the reopened item.
        let mut unknown = before.clone();
        unknown
            .get_mut("build")
            .expect("the lineage's origin")
            .category = None;
        assert_eq!(
            report(vec![item("build", "updated")])
                .actions_against(snapshot, &unknown)
                .reopened,
            0,
            "an item whose category nobody read was counted reopened"
        );
        // And where the lineage is projected under a closed word: a drop after the retry.
        let mut dropped = fixture.snapshot.clone();
        dropped
            .statuses
            .insert("build-2".to_owned(), NodeStatus::Cancelled);
        assert_eq!(
            report(vec![item("build", "updated")])
                .actions_against(&dropped, &before)
                .reopened,
            0,
            "a cancelled item rewritten cancelled was counted reopened"
        );
        // A closeout's `todo`, the word a released claim is written under, is open too.
        let mut released = fixture.snapshot.clone();
        released.claim = Claim::Released;
        assert_eq!(
            report(vec![item("build", "updated")])
                .actions_against(&released, &before)
                .reopened,
            1
        );
    }

    /// A shadow store whose path is not UTF-8 is refused by name before the store is opened,
    /// rather than named to the store lossily — which would point it at a folder nobody wrote.
    #[cfg(unix)]
    #[test]
    fn a_shadow_store_path_that_is_not_utf8_is_refused_rather_than_named_lossily() {
        use std::os::unix::ffi::OsStrExt;
        let fixture = Fixture::new("not-utf8");
        let mut snapshot = fixture.snapshot.clone();
        snapshot.dir = fixture
            .dir
            .join(std::ffi::OsStr::from_bytes(b"shadow-\xff"));
        let store = crate::taskgraph::Store::at(fixture.dir.to_path_buf());
        let mut baseline = super::Baseline::load(&fixture.dir, &snapshot.project);
        let attempted = super::project(
            &store,
            DEFAULT_WRITEBACK_ITEM_BUDGET_SECONDS,
            &snapshot,
            &mut baseline,
            super::Scope::Driven,
        );
        assert!(attempted.calls.is_empty(), "{:?}", attempted.calls);
        let failed = match attempted.result {
            Ok(_) => panic!("a shadow store no setting can name was projected through"),
            Err(failed) => failed,
        };
        assert!(
            failed.reason.contains("is not a UTF-8 path"),
            "{}",
            failed.reason
        );
        assert!(
            !snapshot.dir.exists(),
            "the store was opened over a path it was never handed"
        );
    }
}
