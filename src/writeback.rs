//! Best-effort projection of the journal-owned graph into its onetaskgraph project.
//!
//! The reconcile loop remains the only author of graph state: it hands immutable folded
//! snapshots to this worker, and the worker only projects them. Store reads never feed back
//! into scheduling, and a failed or slow write is reported and retried off the engine thread
//! — unless the store *refused* it, which no retry changes: that is reported once and
//! attempted again only when the run's graph does. See [`FailureClass`].
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
//! * **A task's identity is its lineage root's, not the node's.** A `retry` supersedes a
//!   node with a replacement, and the replacement is projected onto the item the superseded
//!   node already holds: the shadow task's file, its `--member` id and its `onepipeline.id`
//!   are the **root's** — the id the plan authored or an `add` stated — and everything it
//!   says is the **head's**, the one node in the lineage nothing superseded, with
//!   `onepipeline.node` naming which attempt that is and `onepipeline.supersedes` the ones
//!   it replaced. A superseded node has no shadow task of its own, so a retry rewrites one
//!   item rather than closing one and minting another, and a retry or requeue of a
//!   cancelled node writes an open word onto its item, which the store reopens. Entry 80 of
//!   `docs/contract-divergences.md` states the rule; [`Lineages`] computes it.
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
//! # A projection carries what changed
//!
//! Against a hosted destination every item a copy reads or writes spends the same allowance
//! every other reader of the token needs, so an attempt carries only the nodes whose shadow
//! task changed since the last attempt that landed, named to the store's `--member`. It
//! carries the whole project where it has to: when nothing has landed in this worker yet,
//! after an attempt that failed, and against a store that offers no member copy — decided
//! once per run, before its first projection, off the version the launch check read. What a
//! member copy does not name it neither reads nor rewrites, so the ownership rule above still
//! decides every field of every item a projection writes, and a person's edit on an item the
//! copy did not name stands until that node next changes. Every attempt is appended to the
//! run's projection record; see [`ProjectionRecord`].

use std::collections::{BTreeMap, BTreeSet};
use std::num::{NonZeroU32, NonZeroU64};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

use crate::cli::{
    DEFAULT_WRITEBACK_ITEM_BUDGET_SECONDS, WRITEBACK_CLASSIFIED_COMMANDS,
    WRITEBACK_COMMAND_FLOOR_SECONDS, WRITEBACK_DELIVERS_FROM, WRITEBACK_FAILURE_EXIT,
    WRITEBACK_MEMBERS_FROM, WRITEBACK_MEMBER_READ, WRITEBACK_PARTIAL_EXIT,
    WRITEBACK_PROJECTIONS_FILE, WRITEBACK_PROJECTIONS_SCHEMA_VERSION, WRITEBACK_REFUSED_CLASS,
    WRITEBACK_STORE_FILE,
};
use crate::edits::Operation;
use crate::event::Source;
use crate::graph::{Landing, NodeStatus};
use crate::ledger::{LaunchRecord, RunPaths};
use crate::plan::Node;
use crate::projection::RunState;
use crate::taskgraph::{QualifiedId, BINARY_ENV};

const SHADOW_SOURCE: &str = "onepipeline-writeback";
/// The reserved key naming the plan node a destination item is the shadow of: the
/// **lineage root's** id, which is what the store's `task list` is read back by.
const ID_KEY: &str = "onepipeline.id";
/// The reserved key naming the lineage's head — the attempt whose fields the item carries —
/// and, with [`SUPERSEDES_KEY`], one of the two lineage keys entry 80 of
/// `docs/contract-divergences.md` names and
/// [`tests::every_word_and_key_this_projection_writes_is_named_by_the_divergence`] holds it to.
const NODE_KEY: &str = "onepipeline.node";
/// The reserved key listing the ids a lineage's head superseded, root first, written only
/// where the head is not the root.
const SUPERSEDES_KEY: &str = "onepipeline.supersedes";
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
// Cross-platform runners have measured real sibling commands taking longer than ten seconds
// under suite-wide contention. This remains a backstop for an unreachable store, not a
// latency target: projection stays off the reconcile loop while the child runs. It is the
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
// Closeout never inherits the duration of a store command. A slow store may keep working in
// the worker, but it still cannot turn a completed graph into run settlement.
const CLOSEOUT_WAIT: Duration = Duration::from_millis(2_250);
// The three store commands one attempt runs, by the name each one's capture files and
// refusals carry. The store's own class is read off a failure of any of them.
const PROJECT_SHOW: &str = WRITEBACK_CLASSIFIED_COMMANDS[0];
const TASK_LIST: &str = WRITEBACK_CLASSIFIED_COMMANDS[1];
const PROJECT_COPY: &str = WRITEBACK_CLASSIFIED_COMMANDS[2];
// What a member projection reads each named member with, in place of the page of tasks.
const TASK_SHOW: &str = WRITEBACK_MEMBER_READ;

/// How long one store command may run, and the account a refusal gives of the figure.
///
/// The reads are the same size whatever the plan, so [`COMMAND_FLOOR`] alone bounds them.
/// The copy writes one item per node it carries, so its deadline is the launch's per-item
/// budget multiplied by those nodes — every node for a whole copy, the named ones for a
/// member copy, and the list an [`Unprojected`] surface names either way — with the floor
/// governing until a copy is large enough to lift it. The account is derived from the
/// figure rather than stored beside it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Deadline {
    /// The fixed floor, which is the whole deadline for a read.
    Floor,
    /// The copy's: `max(floor, per_item × items)`, with the budget in seconds.
    Copy { per_item: NonZeroU64, items: usize },
}

impl Deadline {
    /// The budget multiplied through, in seconds, before the floor is applied.
    ///
    /// Exact for every product that fits in a `u64` of seconds, and `u64::MAX` seconds for
    /// one that does not — saturating rather than wrapping, because a product that wrapped
    /// to nothing would leave the floor governing exactly the plan the budget exists to
    /// accommodate.
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
            Self::Copy { per_item, items } => Self::product(per_item, items).max(COMMAND_FLOOR),
        }
    }

    /// The one line a command that outlasted this is refused with.
    fn refusal(self, name: &str) -> String {
        let seconds = self.within().as_secs();
        match self {
            Self::Floor => format!("{name} exceeded {seconds} seconds"),
            Self::Copy { per_item, items } => {
                let arithmetic = format!(
                    "{items} {} × {} {} per item",
                    if items == 1 { "item" } else { "items" },
                    per_item,
                    if per_item.get() == 1 {
                        "second"
                    } else {
                        "seconds"
                    }
                );
                if Self::product(per_item, items) < COMMAND_FLOOR {
                    format!(
                        "{name} exceeded {seconds} seconds (the {} second floor; {arithmetic} \
                         is less)",
                        COMMAND_FLOOR.as_secs()
                    )
                } else {
                    format!("{name} exceeded {seconds} seconds ({arithmetic})")
                }
            }
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
    /// What the store's vocabulary offers, which decides whether the shadow task carries
    /// `delivers`.
    vocabulary: Vocabulary,
}

/// What a node the run has not started says about the work it names.
///
/// While a driver drives the run, the run has claimed its whole plan, and an unstarted node
/// is written `queued`, which the store counts as a claim on every ticket it delivers. At
/// closeout — settled or stopped — a node that never started is written `todo`, which the
/// store counts as a release. A store older than [`WRITEBACK_DELIVERS_FROM`] has no `queued`,
/// so every snapshot projected into one releases.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Claim {
    Held,
    Released,
}

/// Whether the store a run projects into offers `queued` and `delivers`, decided off the version
/// its `--version` reported against [`WRITEBACK_DELIVERS_FROM`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Vocabulary {
    /// `queued` is a status, a task carries `delivers`, and the store moves what it delivers.
    WithDelivers,
    /// A release before either: an unstarted node is written `todo` and no task carries
    /// `delivers`.
    BeforeDelivers,
}

impl Vocabulary {
    /// The vocabulary of a store reporting `version`. A version that does not read is taken to
    /// offer only what every release does.
    fn of(version: &str) -> Self {
        if crate::taskgraph::at_least(version, WRITEBACK_DELIVERS_FROM) {
            Self::WithDelivers
        } else {
            Self::BeforeDelivers
        }
    }

    /// What an unstarted node is written under while a driver drives the run.
    fn driving_claim(self) -> Claim {
        match self {
            Self::WithDelivers => Claim::Held,
            Self::BeforeDelivers => Claim::Released,
        }
    }
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
/// its allowance for nothing. `transient` is everything a wait can change. The mapping from a
/// failure to its class is the store's and is never restated here, and the message beside it
/// is never read to decide. A class this build has never heard of does not parse, which
/// leaves the failure unclassified and so on the retry schedule rather than off it.
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

/// What the store said about one failed attempt: its class, and the kind of failure it was.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Classified {
    pub class: FailureClass,
    // llmlint: ignore[invalid_states_unrepresentable] the store's `kind` is open by its own
    // contract — a source error's kind is copied through verbatim, and a plugin a release
    // newer than this build speaks kinds this one has never seen — and it is only ever
    // named to a reader. `class` is the closed half this worker acts on.
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
}

struct Failed {
    reason: String,
    classified: Option<Classified>,
    /// The copy report's `delivered` entries, where a copy that exited unsuccessfully wrote one.
    delivered: Vec<Map<String, Value>>,
}

impl Failed {
    /// A store command that exited unsuccessfully, classed by what it wrote on stdout.
    fn answered(output: &Output, reason: String) -> Self {
        Self {
            reason,
            classified: classified(output.status.code(), &output.stdout),
            delivered: Vec::new(),
        }
    }

    fn refused(&self) -> bool {
        self.classified.as_ref().is_some_and(Classified::refused)
    }

    /// The reason, with the store's class and kind beside it where it gave them.
    ///
    /// A classified reason is the store's own stderr, which is several lines ending in its
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

/// A failure the store wrote nothing about: this worker's own, or a command that never
/// answered.
impl From<String> for Failed {
    fn from(reason: String) -> Self {
        Self {
            reason,
            classified: None,
            delivered: Vec::new(),
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
    /// What the store offers, from the version the launch check read.
    vocabulary: Vocabulary,
    /// The copy's per-item budget, which bounds the launch wait as it bounds the copy.
    per_item: NonZeroU64,
    /// Whether this driver has said why a plan's tickets are not moved by an older store.
    told_why_not_delivered: std::sync::atomic::AtomicBool,
}

impl Writeback {
    /// Start the worker for one driver of a run.
    ///
    /// `version` is the token the store's `--version` printed for the launch check, which is
    /// what the worker decides a member copy is offered from where no earlier driver of the
    /// run has decided it already.
    pub fn start(
        binary: PathBuf,
        version: &str,
        paths: &RunPaths,
        launch: &LaunchRecord,
    ) -> Option<Self> {
        let pending = Arc::new((Mutex::new(Pending::default()), Condvar::new()));
        let worker_pending = Arc::clone(&pending);
        let run_dir = paths.dir.clone();
        let vocabulary = Vocabulary::of(version);
        let version = version.to_owned();
        let launch_dir = if launch.dir.as_os_str().is_empty() {
            PathBuf::from(".")
        } else {
            launch.dir.clone()
        };
        let per_item = per_item_budget(launch);
        // llmlint: ignore-block[changed_behavior_has_e2e] A host refusing one thread while
        // continuing to run this process is resource exhaustion no real CLI journey can
        // arrange at this boundary. Every reachable worker failure is covered against the
        // real sibling and real store; this compatibility edge deliberately disables only
        // the projection and leaves the run unchanged.
        std::thread::Builder::new()
            .name(format!("writeback-{}", paths.run))
            .spawn(move || {
                worker(
                    binary,
                    &version,
                    launch_dir,
                    run_dir,
                    per_item,
                    worker_pending,
                )
            })
            .ok()?;
        // llmlint: ignore-end[changed_behavior_has_e2e]
        let writer = Self {
            pending,
            vocabulary,
            per_item,
            told_why_not_delivered: std::sync::atomic::AtomicBool::new(false),
        };
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
        let Some(snapshot) = snapshot_of(
            paths,
            launch,
            state,
            statuses,
            self.vocabulary.driving_claim(),
            self.vocabulary,
        ) else {
            return;
        };
        self.say_once_why_tickets_are_not_moved(&snapshot);
        self.queue(snapshot);
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
        if let Some(snapshot) = snapshot_of(
            paths,
            launch,
            state,
            statuses,
            Claim::Released,
            self.vocabulary,
        ) {
            self.queue(snapshot);
        }
    }

    /// Wait for this worker to end one attempt, bounded by the store command deadline for a
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

    /// Against a store that has no `delivers`, a plan whose nodes deliver tickets is told once,
    /// on the driver's standard error, that nothing will move them.
    fn say_once_why_tickets_are_not_moved(&self, snapshot: &Snapshot) {
        if self.vocabulary == Vocabulary::WithDelivers
            || snapshot.nodes.values().all(|node| node.delivers.is_empty())
        {
            return;
        }
        if self
            .told_why_not_delivered
            .swap(true, std::sync::atomic::Ordering::Relaxed)
        {
            return;
        }
        eprintln!(
            "onetaskgraph write-back will not move the tickets this plan's tasks deliver for \
             '{}': the store reports a version older than {WRITEBACK_DELIVERS_FROM}, the first \
             release carrying `queued` and `delivers`, so unstarted nodes are written `todo` and \
             no task carries `delivers` — install onetaskgraph {WRITEBACK_DELIVERS_FROM} or newer",
            snapshot.project
        );
    }
}

/// The snapshot one publish hands the worker, or `None` for a launch naming no project.
fn snapshot_of(
    paths: &RunPaths,
    launch: &LaunchRecord,
    state: &RunState,
    statuses: &BTreeMap<String, NodeStatus>,
    claim: Claim,
    vocabulary: Vocabulary,
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
        vocabulary,
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
        // exits. This keeps a completed run from racing a person's next store command,
        // while the hard command limit preserves write-back's latency boundary.
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
        // Deliberately no join: a store process is outside the run's failure and latency
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

/// Release what a run a `stop` ended had claimed and never started: one whole projection, in
/// the stopping process, writing every node the run had not started `todo`.
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
    let store = match crate::taskgraph::Store::resolve() {
        Ok(store) => store,
        Err(error) => {
            // Said rather than swallowed: every task the run claimed and never started stays
            // claimed on the board until something writes it again, and this line is the only
            // place anybody hears that.
            eprintln!(
                "onetaskgraph write-back could not release the nodes this stopped run never \
                 started: {error}"
            );
            return;
        }
    };
    let state = crate::checkpoint::Projected::open(paths);
    let statuses = state.statuses();
    let vocabulary = Vocabulary::of(store.reported_version());
    let Some(snapshot) = snapshot_of(
        paths,
        launch,
        &state,
        &statuses,
        Claim::Released,
        vocabulary,
    ) else {
        return;
    };
    let launch_dir = if launch.dir.as_os_str().is_empty() {
        PathBuf::from(".")
    } else {
        launch.dir.clone()
    };
    let carry = Carry::Whole(WholeBecause::First);
    let items = carry.items(&snapshot);
    let at = crate::sys::now_rfc3339();
    let started = Instant::now();
    // llmlint: ignore-block[changed_behavior_has_e2e] a stop whose release outlasts its deadline
    // takes exactly the lines below that a refused release takes: `bounded_output` kills the copy
    // and answers `Err`, and the attempt is recorded and said on stderr as any failure is. Those
    // lines are driven end to end by
    // `delivers::a_stop_whose_release_the_store_refuses_still_stops_and_says_so`, which asserts
    // the stop's answer, its stderr and the failed record line. The one timeout-specific branch
    // is `bounded_output`'s kill, driven by
    // `writeback_budget::a_copy_held_past_a_tiny_budget_is_killed_and_the_refusal_names_the_arithmetic`
    // and `delivers::a_first_projection_held_past_its_deadline_does_not_hold_back_the_first_dispatch`.
    // A journey holding a `stop` past the sixty-second floor would spend that minute on no line
    // those three do not already reach.
    let attempt = project(
        &store.binary(),
        &launch_dir,
        &paths.dir,
        per_item_budget(launch),
        &snapshot,
        &carry,
        &BTreeMap::new(),
    );
    append_record(
        &paths.dir,
        &ProjectionRecord::of(
            at,
            &snapshot.project,
            &carry,
            items,
            started.elapsed(),
            &attempt,
        ),
    );
    if let Err(failed) = attempt {
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
    binary: PathBuf,
    version: &str,
    launch_dir: PathBuf,
    run_dir: PathBuf,
    per_item: NonZeroU64,
    pending: Arc<(Mutex<Pending>, Condvar)>,
) {
    // Decided here, on the worker's own thread and before the first snapshot is taken, so
    // the reconcile loop neither waits on it nor reads it.
    let members = decide_member_copy_once(&run_dir, version);
    let mut standing = Standing::Landing;
    let mut carried = Carried::default();
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
        let carry = Carry::decide(
            members,
            standing != Standing::Landing,
            carried.last.as_ref(),
            &snapshot,
        );
        let items = carry.items(&snapshot);
        let at = crate::sys::now_rfc3339();
        let started = Instant::now();
        let attempt = project(
            &binary,
            &launch_dir,
            &run_dir,
            per_item,
            &snapshot,
            &carry,
            &carried.origins,
        );
        append_record(
            &run_dir,
            &ProjectionRecord::of(
                at,
                &snapshot.project,
                &carry,
                items.clone(),
                started.elapsed(),
                &attempt,
            ),
        );
        {
            let (lock, ready) = &*pending;
            if let Ok(mut state) = lock.lock() {
                state.attempts = state.attempts.saturating_add(1);
                ready.notify_all();
            }
        }
        match attempt {
            Ok(landed) => {
                if standing != Standing::Landing {
                    eprintln!(
                        "onetaskgraph write-back recovered for '{}'",
                        snapshot.project
                    );
                }
                standing = Standing::Landing;
                carried = Carried {
                    last: Some(snapshot.clone()),
                    origins: landed.origins,
                };
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
                if first {
                    // The one line on the driver's stderr that ever says a projection is in
                    // trouble, so it says what an operator's next question is: whether to
                    // expect another attempt in a moment or in a minute.
                    eprintln!(
                        "onetaskgraph write-back failed for '{}': {}; retrying, spacing \
                         further attempts out to {} seconds apart while it keeps failing",
                        snapshot.project,
                        failed.said(),
                        RETRY_CEILING.as_secs()
                    );
                }
                // How soon the planner hears of it, which is this sum and no deadline: the
                // surface is recorded one [`FIRST_RETRY_AFTER`] after the command that refused,
                // and the reconcile loop asks for it every `engine::CHANNEL_POLL`. A store that
                // is not there refuses the first command outright — `onetaskgraph` cannot
                // resolve a root that is gone, and answers so in milliseconds — so
                // [`COMMAND_FLOOR`] is no part of it: that is spent only by a command that has
                // not exited, which is a store answering slowly.
                if !should_retry_after(&pending, retry_after(failures.get())) {
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

/// Wait out at most one retry interval, answering whether to attempt again at all.
///
/// `false` is [`RunPhase::Stopping`] and the caller returns on it; `true` is the wait
/// having been served, or [`RunPhase::ClosingOut`]. Both phases are read on entry as well
/// as on every wake, so one the run reached while this worker was projecting is honoured
/// rather than missed. A snapshot published meanwhile deliberately does *not* shorten the
/// wait: what the interval spaces is the destination's refusal, and a run that keeps
/// folding new graph state would otherwise retry as fast as it publishes.
fn should_retry_after(pending: &(Mutex<Pending>, Condvar), interval: Duration) -> bool {
    let (lock, ready) = pending;
    let Ok(mut state) = lock.lock() else {
        return false;
    };
    let due = Instant::now() + interval;
    loop {
        match state.phase {
            RunPhase::Stopping => return false,
            RunPhase::ClosingOut => return true,
            RunPhase::Running => {}
        }
        let left = due.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return true;
        }
        let Ok((next, _)) = ready.wait_timeout(state, left) else {
            return false;
        };
        state = next;
    }
}

/// One attempt that landed: where each node's destination item now is, and what the copy
/// said it did and spent.
struct Landed {
    origins: BTreeMap<String, Origin>,
    actions: Option<ProjectionActions>,
    spent: Option<Map<String, Value>>,
    delivered: Vec<Map<String, Value>>,
}

fn project(
    binary: &Path,
    launch_dir: &Path,
    run_dir: &Path,
    per_item: NonZeroU64,
    snapshot: &Snapshot,
    carry: &Carry,
    known: &BTreeMap<String, Origin>,
) -> Result<Landed, Failed> {
    let destination_project = destination_project(binary, launch_dir, run_dir, snapshot)?;
    let mut origins = match carry {
        Carry::Whole(_) => destination_origins(binary, launch_dir, run_dir, snapshot)?,
        Carry::Members(named) => member_origins(binary, launch_dir, run_dir, named, known)?,
    };
    // llmlint: ignore-block[changed_behavior_has_e2e] The real outage journey drives
    // destination write failure through onetaskgraph. Making this private, run-owned
    // shadow directory unwritable would instead require sabotaging the host filesystem,
    // outside the public run interface and unrelated to store availability.
    write_shadow(snapshot, &origins, &destination_project)?;
    // llmlint: ignore-end[changed_behavior_has_e2e]
    let root = snapshot.dir.to_string_lossy().into_owned();
    let mut args = vec![
        "project".to_owned(),
        "copy".to_owned(),
        format!("{SHADOW_SOURCE}:{}", project_file(&snapshot.project)),
        "--to".to_owned(),
        snapshot.project.source().to_owned(),
        "--json".to_owned(),
        "--set".to_owned(),
        format!("sources.{SHADOW_SOURCE}.plugin=local-md"),
        "--set".to_owned(),
        format!("sources.{SHADOW_SOURCE}.config.root={root}"),
    ];
    // A member copy names exactly the nodes that changed. Naming none is not a copy of
    // everything: the project item alone carries what changed at the project's level.
    if let Carry::Members(named) = carry {
        // llmlint: ignore-block[changed_behavior_has_e2e] a member projection naming no node is
        // reached only by a snapshot that moves no node's shadow task — a status moving inside
        // one board word, pending to ready — because no edit a CLI accepts changes only the
        // project's metadata, and the run passes through such a move inside a pass no input
        // holds open. `writeback::tests` holds the decision that names none; `--no-tasks` is the
        // store's own documented flag for copying the project item alone.
        if named.is_empty() {
            args.push("--no-tasks".to_owned());
        }
        // llmlint: ignore-end[changed_behavior_has_e2e]
        for node in named {
            args.extend(["--member".to_owned(), member_id(snapshot, node)]);
        }
    }
    // The one command that is linear in what it carries, so the one whose deadline is.
    let deadline = Deadline::Copy {
        per_item,
        items: carry.items(snapshot).len(),
    };
    let output = bounded_output(binary, launch_dir, run_dir, PROJECT_COPY, &args, deadline)?;
    // Read for what the copy says it did. A report this build cannot read leaves that unsaid
    // on the record rather than failing a copy the store says landed.
    let report: Option<CopyReport> = serde_json::from_slice(&output.stdout).ok();
    if output.status.success() {
        // Counted off the origins as the pre-copy read left them, before the report teaches
        // the run where anything moved: a reopen is decided by what the item read as before
        // the copy and what the copy wrote onto it.
        let actions = report
            .as_ref()
            .map(|report| report.actions(snapshot, &origins));
        if let Some(report) = &report {
            report.learn(&mut origins, snapshot);
        }
        let (spent, delivered) = report
            .map(|report| (report.spent, report.delivered))
            .unwrap_or_default();
        Ok(Landed {
            origins,
            actions,
            spent,
            delivered,
        })
    } else {
        let delivered = report.map(|report| report.delivered).unwrap_or_default();
        // A copy whose own writes landed and whose delivered tickets the store could not keep
        // in step exits as a partial answer, and is a projection that did not fully land: the
        // tickets are behind the run. So it is failed, surfaced and retried whole exactly as
        // any partial projection is, with the tickets and what the store said of each as the
        // reason.
        let reason = match failed_deliveries(&output.stdout) {
            Some(tickets) if output.status.code() == Some(WRITEBACK_PARTIAL_EXIT) => format!(
                "the copy landed, but the store could not keep every delivered ticket in step: \
                 {tickets}"
            ),
            _ => format!(
                "copy exited {}: {}",
                exit(&output.status),
                String::from_utf8_lossy(&output.stderr).trim()
            ),
        };
        let mut failed = Failed::answered(&output, reason);
        failed.delivered = delivered;
        Err(failed)
    }
}

/// Each ticket a copy report says the store failed to keep in step, named with its deliverer and
/// the store's own words about it, read through [`DeliveredAnswer`].
///
/// `None` where the report failed no ticket, and equally where any failed entry does not read —
/// an id that is not qualified, an outcome this build has never heard of, a failure with no
/// class or message. An answer that does not validate is reported by the copy's own exit and
/// stderr, never by a sentence assembled out of fields that did not.
fn failed_deliveries(stdout: &[u8]) -> Option<String> {
    let answer: DeliveredAnswer = serde_json::from_slice(stdout).ok()?;
    let failed: Vec<String> = answer
        .delivered
        .iter()
        .filter_map(|entry| match &entry.outcome {
            DeliveredOutcome::Failed { failure } => Some(format!(
                "ticket {} (delivered by {}): {}",
                entry.ticket,
                entry.deliverer,
                failure
                    .message
                    .split_whitespace()
                    .collect::<Vec<_>>()
                    .join(" ")
            )),
            DeliveredOutcome::Written | DeliveredOutcome::Unchanged | DeliveredOutcome::Left => {
                None
            }
        })
        .collect();
    (!failed.is_empty()).then(|| failed.join("; "))
}

fn destination_project(
    binary: &Path,
    launch_dir: &Path,
    run_dir: &Path,
    snapshot: &Snapshot,
) -> Result<DestinationProjectItem, Failed> {
    let args = ["project", "show", snapshot.project.as_str(), "--json"];
    let output = bounded_output(
        binary,
        launch_dir,
        run_dir,
        PROJECT_SHOW,
        &args,
        Deadline::Floor,
    )?;
    if !output.status.success() {
        let reason = format!(
            "project show exited {}: {}",
            exit(&output.status),
            String::from_utf8_lossy(&output.stderr).trim()
        );
        return Err(Failed::answered(&output, reason));
    }
    // llmlint: ignore-block[changed_behavior_has_e2e] These refusals defend the compiled
    // sibling's machine contract. Producing malformed JSON, partial results, no project,
    // or a different/duplicate project here requires replacing the real onetaskgraph
    // executable with a scripted mock; the real-store journey drives the successful read,
    // total-replacement copy, and preservation of present and absent content end to end.
    let response: ProjectPage = answered(&output.stdout)?;
    if !response.errors.is_empty() {
        return Err("project show returned partial results".to_owned().into());
    }
    let mut items = response.items.into_iter();
    let project = items
        .next()
        .ok_or_else(|| format!("project '{}' was not found", snapshot.project))?;
    if items.next().is_some() || project.id != snapshot.project {
        return Err(format!(
            "project show returned the wrong project for '{}'",
            snapshot.project
        )
        .into());
    }
    // llmlint: ignore-end[changed_behavior_has_e2e]
    Ok(project.item)
}

/// What the destination already holds for one lineage, keyed by the lineage's root.
///
/// The id is what a projection writes back onto; the labels are what it carries forward
/// unchanged, because no plan models them; the category is what the item read as *before*
/// the copy, which is the half of a reopen the copy report cannot say. The id keeps the type
/// it was read through: every id the store answers with crossed [`QualifiedId`]'s boundary,
/// and narrowing it to a `String` here would let an unqualified one be written back.
#[derive(Clone)]
struct Origin {
    id: QualifiedId,
    labels: Vec<DestinationLabel>,
    /// The item's normalised status category as the attempt's own pre-copy read reported
    /// it, or `None` where that read answered without one.
    category: Option<DestinationCategory>,
}

impl Origin {
    /// Whether the item read as closed — `done` or `cancelled` — before the copy, so a copy
    /// that rewrote it onto an open word reopened it.
    fn closed(&self) -> bool {
        self.category.is_some_and(|category| {
            matches!(
                category,
                DestinationCategory::Done | DestinationCategory::Cancelled
            )
        })
    }
}

fn destination_origins(
    binary: &Path,
    launch_dir: &Path,
    run_dir: &Path,
    snapshot: &Snapshot,
) -> Result<BTreeMap<String, Origin>, Failed> {
    // Each item placed in its lineage as the page is read, and folded onto lineage roots
    // once the whole page has been.
    let lineages = snapshot.lineages();
    let mut placed: Vec<Placed> = Vec::new();
    let mut page: Option<String> = None;
    let mut cursors = BTreeSet::new();
    loop {
        let mut args = vec![
            "task".to_owned(),
            "list".to_owned(),
            "--project".to_owned(),
            snapshot.project.as_str().to_owned(),
            "--limit".to_owned(),
            "10000".to_owned(),
            "--json".to_owned(),
        ];
        if let Some(token) = &page {
            args.extend(["--page".to_owned(), token.clone()]);
        }
        // llmlint: ignore-block[changed_behavior_has_e2e] The real unavailable-store
        // journey proves this asynchronous read cannot affect or delay reconciliation.
        // Making the real sibling hang requires host-level process suspension, not an
        // input exposed by either CLI, and substituting a hanging script would mock the
        // exact executable boundary the journey is required to drive.
        let output = bounded_output(
            binary,
            launch_dir,
            run_dir,
            TASK_LIST,
            &args,
            Deadline::Floor,
        )?;
        // llmlint: ignore-end[changed_behavior_has_e2e]
        if !output.status.success() {
            let reason = format!(
                "task list exited {}: {}",
                output.status,
                String::from_utf8_lossy(&output.stderr).trim()
            );
            return Err(Failed::answered(&output, reason));
        }
        // llmlint: ignore-block[changed_behavior_has_e2e] These refusals defend the
        // compiled sibling's machine contract. Producing malformed JSON, partial errors,
        // invalid qualified ids, missing node ids, or duplicate node ids here requires
        // replacing the real onetaskgraph executable with a scripted mock; real-store
        // success and outage/recovery are driven end to end instead.
        let response: TaskPage = answered(&output.stdout)?;
        if !response.errors.is_empty() {
            return Err("task list returned partial results".to_owned().into());
        }
        for task in response.items {
            placed.push(Placed::of(&lineages, task)?);
        }
        let Some(next) = response.next else { break };
        if next.is_empty() {
            return Err("task list returned an empty next-page cursor"
                .to_owned()
                .into());
        }
        if !cursors.insert(next.clone()) {
            return Err("task list repeated a next-page cursor".to_owned().into());
        }
        page = Some(next);
    }
    // llmlint: ignore-end[changed_behavior_has_e2e]
    Ok(furthest_along(placed)?)
}

/// One destination item, placed in its lineage by the attempt it names.
///
/// The lineage is the one its `onepipeline.id` belongs to. Its position in that lineage is
/// its `onepipeline.node` where the item names one — the head this build wrote onto it — and
/// its `onepipeline.id` otherwise, which is every item an older build wrote and every item
/// nothing retried; an attempt the lineage does not hold places at the root.
struct Placed {
    // llmlint: ignore-block[invalid_states_unrepresentable] node ids off the item's own
    // reserved keys, the plain `String` the snapshot keys by; the position is an index into
    // the lineage's chain, and the origin is the typed half.
    root: String,
    position: usize,
    attempt: String,
    // llmlint: ignore-end[invalid_states_unrepresentable]
    origin: Origin,
}

impl Placed {
    fn of(lineages: &Lineages, task: DestinationTask) -> Result<Self, String> {
        let named = |key: &str| {
            task.item
                .metadata
                .get(key)
                .and_then(Value::as_str)
                .filter(|id| !id.is_empty())
                .map(str::to_owned)
        };
        let node =
            named(ID_KEY).ok_or_else(|| format!("task '{}' has no {ID_KEY}", task.id.as_str()))?;
        let attempt = named(NODE_KEY).unwrap_or_else(|| node.clone());
        let root = lineages.root_of(&node).to_owned();
        let position = lineages
            .chain(&root)
            .and_then(|chain| chain.iter().position(|id| *id == attempt))
            .unwrap_or(0);
        Ok(Self {
            root,
            position,
            attempt,
            origin: Origin {
                id: task.id,
                labels: task.item.labels,
                category: task.item.status.map(|status| status.category),
            },
        })
    }
}

/// Fold what the destination holds onto lineage roots.
///
/// Several items under one root are what a board an older build wrote holds after an
/// `--adopt` — one item per attempt — and, after this build's first projection over it, that
/// board holding the older item at the root beside the rewritten head. The lineage is
/// projected onto the item at the furthest-along position, and the rest are left exactly as
/// they are: a copy never deletes, and nothing here cleans up. Two items at one position are
/// two items for one attempt, and stay the refusal they always were. An item whose node no
/// lineage holds is its own root, as it was.
fn furthest_along(placed: Vec<Placed>) -> Result<BTreeMap<String, Origin>, String> {
    let mut by_position: BTreeMap<(String, usize), Placed> = BTreeMap::new();
    for item in placed {
        let key = (item.root.clone(), item.position);
        if by_position.contains_key(&key) {
            return Err(format!(
                "project has more than one task for node '{}'",
                item.attempt
            ));
        }
        by_position.insert(key, item);
    }
    // Ascending by root and then by position, so the last item under each root is its
    // furthest along.
    let mut by_root: BTreeMap<String, Origin> = BTreeMap::new();
    for ((root, _), item) in by_position {
        by_root.insert(root, item.origin);
    }
    Ok(by_root)
}

struct Output {
    status: ExitStatus,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}

/// Run one sibling command without letting either its duration or output pipes hold the worker.
fn bounded_output<S: AsRef<std::ffi::OsStr>>(
    binary: &Path,
    launch_dir: &Path,
    run_dir: &Path,
    name: &str,
    args: &[S],
    deadline: Deadline,
) -> Result<Output, String> {
    let stdout = run_dir.join(format!("writeback-{name}.stdout"));
    let stderr = run_dir.join(format!("writeback-{name}.stderr"));
    let stdout_file = std::fs::File::create(&stdout).map_err(|error| error.to_string())?;
    let stderr_file = std::fs::File::create(&stderr).map_err(|error| error.to_string())?;
    // llmlint: ignore-block[changed_behavior_has_e2e] Resolution and version checking
    // already exercise the real executable. Inducing spawn or wait syscall failure requires
    // replacing the executable or sabotaging the host; the real-store journey covers the
    // actionable command refusal, retry, and recovery behavior.
    let mut child = Command::new(binary)
        .current_dir(launch_dir)
        .args(args)
        .env_remove(BINARY_ENV)
        .stdout(Stdio::from(stdout_file))
        .stderr(Stdio::from(stderr_file))
        .spawn()
        .map_err(|error| format!("cannot run {}: {error}", binary.display()))?;
    let started = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if started.elapsed() < deadline.within() => {
                std::thread::sleep(Duration::from_millis(25));
            }
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(deadline.refusal(name));
            }
            Err(error) => return Err(format!("cannot wait for {name}: {error}")),
        }
    };
    // llmlint: ignore-end[changed_behavior_has_e2e]
    Ok(Output {
        status,
        stdout: std::fs::read(stdout).map_err(|error| error.to_string())?,
        stderr: std::fs::read(stderr).map_err(|error| error.to_string())?,
    })
}

/// Read one of the store's answers, naming the field that refused it.
///
/// `serde_json`'s own error says the type it wanted and not where it wanted it, and this
/// projection reads several fields of several shapes out of one paged response — so a
/// `labels` answered as a string and a `metadata` answered as one are otherwise the same
/// sentence. The path is the whole of what makes the refusal actionable, because of where
/// it lands: write-back is best-effort, so this one line on the driver's own standard
/// error and the planner surface built from it are all anybody gets. Read the same way
/// `taskgraph::read` reads a project, for the same reason.
fn answered<T: serde::de::DeserializeOwned>(stdout: &[u8]) -> Result<T, String> {
    let mut reading = serde_json::Deserializer::from_slice(stdout);
    let response: T =
        serde_path_to_error::deserialize(&mut reading).map_err(|error| {
            match error.path().to_string() {
                path if path == "." => error.into_inner().to_string(),
                path => format!("{path}: {}", error.into_inner()),
            }
        })?;
    // Trailing bytes are still a refusal, exactly as `serde_json::from_slice` made them:
    // a second document after the answer is not an answer this build can act on.
    reading.end().map_err(|error| error.to_string())?;
    Ok(response)
}

/// What the store classed one failed command as, read off its own answer and nothing else.
///
/// Exit [`WRITEBACK_FAILURE_EXIT`] carries one failure document. Exit
/// [`WRITEBACK_PARTIAL_EXIT`] carries a partial answer each of whose `errors` names a class,
/// and that answer is refused only where **every** entry is: a source that could not be reached
/// beside one that refused could still answer next time. Anything else is `None`, and so is an
/// answer that does not parse — a store release that predates the document, a command killed
/// at its deadline, and a class this build has never heard of all keep the retry schedule
/// rather than stopping it.
fn classified(code: Option<i32>, stdout: &[u8]) -> Option<Classified> {
    match code? {
        WRITEBACK_FAILURE_EXIT => {
            let document: FailureDocument = serde_json::from_slice(stdout).ok()?;
            Some(Classified {
                class: document.failure.class,
                kind: document.failure.kind,
            })
        }
        WRITEBACK_PARTIAL_EXIT => {
            // A partial read names its failures under `errors`; a copy whose delivered tickets
            // the store could not keep in step names them under `delivered`, each with the
            // store's own failure. Either is refused only where every failure it names is.
            let failures: Vec<(FailureClass, String)> =
                match serde_json::from_slice::<PartialAnswer>(stdout) {
                    Ok(answer) if !answer.errors.is_empty() => answer
                        .errors
                        .into_iter()
                        .map(|entry| (entry.class, entry.error.kind))
                        .collect(),
                    _ => serde_json::from_slice::<DeliveredAnswer>(stdout)
                        .ok()?
                        .delivered
                        .into_iter()
                        .filter_map(|entry| match entry.outcome {
                            DeliveredOutcome::Failed { failure } => {
                                Some((failure.class, failure.kind))
                            }
                            DeliveredOutcome::Written
                            | DeliveredOutcome::Unchanged
                            | DeliveredOutcome::Left => None,
                        })
                        .collect(),
                };
            if failures.is_empty() {
                return None;
            }
            let class = if failures
                .iter()
                .all(|(class, _)| *class == FailureClass::Refused)
            {
                FailureClass::Refused
            } else {
                FailureClass::Transient
            };
            let mut kinds: Vec<String> = Vec::new();
            for (_, kind) in failures {
                if !kinds.contains(&kind) {
                    kinds.push(kind);
                }
            }
            Some(Classified {
                class,
                kind: kinds.join(", "),
            })
        }
        _ => None,
    }
}

fn exit(status: &ExitStatus) -> String {
    status
        .code()
        .map_or_else(|| "on a signal".into(), |code| code.to_string())
}

// Every type below describes a response `onetaskgraph` composes, so none of them denies
// unknown fields: that program adds one to its own answer in a patch release — `location`
// on a project item, at 0.2.14 — and a consumer mirroring a producer's shape under
// `deny_unknown_fields` makes each of those a hard read failure. A document this crate
// authors and reads back is the opposite case and keeps the deny; this module authors only
// the shadow project, which nothing reads back through a type. What the projection
// consumes stays required and typed, and what it never read is no longer enumerated.

/// What a store command writes on stdout when it exits [`WRITEBACK_FAILURE_EXIT`].
#[derive(Deserialize)]
struct FailureDocument {
    failure: StoreFailure,
}

/// The two members of a store's failure this worker reads. Its `message` is the stderr line
/// already carried as the reason, and is never read to decide anything.
#[derive(Deserialize)]
struct StoreFailure {
    class: FailureClass,
    // llmlint: ignore[invalid_states_unrepresentable] open by the store's own contract, for
    // the reason `Classified::kind` records; only ever named to a reader.
    kind: String,
}

/// The half of a partial answer — exit [`WRITEBACK_PARTIAL_EXIT`] — this worker reads.
#[derive(Deserialize)]
struct PartialAnswer {
    errors: Vec<PartialError>,
}

/// One source's failure in a partial answer, carrying the store's class for it.
#[derive(Deserialize)]
struct PartialError {
    class: FailureClass,
    error: PartialCause,
}

/// The half of a copy report a partial copy is classified by: each delivered ticket's outcome,
/// and the store's failure for one it could not keep in step.
#[derive(Deserialize)]
struct DeliveredAnswer {
    delivered: Vec<DeliveredEntry>,
}

/// One entry of a copy report's `delivered`, validated at the boundary before anything is said
/// about it: both ids are qualified, and the outcome is one this build knows, carrying the
/// store's failure exactly when it is `failed`. The record keeps the store's entry verbatim; this
/// is what the worker interprets.
#[derive(Deserialize)]
struct DeliveredEntry {
    ticket: QualifiedId,
    deliverer: QualifiedId,
    #[serde(flatten)]
    outcome: DeliveredOutcome,
}

/// The store's failure for one delivered ticket it could not keep in step.
#[derive(Deserialize)]
struct DeliveredFailure {
    class: FailureClass,
    // llmlint: ignore-block[invalid_states_unrepresentable] the store's `kind` is open by its
    // own contract, for the reason `Classified::kind` records, and its `message` is its own
    // words; both are only ever named to a reader, and `class` is the closed half acted on.
    kind: String,
    message: String,
    // llmlint: ignore-end[invalid_states_unrepresentable]
}

/// What the store did to one delivered ticket, tagged by its `outcome`. Only a ticket the store
/// failed to keep in step carries a failure, and it has to: a `failed` entry without one does not
/// read, and neither does an outcome this build has never heard of — either leaves the answer
/// unclassified, and so on the retry schedule.
#[derive(Deserialize)]
#[serde(tag = "outcome", rename_all = "kebab-case")]
enum DeliveredOutcome {
    Written,
    Unchanged,
    Left,
    Failed { failure: DeliveredFailure },
}

#[derive(Deserialize)]
struct PartialCause {
    // llmlint: ignore[invalid_states_unrepresentable] a source error's kind, copied through
    // verbatim by the store and open for the reason `Classified::kind` records.
    kind: String,
}

/// One page of the store's answer to `task list`.
#[derive(Deserialize)]
struct TaskPage {
    items: Vec<DestinationTask>,
    next: Option<String>,
    errors: Vec<Value>,
}

/// The store's answer to `project show`.
#[derive(Deserialize)]
struct ProjectPage {
    items: Vec<DestinationProject>,
    errors: Vec<Value>,
}

#[derive(Deserialize)]
struct DestinationProject {
    id: QualifiedId,
    item: DestinationProjectItem,
}

#[derive(Deserialize)]
struct DestinationProjectItem {
    /// Not declared by a plan, so it is preserved rather than replaced.
    title: String,
    /// Not declared by a plan, so it is preserved rather than replaced.
    content: Option<String>,
    /// Not modelled by a plan at all, so they are preserved rather than dropped.
    labels: Vec<DestinationLabel>,
    /// Only the reserved keys this worker owns are rewritten; the rest are preserved.
    metadata: BTreeMap<String, Value>,
    // llmlint: ignore-block[invalid_states_unrepresentable] `location` is onetaskgraph's
    // own answer about where it keeps an item, and this projection neither mints one nor
    // reads one. It is named anyway so this boundary *states* the newest field the store
    // reports rather than leaving it to be inferred from an absence, and it is typed as
    // the raw document precisely so that it cannot narrow: the shapes are the producer's
    // to change, and it has already shipped `{"path": …}`, `{"url": …}` and
    // `{"kind": …, "url": …}` across patch releases. Giving it a type of its own is what
    // would make a fourth shape a projection failure, which is the defect this field was
    // added under rather than a stricter form of the fix.
    // llmlint: ignore-block[changed_behavior_has_e2e] naming it changes no behaviour that
    // could be driven on its own: the types here no longer deny unknown fields, so a page
    // carrying `location` was already accepted and naming it accepts exactly the same
    // pages. What there is to drive is that acceptance, and
    // `store::a_settlement_reaches_a_store_whose_answer_grew_a_field_this_build_does_not_know`
    // drives it against the real binary at a release this build was not written against —
    // on the response, on each item, on that item's body and on every label it holds.
    #[serde(rename = "location", default)]
    _location: Option<Value>,
    // llmlint: ignore-end[changed_behavior_has_e2e]
    // llmlint: ignore-end[invalid_states_unrepresentable]
}

#[derive(Deserialize)]
struct DestinationTask {
    id: QualifiedId,
    item: DestinationTaskItem,
}

#[derive(Deserialize)]
struct DestinationTaskItem {
    /// Not modelled by a plan at all, so they are preserved rather than dropped.
    labels: Vec<DestinationLabel>,
    /// Read for `onepipeline.id`, which is how a destination task names its plan node.
    metadata: BTreeMap<String, Value>,
    /// Read for its category, which is what says whether a copy reopened the item. Lenient:
    /// an answer without one — `store::…_a_field_it_never_read` drops it — reads as an item
    /// whose category nobody knows, which no copy is counted as reopening.
    #[serde(default)]
    status: Option<DestinationStatus>,
    // llmlint: ignore-block[invalid_states_unrepresentable] `location` is onetaskgraph's
    // own answer about where it keeps an item, and this projection neither mints one nor
    // reads one. It is named anyway so this boundary *states* the newest field the store
    // reports rather than leaving it to be inferred from an absence, and it is typed as
    // the raw document precisely so that it cannot narrow: the shapes are the producer's
    // to change, and it has already shipped `{"path": …}`, `{"url": …}` and
    // `{"kind": …, "url": …}` across patch releases. Giving it a type of its own is what
    // would make a fourth shape a projection failure, which is the defect this field was
    // added under rather than a stricter form of the fix.
    // llmlint: ignore-block[changed_behavior_has_e2e] naming it changes no behaviour that
    // could be driven on its own: the types here no longer deny unknown fields, so a page
    // carrying `location` was already accepted and naming it accepts exactly the same
    // pages. What there is to drive is that acceptance, and
    // `store::a_settlement_reaches_a_store_whose_answer_grew_a_field_this_build_does_not_know`
    // drives it against the real binary at a release this build was not written against —
    // on the response, on each item, on that item's body and on every label it holds.
    #[serde(rename = "location", default)]
    _location: Option<Value>,
    // llmlint: ignore-end[changed_behavior_has_e2e]
    // llmlint: ignore-end[invalid_states_unrepresentable]
}

/// The half of a destination item's status this worker reads: its normalised category.
#[derive(Deserialize)]
struct DestinationStatus {
    category: DestinationCategory,
}

/// onetaskgraph's normalised status categories, as its `--json` answers name them.
///
/// Closed under `Unknown`, the store's own word for a status its vocabulary cannot place,
/// so a category a later release adds reads as that rather than refusing the page — and is
/// neither `done` nor `cancelled`, so nothing is counted reopened off it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
enum DestinationCategory {
    Draft,
    Backlog,
    Todo,
    Queued,
    InProgress,
    Done,
    Cancelled,
    #[serde(other)]
    Unknown,
}

/// One label a destination item carries, read back and written unchanged.
///
/// Serialized as well as deserialized: a preserved label is written into the shadow
/// document whole, so the destination's own label id and colour survive the round trip
/// rather than being reduced to a name the store would have to re-resolve.
// llmlint: ignore-block[invalid_states_unrepresentable] These three are onetaskgraph's own
// strings, and this projection neither mints nor interprets one — it reads a label off the
// destination and writes the same label back. Narrowing them here would turn a label the
// store legitimately holds into a projection failure, which is the defect this type exists
// to fix rather than a stricter form of the fix.
#[derive(Clone, Deserialize, Serialize)]
struct DestinationLabel {
    id: String,
    name: String,
    color: Option<String>,
}
// llmlint: ignore-end[invalid_states_unrepresentable]

/// Build the shadow project a `project copy` then projects onto the destination.
///
/// Every field written here is decided by the module's ownership rule: a plan-declared
/// field is restated from the snapshot, and a field no plan models is carried over from
/// the destination item this was read against.
fn write_shadow(
    snapshot: &Snapshot,
    origins: &BTreeMap<String, Origin>,
    destination_project: &DestinationProjectItem,
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
    // build left here: a whole copy carries every task the shadow store holds, so a document
    // this snapshot did not write would reach the board as an item of its own.
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
/// lineage whose projection changed from one whose did not: see [`Carry::decide`].
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
    // The store's own field, never a reserved key: taken out of the wire whether or not the
    // store offers it, so an older store is handed no `onepipeline.delivers` either.
    let delivers = wire
        .remove(DELIVERS_FIELD)
        .and_then(|v| v.as_array().cloned())
        .unwrap_or_default();
    wire.remove("id");
    let mut metadata = Map::new();
    metadata.insert(ID_KEY.into(), json!(root));
    metadata.insert(NODE_KEY.into(), json!(head));
    if !superseded.is_empty() {
        metadata.insert(SUPERSEDES_KEY.into(), json!(superseded));
    }
    if let Some(origin) = origin {
        metadata.insert("onetaskgraph.origin".into(), json!(origin.id.as_str()));
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
    if snapshot.vocabulary == Vocabulary::WithDelivers && !delivers.is_empty() {
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

fn document(path: &Path, front: &Value, body: &str) -> Result<(), String> {
    let yaml = serde_norway::to_string(front).map_err(|e| e.to_string())?;
    std::fs::write(path, format!("---\n{yaml}---\n{body}")).map_err(|e| e.to_string())
}

/// The word one node's state is written onto its destination item's status.
///
/// A onetaskgraph status is a **name** and a normalised **category**, and this is
/// the name — four of these are a category's own word and the rest are words that
/// vocabulary has none of. `docs/contract-divergences.md` records what that costs
/// and why it is the cheaper of the two.
#[derive(Clone, Copy, PartialEq, Eq, Serialize)]
enum ProjectedStatus {
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

/// The shadow store's own qualified id for one node's task, which is what `--member` names.
fn member_id(snapshot: &Snapshot, id: &str) -> String {
    format!(
        "{SHADOW_SOURCE}:{}/{}",
        project_file(&snapshot.project),
        task_file(id)
    )
}

/// What one attempt carries, decided before it reads anything.
enum Carry {
    /// Every node, as a copy naming no member carries them.
    Whole(WholeBecause),
    /// The nodes whose shadow task changed since the last attempt that landed — possibly none,
    /// which is a copy of the project item alone.
    Members(BTreeSet<String>),
}

impl Carry {
    /// Whole where it has to be, and otherwise exactly the lineages that changed.
    ///
    /// Whole when the store offers no member copy, when the attempt before this one failed —
    /// what the destination holds is then not known to be the last success — and when nothing
    /// has landed in this worker yet, in that order of precedence. A lineage changed when its
    /// shadow task, rendered from the snapshot alone, differs from the one the last success
    /// rendered under the same root, or when that success rendered none. Project-level
    /// metadata is not a node: the project item every copy includes carries it. A node never
    /// leaves a snapshot — every node the plan or an edit ever held is in one — so there is no
    /// item to carry away: a dropped node changes by its word turning `cancelled`, and a retry
    /// changes its lineage's one task by giving it a new head, each carried like any other
    /// change under the root's member, which
    /// `live_edit::retry_cancel_requeue_and_drop_are_projected_after_their_rulings` drives to
    /// the board.
    fn decide(
        members: bool,
        after_failure: bool,
        last: Option<&Snapshot>,
        snapshot: &Snapshot,
    ) -> Self {
        if !members {
            return Self::Whole(WholeBecause::StoreLacksMembers);
        }
        if after_failure {
            return Self::Whole(WholeBecause::AfterFailure);
        }
        let Some(last) = last else {
            return Self::Whole(WholeBecause::First);
        };
        let (now, before) = (snapshot.lineages(), last.lineages());
        Self::Members(
            now.roots()
                .filter(|root| {
                    task_document(last, &before, root, None).ok()
                        != task_document(snapshot, &now, root, None).ok()
                })
                .cloned()
                .collect(),
        )
    }

    /// The lineage roots the copy carries: every one, for a whole copy.
    fn items(&self, snapshot: &Snapshot) -> Vec<String> {
        match self {
            Self::Whole(_) => snapshot.lineages().roots().cloned().collect(),
            Self::Members(named) => named.iter().cloned().collect(),
        }
    }
}

/// What the worker carries from one attempt that landed to the next.
#[derive(Default)]
struct Carried {
    /// The snapshot that attempt projected, which the next one is compared against.
    last: Option<Snapshot>,
    /// Each lineage's destination item as that attempt left it: read by the last whole
    /// projection's page of tasks, re-read for every member named since, and updated by what
    /// each copy reported creating. A member copy's unnamed members are written into the shadow
    /// from here rather than read again.
    origins: BTreeMap<String, Origin>,
}

/// What the destination holds for the nodes a member copy names, read one named member at a
/// time and nothing else.
///
/// Every unnamed member's origin comes from what the run already carries, so it is never read.
/// A named member is read again, because its labels are the destination's own and a person may
/// have changed them since. A named node the destination holds no item for is created by the
/// copy, so there is nothing to read.
fn member_origins(
    binary: &Path,
    launch_dir: &Path,
    run_dir: &Path,
    named: &BTreeSet<String>,
    known: &BTreeMap<String, Origin>,
) -> Result<BTreeMap<String, Origin>, Failed> {
    let mut origins = known.clone();
    for node in named {
        let Some(origin) = origins.get_mut(node) else {
            continue;
        };
        let args = ["task", "show", origin.id.as_str(), "--json"];
        let output = bounded_output(
            binary,
            launch_dir,
            run_dir,
            TASK_SHOW,
            &args,
            Deadline::Floor,
        )?;
        if !output.status.success() {
            let reason = format!(
                "task show exited {}: {}",
                exit(&output.status),
                String::from_utf8_lossy(&output.stderr).trim()
            );
            return Err(Failed::answered(&output, reason));
        }
        // llmlint: ignore-block[changed_behavior_has_e2e] These refusals defend the compiled
        // sibling's machine contract, exactly as `destination_project`'s do. Producing
        // malformed JSON, partial results, no task or a different task here requires
        // replacing the real onetaskgraph executable with a scripted mock; the member journey
        // drives the successful read and the carried-through labels end to end.
        let response: TaskPage = answered(&output.stdout)?;
        if !response.errors.is_empty() {
            return Err("task show returned partial results".to_owned().into());
        }
        let mut items = response.items.into_iter();
        let task = items
            .next()
            .ok_or_else(|| format!("task '{}' was not found", origin.id))?;
        if items.next().is_some() || task.id != origin.id {
            return Err(format!("task show returned the wrong task for '{}'", origin.id).into());
        }
        // llmlint: ignore-end[changed_behavior_has_e2e]
        origin.labels = task.item.labels;
        origin.category = task.item.status.map(|status| status.category);
    }
    Ok(origins)
}

/// The store's answer to a `project copy` that landed: what it did to each item it carried,
/// and what it spent.
///
/// Read leniently, like every answer the store composes, and never required: a report this
/// build cannot read leaves the record's `actions` and `spent` unsaid rather than failing a
/// copy the store says landed.
#[derive(Deserialize)]
struct CopyReport {
    items: Vec<CopiedItem>,
    /// Verbatim: the store's own account, absent where no source in the command meters.
    #[serde(default)]
    spent: Option<Map<String, Value>>,
    /// Verbatim: what the store did to each ticket a carried task delivers, absent from a store
    /// that has no `delivers`.
    // llmlint: ignore[boundary_inputs_validated] kept as the store wrote it because the
    // projection record is required to carry each attempt's `delivered` entries verbatim, exactly
    // as it carries `spent`: a reader of the record is owed what the store said, including an
    // entry this build could not read. Nothing interprets this copy — every decision and every
    // sentence about a ticket reads the same answer through `DeliveredAnswer`, whose typed entry
    // validates both ids, the outcome and the failure at the boundary.
    #[serde(default)]
    delivered: Vec<Map<String, Value>>,
}

#[derive(Deserialize)]
struct CopiedItem {
    source: QualifiedId,
    action: CopiedAction,
    #[serde(default)]
    destination: Option<QualifiedId>,
}

/// What a copy did to one item. An action this build has never heard of leaves the report
/// unread, so its counts are never missing one.
#[derive(Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
enum CopiedAction {
    Created,
    Updated,
    Unchanged,
    Orphaned,
}

impl CopyReport {
    /// How many items the copy did each thing to, the project item included — and, derived
    /// here rather than reported by the store, how many it reopened.
    ///
    /// An item was reopened when the store says it `updated` it, the attempt's own pre-copy
    /// read (`before`, off `task list` for a whole copy and `task show` for a member copy)
    /// reported its category `done` or `cancelled`, and the word this snapshot projects its
    /// lineage under is neither. The store's copy-report vocabulary is not extended for it:
    /// an action word an older build has not heard of makes it drop the whole report and then
    /// re-create items.
    fn actions(&self, snapshot: &Snapshot, before: &BTreeMap<String, Origin>) -> ProjectionActions {
        let lineages = snapshot.lineages();
        let members = shadow_members(snapshot, &lineages);
        let mut actions = ProjectionActions::default();
        for item in &self.items {
            let count = match item.action {
                CopiedAction::Created => &mut actions.created,
                CopiedAction::Updated => &mut actions.updated,
                CopiedAction::Unchanged => &mut actions.unchanged,
                CopiedAction::Orphaned => &mut actions.orphaned,
            };
            *count = count.saturating_add(1);
            if item.action != CopiedAction::Updated {
                continue;
            }
            let reopened = members.get(item.source.as_str()).is_some_and(|root| {
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
    /// the next member copy naming it would carry no origin and create it a second time. An
    /// item that is not one of this snapshot's shadow tasks says nothing about a lineage.
    fn learn(&self, origins: &mut BTreeMap<String, Origin>, snapshot: &Snapshot) {
        let lineages = snapshot.lineages();
        let members = shadow_members(snapshot, &lineages);
        for item in &self.items {
            let (Some(node), Some(destination)) =
                (members.get(item.source.as_str()), item.destination.as_ref())
            else {
                continue;
            };
            if item.action == CopiedAction::Orphaned {
                continue;
            }
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
}

/// Each lineage's shadow member id, mapped back to its root: what a copy report's `source`
/// names.
fn shadow_members<'a>(snapshot: &Snapshot, lineages: &'a Lineages) -> BTreeMap<String, &'a String> {
    lineages
        .roots()
        .map(|root| (member_id(snapshot, root), root))
        .collect()
}

/// Whether this run's store offers a member copy, decided once for the run.
///
/// Read off [`WRITEBACK_STORE_FILE`] where an earlier driver of the run decided it, and
/// otherwise decided from the version the launch check's `--version` already read and written
/// there before the first projection. Never found out by attempting a copy: against a store
/// without `--member` that is a failed attempt, and the attempt after a failure is whole — the
/// cost a member copy exists to remove. A record that does not read is decided again and
/// replaced.
fn decide_member_copy_once(run_dir: &Path, reported: &str) -> bool {
    let path = run_dir.join(WRITEBACK_STORE_FILE);
    if let Some(recorded) = std::fs::read(&path)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<StoreRecord>(&bytes).ok())
    {
        return recorded.members();
    }
    let recorded = StoreRecord {
        version: reported.to_owned(),
    };
    let members = recorded.members();
    // llmlint: ignore-block[changed_behavior_has_e2e] writing a small file into the run's own
    // directory fails only on a host whose run directory has been made unwritable, which no
    // CLI journey arranges; the answer is still used for this driver, and a later driver that
    // finds no record decides it again the same way.
    let written = serde_json::to_vec(&recorded)
        .map_err(|error| error.to_string())
        .and_then(|bytes| std::fs::write(&path, bytes).map_err(|error| error.to_string()));
    if let Err(error) = written {
        eprintln!(
            "onetaskgraph write-back could not record whether the store offers a member copy \
             at {}: {error}",
            path.display()
        );
    }
    // llmlint: ignore-end[changed_behavior_has_e2e]
    members
}

/// What [`WRITEBACK_STORE_FILE`] holds: the version the run's answer was decided from.
///
/// Written with the decision beside it, for a reader, and read back only where the two agree.
/// The decision is derived from the version rather than kept beside it, so a record whose
/// `members` says something its `version` does not is no record of this run's answer, and the
/// answer is decided again.
#[derive(Clone, Serialize, Deserialize)]
#[serde(try_from = "StoreWire", into = "StoreWire")]
struct StoreRecord {
    // llmlint: ignore[invalid_states_unrepresentable] the token the store printed, recorded
    // verbatim for a reader and read by `taskgraph::at_least` for the one decision made of it;
    // a token that does not read as a version is a store offering no member copy, which is an
    // answer rather than an invalid record.
    version: String,
}

impl StoreRecord {
    /// Whether the store this was decided from offers a member copy.
    fn members(&self) -> bool {
        crate::taskgraph::at_least(&self.version, WRITEBACK_MEMBERS_FROM)
    }
}

/// The two keys [`StoreRecord`] is written as.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoreWire {
    version: String,
    members: bool,
}

impl TryFrom<StoreWire> for StoreRecord {
    type Error = String;

    fn try_from(wire: StoreWire) -> Result<Self, String> {
        let record = Self {
            version: wire.version,
        };
        if record.members() == wire.members {
            Ok(record)
        } else {
            Err(format!(
                "the store record says `members: {}` of version {:?}, which says otherwise",
                wire.members, record.version
            ))
        }
    }
}

impl From<StoreRecord> for StoreWire {
    fn from(record: StoreRecord) -> Self {
        let members = record.members();
        Self {
            version: record.version,
            members,
        }
    }
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
/// What a projection's cost is read off: what it carried and why, how it ended, how long it
/// took, and what the store said it spent. Entry 73 of `docs/contract-divergences.md` states
/// the shape and is the one source for it. The wire form is flat — every key written, `null`
/// where it says nothing — and this is the shape of it that cannot say a contradiction: a
/// member copy has no reason to be whole, and a failed attempt names neither `actions` nor
/// `spent`, the halves of a copy report only a landed one has. `delivered` is the half a
/// partial copy reports as it fails, so it is kept either way.
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
}

/// What one projection attempt carried.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProjectionScope {
    /// Every node of the plan, for the reason given.
    Whole(WholeBecause),
    /// Only the nodes whose projection changed since the last attempt that landed.
    Members,
}

/// Why a projection attempt carried the whole project.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum WholeBecause {
    /// Nothing has landed in this driver yet: its first projection, including one an `adopt`
    /// started.
    First,
    /// The attempt before this one failed.
    AfterFailure,
    /// The store reported a version older than
    /// [`WRITEBACK_MEMBERS_FROM`](crate::cli::WRITEBACK_MEMBERS_FROM).
    StoreLacksMembers,
}

/// How one projection attempt ended.
#[derive(Debug, Clone, PartialEq)]
pub enum ProjectionEnded {
    /// The copy landed.
    Projected {
        /// How many items the copy report says it did each thing to, or `None` where no
        /// report was read.
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
    fn of(
        at: String,
        project: &QualifiedId,
        carry: &Carry,
        items: Vec<String>,
        took: Duration,
        attempt: &Result<Landed, Failed>,
    ) -> Self {
        Self {
            at,
            project: project.as_str().to_owned(),
            scope: match carry {
                Carry::Whole(because) => ProjectionScope::Whole(*because),
                Carry::Members(_) => ProjectionScope::Members,
            },
            items,
            duration_ms: u64::try_from(took.as_millis()).unwrap_or(u64::MAX),
            ended: match attempt {
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
            delivered: match attempt {
                Ok(landed) => landed.delivered.clone(),
                Err(failed) => failed.delivered.clone(),
            },
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
}

/// The version of a projection line that names none: the shape before `delivered` existed.
fn unversioned_projection_line() -> u32 {
    1
}

/// The version that added `delivered`, and the last that named no `actions.reopened`.
const PROJECTION_LINE_WITH_DELIVERED: u32 = 2;

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
        match wire.schema_version {
            WRITEBACK_PROJECTIONS_SCHEMA_VERSION => {
                if wire.actions.is_some() && !names_reopened {
                    return Err(format!(
                        "a version {WRITEBACK_PROJECTIONS_SCHEMA_VERSION} line names `actions` \
                         without `actions.reopened`, which every landed attempt at that version \
                         names"
                    ));
                }
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
                     {WRITEBACK_PROJECTIONS_SCHEMA_VERSION} added",
                    wire.schema_version
                ))
            }
            1 | PROJECTION_LINE_WITH_DELIVERED => {}
            other => {
                return Err(format!(
                    "`schema_version` {other} is not one this build reads (1, \
                     {PROJECTION_LINE_WITH_DELIVERED}, {WRITEBACK_PROJECTIONS_SCHEMA_VERSION})"
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
            schema_version: WRITEBACK_PROJECTIONS_SCHEMA_VERSION,
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
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        classified, per_item_budget, projected, write_shadow, Claim, Classified, Deadline,
        DestinationCategory, DestinationLabel, DestinationProjectItem, FailureClass, Landing,
        Origin, Pending, ProjectedStatus, Snapshot, WorkerState, Writeback, CHANGE_URL_KEY,
        COMMAND_FLOOR, DELIVERS_FIELD, ID_KEY, LANDING_COMMIT_KEY, LANDING_EVIDENCE_KEY,
        LANDING_KEY, NODE_KEY, SUPERSEDES_KEY,
    };
    use crate::cli::DEFAULT_WRITEBACK_ITEM_BUDGET_SECONDS;
    use crate::graph::NodeStatus;
    use crate::ledger::{LaunchRecord, RunPaths};
    use crate::plan::Node;
    use crate::projection::RunState;
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

    /// The copy's deadline is the per-item budget multiplied by the item count, and never
    /// below the floor; the refusal says which governed and what it was computed from.
    ///
    /// The incident: 34 items under the fixed sixty seconds. Under the shipped budget that
    /// run is allowed 340, and the line an operator reads says so — and a plan the floor
    /// still governs says that instead, rather than a figure that was never the deadline.
    #[test]
    fn the_copy_deadline_is_the_budget_times_the_items_and_never_below_the_floor() {
        let shipped = DEFAULT_WRITEBACK_ITEM_BUDGET_SECONDS;
        let copy = |items: usize| Deadline::Copy {
            per_item: shipped,
            items,
        };
        assert_eq!(copy(34).within(), Duration::from_secs(340));
        assert_eq!(
            copy(34).refusal("project-copy"),
            "project-copy exceeded 340 seconds (34 items × 10 seconds per item)"
        );

        assert_eq!(copy(2).within(), COMMAND_FLOOR);
        assert_eq!(
            copy(2).refusal("project-copy"),
            "project-copy exceeded 60 seconds (the 60 second floor; 2 items × 10 seconds \
             per item is less)"
        );

        // Exactly the floor is the product, and said as the product: the floor did not
        // lift anything.
        assert_eq!(copy(6).within(), COMMAND_FLOOR);
        assert_eq!(
            copy(6).refusal("project-copy"),
            "project-copy exceeded 60 seconds (6 items × 10 seconds per item)"
        );

        // One of each, in the singular, so the line reads as a sentence.
        let one = Deadline::Copy {
            per_item: NonZeroU64::MIN,
            items: 1,
        };
        assert_eq!(
            one.refusal("project-copy"),
            "project-copy exceeded 60 seconds (the 60 second floor; 1 item × 1 second per \
             item is less)"
        );

        // The reads are the floor alone, and their refusal is the line it always was.
        assert_eq!(Deadline::Floor.within(), COMMAND_FLOOR);
        assert_eq!(
            Deadline::Floor.refusal("project-show"),
            "project-show exceeded 60 seconds"
        );
        assert_eq!(
            Deadline::Floor.refusal("task-list"),
            "task-list exceeded 60 seconds"
        );

        // The product is exact for every count a `u64` of seconds can carry — well past
        // the four billion a narrower multiplication would have capped at — and saturates
        // to `u64::MAX` seconds beyond that, rather than wrapping to a figure the floor
        // would then govern.
        let vast = Deadline::Copy {
            per_item: NonZeroU64::MIN,
            items: usize::MAX / 2,
        };
        assert_eq!(vast.within(), Duration::from_secs(usize::MAX as u64 / 2));
        let saturated = Deadline::Copy {
            per_item: NonZeroU64::MAX,
            items: 2,
        };
        assert_eq!(saturated.within(), Duration::from_secs(u64::MAX));
        assert_eq!(
            saturated.refusal("project-copy"),
            format!(
                "project-copy exceeded {} seconds (2 items × {} seconds per item)",
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
            Some("max(floor_seconds, budget × items)"),
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

    /// Entry 72's rule is the one the worker classifies by: the member and value it names
    /// make a failure document refused, the commands it names are the three an attempt runs,
    /// and a partial answer is refused only where every entry is.
    ///
    /// `tests/contract.rs` holds the same block against the published constants; the
    /// classifier and the type it reads through are private, so this is where the block
    /// meets what actually decides whether a retry is scheduled.
    #[test]
    fn the_divergence_records_refusal_rule_is_the_one_the_worker_classifies_by() {
        let block = divergence_block("72.");
        let rule = &block["failure"];
        assert_eq!(
            rule["commands"],
            json!([super::PROJECT_SHOW, super::TASK_LIST, super::PROJECT_COPY]),
            "entry 72 names other commands than the three an attempt runs"
        );

        let stops = rule["stops_the_timer"]
            .as_str()
            .expect("entry 72 names the class that stops the timer");
        let (object, member) = rule["member"]
            .as_str()
            .and_then(|path| path.split_once('.'))
            .expect("entry 72 names the member as a path");
        let document = |class: &str| {
            let mut failure = Map::new();
            failure.insert(member.to_owned(), json!(class));
            failure.insert("kind".to_owned(), json!("stale-origin"));
            failure.insert("source".to_owned(), Value::Null);
            failure.insert("message".to_owned(), json!("the store's own words"));
            failure.insert("retry_after_seconds".to_owned(), Value::Null);
            let mut document = Map::new();
            document.insert(object.to_owned(), Value::Object(failure));
            Value::Object(document).to_string()
        };
        let exit = rule["failure_document_exit"]
            .as_i64()
            .and_then(|code| i32::try_from(code).ok())
            .expect("entry 72 names the exit a failure document is written under");
        let refused = Some(Classified {
            class: FailureClass::Refused,
            kind: "stale-origin".to_owned(),
        });
        assert_eq!(
            classified(Some(exit), document(stops).as_bytes()),
            refused,
            "the document entry 72 describes is not one the worker reads as refused"
        );
        assert_eq!(
            classified(Some(exit), document("transient").as_bytes()).map(|c| c.class),
            Some(FailureClass::Transient)
        );
        // Unclassified, so on the schedule: the same document under an exit that does not
        // carry one, a class nobody has named, words that are not a document at all, and a
        // command that ended on a signal.
        assert_eq!(classified(Some(2), document(stops).as_bytes()), None);
        assert_eq!(classified(Some(exit), document("maybe").as_bytes()), None);
        assert_eq!(classified(Some(exit), b"no project with that id"), None);
        assert_eq!(classified(None, document(stops).as_bytes()), None);

        let partial = &rule["partial_answer"];
        assert_eq!(
            partial["refused_when"].as_str(),
            Some("every"),
            "entry 72 states a partial-answer rule other than the one the worker applies"
        );
        let partial_exit = partial["exit"]
            .as_i64()
            .and_then(|code| i32::try_from(code).ok())
            .expect("entry 72 names the exit a partial answer is written under");
        let (list, member) = partial["member"]
            .as_str()
            .and_then(|path| path.split_once("[]."))
            .expect("entry 72 names the partial member as a path");
        let answer = |classes: &[Option<&str>]| {
            let entries: Vec<Value> = classes
                .iter()
                .enumerate()
                .map(|(n, class)| {
                    let mut entry = Map::new();
                    entry.insert("source".to_owned(), json!(format!("source{n}")));
                    entry.insert(
                        "error".to_owned(),
                        json!({"kind": if n == 0 { "config" } else { "unavailable" },
                               "message": "the store's own words"}),
                    );
                    if let Some(class) = class {
                        entry.insert(member.to_owned(), json!(class));
                    }
                    Value::Object(entry)
                })
                .collect();
            let mut answer = Map::new();
            answer.insert("items".to_owned(), json!([]));
            answer.insert("next".to_owned(), Value::Null);
            answer.insert(list.to_owned(), Value::Array(entries));
            Value::Object(answer).to_string()
        };
        assert_eq!(
            classified(Some(partial_exit), answer(&[Some(stops)]).as_bytes()),
            Some(Classified {
                class: FailureClass::Refused,
                kind: "config".to_owned(),
            })
        );
        assert_eq!(
            classified(
                Some(partial_exit),
                answer(&[Some(stops), Some("transient")]).as_bytes()
            ),
            Some(Classified {
                class: FailureClass::Transient,
                kind: "config, unavailable".to_owned(),
            }),
            "a partial answer with one entry a wait could change was read as refused"
        );
        assert_eq!(
            classified(Some(partial_exit), answer(&[]).as_bytes()),
            None,
            "a partial answer naming no failure was classified"
        );
        assert_eq!(
            classified(Some(partial_exit), answer(&[Some(stops), None]).as_bytes()),
            None,
            "an entry from a store that writes no class was read as a refusal"
        );
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
            vocabulary: super::Vocabulary::WithDelivers,
            per_item: DEFAULT_WRITEBACK_ITEM_BUDGET_SECONDS,
            told_why_not_delivered: std::sync::atomic::AtomicBool::new(false),
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
    /// when, and only when, `worker` returns. The destination refuses at the same
    /// subprocess boundary a rate-limited one does, because the binary the run names is not
    /// installed, which is a refusal this test can arrange without a store at all.
    #[test]
    fn a_stop_reaches_a_worker_that_is_waiting_out_a_retry_interval() {
        let dir = scratch("stop-mid-wait");
        let paths = RunPaths {
            run: "stopmidwait".to_owned(),
            dir: dir.to_path_buf(),
        };
        let launch = a_launch(&paths);
        let writeback = Writeback::start(
            dir.join("onetaskgraph-nobody-installed"),
            "",
            &paths,
            &launch,
        )
        .expect("a write-back worker");
        writeback.publish(&paths, &launch, &RunState::default(), &BTreeMap::new());

        // Four attempts in, the interval the worker is now waiting out is longer than every
        // one before it — so the last one this test actually watched is a lower bound on
        // what a stop would have to sit through if it were not honoured.
        let waited = intervals_between_attempts(&paths, 4);
        let outstanding = *waited.last().expect("an interval between attempts");
        assert!(
            outstanding >= Duration::from_secs(2),
            "the streak is not deep enough for a stop to have anything to wait out: {waited:?}"
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
        assert!(
            !attempt_capture(&paths).exists(),
            "the worker asked the destination again on its way out"
        );
    }

    /// The wall-clock intervals between the worker's next `count` attempts.
    ///
    /// An attempt is counted where the destination's side of the boundary is: the capture
    /// file the worker creates for the store command it is about to run. This takes each
    /// one away again, so the next one appearing is the next attempt rather than the same
    /// file read twice — which a modification time this filesystem may round would not tell
    /// apart.
    fn intervals_between_attempts(paths: &RunPaths, count: usize) -> Vec<Duration> {
        let capture = attempt_capture(paths);
        let deadline = Instant::now() + Duration::from_secs(60);
        let mut at: Vec<Instant> = Vec::new();
        while at.len() < count {
            assert!(
                Instant::now() < deadline,
                "the worker made {} attempts, not the {count} this test reads its intervals \
                 off",
                at.len()
            );
            if capture.exists() {
                at.push(Instant::now());
                std::fs::remove_file(&capture).expect("the capture file is taken away");
            } else {
                std::thread::sleep(Duration::from_millis(5));
            }
        }
        at.windows(2).map(|pair| pair[1] - pair[0]).collect()
    }

    fn attempt_capture(paths: &RunPaths) -> PathBuf {
        paths.dir.join("writeback-project-show.stderr")
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
            Duration::from_secs(120),
            "the launch wait did not follow the 12 × 10 second deadline of the queued copy"
        );
    }

    /// The gate over `WRITEBACK_DELIVERS_FROM`, on both sides of the boundary: the release it
    /// names and every later one offer `queued` and `delivers`, and the release before it, a
    /// pre-release of it, and a version that does not read offer neither. What each side writes
    /// follows from it: a driven unstarted node is `queued` and its shadow task carries `delivers`
    /// on one side, and `todo` with no `delivers` on the other — and no reserved key carries the
    /// tickets on either.
    #[test]
    fn the_store_vocabulary_gate_holds_on_both_sides_of_the_first_release_carrying_delivers() {
        use super::Vocabulary;
        let from = crate::cli::WRITEBACK_DELIVERS_FROM;
        assert_eq!(Vocabulary::of(from), Vocabulary::WithDelivers);
        assert_eq!(Vocabulary::of("0.3.0"), Vocabulary::WithDelivers);
        for older in ["0.2.31", "0.2.32-rc.1", "0.1.0", ""] {
            assert_eq!(Vocabulary::of(older), Vocabulary::BeforeDelivers, "{older}");
        }
        for (vocabulary, word, carried) in [
            (
                Vocabulary::WithDelivers,
                "queued",
                Some(json!(["tickets:t-1"])),
            ),
            (Vocabulary::BeforeDelivers, "todo", None),
        ] {
            let snapshot = Fixture::new("vocabulary").snapshot_with(|snapshot| {
                snapshot.claim = vocabulary.driving_claim();
                snapshot.vocabulary = vocabulary;
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
            assert_eq!(front["status"], word, "{vocabulary:?}");
            assert_eq!(front.get("delivers").cloned(), carried, "{vocabulary:?}");
            assert!(
                front["metadata"].get("onepipeline.delivers").is_none(),
                "a reserved key carries the tickets: {front}"
            );
        }
    }

    /// A copy report's failed tickets are described only where every failed entry reads: a
    /// well-formed one is named with its deliverer and the store's words on one line, a report
    /// failing no ticket describes none, and an entry that does not validate — an unqualified
    /// ticket, an unknown outcome, a failure with nothing in it — describes nothing at all.
    #[test]
    fn a_delivered_report_is_described_only_where_every_failed_entry_reads() {
        use super::failed_deliveries;
        let failure = json!({"class": "transient", "kind": "unavailable", "source": "tickets",
                             "message": "cannot write\nnext: fix it", "retry_after_seconds": null});
        let entry = |ticket: &str, outcome: &str, failure: Value| {
            json!({"ticket": ticket, "deliverer": "plans:p/a", "outcome": outcome,
                   "from": "queued", "failure": failure})
        };
        let report = |entries: Vec<Value>| {
            json!({"items": [], "delivered": entries})
                .to_string()
                .into_bytes()
        };
        assert_eq!(
            failed_deliveries(&report(vec![entry(
                "tickets:t/one",
                "failed",
                failure.clone()
            )]))
            .as_deref(),
            Some("ticket tickets:t/one (delivered by plans:p/a): cannot write next: fix it")
        );
        assert_eq!(
            failed_deliveries(&report(vec![json!({
                "ticket": "tickets:t/two", "deliverer": "plans:p/a", "outcome": "written",
                "from": "todo", "to": "queued"
            })])),
            None,
            "a report failing no ticket described one"
        );
        for unreadable in [
            entry("not qualified", "failed", failure.clone()),
            entry("tickets:t/one", "exploded", failure.clone()),
            entry("tickets:t/one", "failed", Value::Null),
        ] {
            assert_eq!(
                failed_deliveries(&report(vec![unreadable.clone()])),
                None,
                "an entry that does not validate was described: {unreadable}"
            );
        }
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
    fn preserved_of(destination: &DestinationProjectItem) -> Value {
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
        destination: DestinationProjectItem,
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
                    vocabulary: super::Vocabulary::WithDelivers,
                },
                // Only `build`. A node the plan has just added has no destination
                // task at all, and holding both kinds in one fixture is what makes
                // "carried through" and "invented none" separable answers.
                origins: BTreeMap::from([(
                    "build".to_owned(),
                    Origin {
                        id: "plans:board/002-build".parse().expect("a qualified task"),
                        labels: labels(&[("needs-review", Some("d73a4a"))]),
                        category: Some(DestinationCategory::Done),
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
        destination: &DestinationProjectItem,
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
    /// Built out of the sibling's own machine response rather than by naming
    /// fields, so the shape this projection reads is the shape `project show
    /// --json` answers in.
    fn destination(title: &str, labels: &[&str]) -> DestinationProjectItem {
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
        .expect("the sibling's own project response")
    }

    fn labels(named: &[(&str, Option<&str>)]) -> Vec<DestinationLabel> {
        serde_json::from_value(json!(named
            .iter()
            .map(|(name, color)| json!({"id": name, "name": name, "color": color}))
            .collect::<Vec<_>>()))
        .expect("the sibling's own labels")
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
        decide_member_copy_once, member_id, Carry, CopyReport, ProjectionActions, WholeBecause,
    };
    use crate::cli::{WRITEBACK_MEMBERS_FROM, WRITEBACK_STORE_FILE};

    /// The node ids a decision carries as members, refusing a whole one.
    fn named(carry: &Carry) -> Vec<String> {
        match carry {
            Carry::Members(named) => named.iter().cloned().collect(),
            Carry::Whole(because) => panic!("a member projection was decided whole: {because:?}"),
        }
    }

    /// After a success, a projection carries exactly the nodes whose shadow task changed: a
    /// node whose word on the board moved, a node the success did not hold, and nothing for a
    /// change that renders no differently or that only the project item carries.
    #[test]
    fn a_member_projection_carries_exactly_the_nodes_whose_shadow_task_changed() {
        let fixture = Fixture::new("carry");
        let last = fixture.snapshot.clone();

        assert_eq!(
            named(&Carry::decide(true, false, Some(&last), &last)),
            Vec::<String>::new(),
            "a snapshot the last success already projected carried a node"
        );

        let mut one = last.clone();
        one.statuses.insert("build".to_owned(), NodeStatus::Running);
        assert_eq!(
            named(&Carry::decide(true, false, Some(&last), &one)),
            ["build"]
        );

        let mut both = one.clone();
        both.settlements
            .insert("design".to_owned(), json!({"status": "done"}));
        assert_eq!(
            named(&Carry::decide(true, false, Some(&last), &both)),
            ["build", "design"]
        );

        // Pending and ready are both `todo` on the board, so moving between them is no change
        // to what a copy would write.
        let mut pending = last.clone();
        pending
            .statuses
            .insert("design".to_owned(), NodeStatus::Pending);
        let mut ready = last.clone();
        ready
            .statuses
            .insert("design".to_owned(), NodeStatus::Ready);
        assert_eq!(
            named(&Carry::decide(true, false, Some(&pending), &ready)),
            Vec::<String>::new()
        );

        let mut added = last.clone();
        let mut verify = added.nodes["design"].clone();
        verify.id = "verify".to_owned();
        added.nodes.insert("verify".to_owned(), verify);
        assert_eq!(
            named(&Carry::decide(true, false, Some(&last), &added)),
            ["verify"]
        );

        let mut project_level = last.clone();
        project_level
            .project_metadata
            .insert("onepipeline.goal".to_owned(), json!("a goal restated"));
        assert_eq!(
            named(&Carry::decide(true, false, Some(&last), &project_level)),
            Vec::<String>::new(),
            "a change only the project item carries named a task"
        );
    }

    /// Whole where it has to be, and the reason recorded is the first of entry 73's precedence
    /// that applies — for every combination of the three, so a block that reordered them and a
    /// worker that decided in another order cannot both pass.
    #[test]
    fn a_projection_is_whole_for_the_first_reason_entry_73_ranks_that_applies() {
        let block = divergence_block("73.");
        let precedence: Vec<String> =
            serde_json::from_value(block["projection"]["whole_because_precedence"].clone())
                .expect("entry 73 ranks the reasons a projection is whole");
        let reasons: std::collections::BTreeSet<String> = block["projection"]["whole_because"]
            .as_object()
            .expect("entry 73 names each reason")
            .keys()
            .cloned()
            .collect();
        assert_eq!(
            precedence
                .iter()
                .cloned()
                .collect::<std::collections::BTreeSet<_>>(),
            reasons,
            "entry 73 ranks other reasons than it names"
        );
        let snapshot = Fixture::new("whole").snapshot.clone();
        for members in [true, false] {
            for after_failure in [true, false] {
                for landed in [true, false] {
                    let applies = |reason: &str| match reason {
                        "store-lacks-members" => !members,
                        "after-failure" => after_failure,
                        "first" => !landed,
                        other => {
                            panic!("entry 73 names a reason the worker never decides: {other}")
                        }
                    };
                    let expected = precedence.iter().find(|reason| applies(reason));
                    let decided = Carry::decide(
                        members,
                        after_failure,
                        landed.then_some(&snapshot),
                        &snapshot,
                    );
                    match (expected, decided) {
                        (Some(reason), Carry::Whole(because)) => assert_eq!(
                            serde_json::to_value(because).expect("a reason serializes"),
                            json!(reason),
                            "members {members}, after a failure {after_failure}, landed {landed}"
                        ),
                        (None, Carry::Members(_)) => {}
                        (expected, Carry::Whole(because)) => panic!(
                            "decided whole ({because:?}) where entry 73 expects {expected:?}"
                        ),
                        (expected, Carry::Members(_)) => {
                            panic!("decided members where entry 73 expects {expected:?}")
                        }
                    }
                }
            }
        }
        assert_eq!(
            serde_json::to_value(WholeBecause::StoreLacksMembers).expect("serializes"),
            json!("store-lacks-members")
        );
    }

    /// A copy report is counted item by item, the project item included, and teaches the run
    /// where a node the copy created landed — without which the next member copy naming it would
    /// create it again. An item that is no shadow task of this snapshot teaches nothing, and an
    /// action this build has never heard of leaves the report unread rather than miscounted.
    #[test]
    fn a_copy_report_is_counted_and_teaches_the_run_where_a_created_node_landed() {
        let fixture = Fixture::new("report");
        let snapshot = &fixture.snapshot;
        let report: CopyReport = serde_json::from_value(json!({
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
            report.actions(snapshot, &fixture.origins),
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
            report.spent,
            Some(
                json!({"requests": 3, "budgets": []})
                    .as_object()
                    .cloned()
                    .expect("an object")
            )
        );

        let mut origins = fixture.origins.clone();
        report.learn(&mut origins, snapshot);
        assert_eq!(
            origins.len(),
            2,
            "an item that is no node's shadow task taught a node"
        );
        assert_eq!(origins["design"].id.as_str(), "plans:board/003-design");
        assert!(origins["design"].labels.is_empty());
        assert_eq!(origins["build"].id.as_str(), "plans:board/002-build");
        assert_eq!(
            origins["build"].labels.len(),
            1,
            "an updated node the run already knew lost the labels it was read with"
        );

        assert!(
            serde_json::from_value::<CopyReport>(json!({
                "items": [{"source": "plans:board/a", "action": "moved", "destination": "plans:board/b"}]
            }))
            .is_err(),
            "a report naming an action this build does not know was read"
        );
    }

    /// Whether the store offers a member copy is decided from the version once, recorded in the
    /// run's directory in the shape entry 73 states, and read back from there by every later
    /// decision — so a driver reading a newer version keeps the run's answer.
    #[test]
    fn whether_the_store_offers_a_member_copy_is_decided_once_and_read_back_after() {
        let block = divergence_block("73.");
        let detection = &block["detection"];
        assert_eq!(
            detection["members_from"].as_str(),
            Some(WRITEBACK_MEMBERS_FROM)
        );
        assert!(crate::taskgraph::at_least(
            WRITEBACK_MEMBERS_FROM,
            WRITEBACK_MEMBERS_FROM
        ));
        assert!(crate::taskgraph::at_least("0.3.0", WRITEBACK_MEMBERS_FROM));
        assert!(!crate::taskgraph::at_least(
            "0.2.29",
            WRITEBACK_MEMBERS_FROM
        ));
        assert!(!crate::taskgraph::at_least(
            "0.2.30-rc.1",
            WRITEBACK_MEMBERS_FROM
        ));
        assert!(!crate::taskgraph::at_least("", WRITEBACK_MEMBERS_FROM));

        let dir = scratch("members-record");
        assert!(!decide_member_copy_once(&dir, "0.2.29"));
        let path = dir.join(WRITEBACK_STORE_FILE);
        let recorded: Value =
            serde_json::from_slice(&std::fs::read(&path).expect("the answer is recorded"))
                .expect("the record is JSON");
        assert_eq!(recorded, json!({"version": "0.2.29", "members": false}));
        assert_eq!(
            recorded
                .as_object()
                .map(|record| record.keys().collect::<Vec<_>>()),
            detection["record_example"]
                .as_object()
                .map(|record| record.keys().collect::<Vec<_>>()),
            "the record is not the shape entry 73 states"
        );
        assert!(
            !decide_member_copy_once(&dir, "0.2.30"),
            "a later decision asked the version again rather than reading the run's answer"
        );

        std::fs::write(&path, "not a record").expect("the record is spoiled");
        assert!(
            decide_member_copy_once(&dir, "0.2.30"),
            "a record that does not read was kept"
        );
        assert_eq!(
            serde_json::from_slice::<Value>(&std::fs::read(&path).expect("a record"))
                .expect("JSON"),
            detection["record_example"],
        );

        // A record whose decision disagrees with its version is no record of the answer.
        std::fs::write(&path, r#"{"version": "0.2.29", "members": true}"#)
            .expect("a contradictory record is written");
        assert!(
            decide_member_copy_once(&dir, "0.2.30"),
            "a record whose `members` its version contradicts was trusted"
        );
        assert_eq!(
            serde_json::from_slice::<Value>(&std::fs::read(&path).expect("a record"))
                .expect("JSON"),
            json!({"version": "0.2.30", "members": true}),
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

        // Every lineage root, in order, is what a whole copy carries and the record names.
        assert_eq!(
            Carry::Whole(WholeBecause::First).items(&fixture.snapshot),
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
            named(&Carry::decide(true, false, Some(&last), &once)),
            ["build", "ship"],
            "a retry named the replacement as a member of its own, or missed the root"
        );

        let mut fixture = Fixture::new("lineage-carry-twice");
        retried(&mut fixture, true);
        let twice = fixture.snapshot.clone();
        assert_eq!(
            named(&Carry::decide(true, false, Some(&once), &twice)),
            ["build"],
            "a second retry of one lineage named something other than its root"
        );
        assert_eq!(
            named(&Carry::decide(true, false, Some(&twice), &twice)),
            Vec::<String>::new()
        );
    }

    /// What the destination holds is folded onto lineage roots, and several items under one
    /// root — what a board an older build wrote holds — resolve to the one at the
    /// furthest-along position: its `onepipeline.node` where it names one, its
    /// `onepipeline.id` otherwise. So the same board after this build has rewritten the head
    /// — an older item still plain at the root beside the head naming `onepipeline.node` —
    /// resolves the same way on every later whole read, rather than refusing two items for
    /// one `onepipeline.id`. Two items at one position stay a refusal naming the attempt. The
    /// rest are not carried, an item of a node no lineage holds stays under its own id, and a
    /// lineage nothing on the board holds has no origin.
    #[test]
    fn several_items_under_one_root_resolve_to_the_furthest_along_attempt() {
        let mut fixture = Fixture::new("furthest-along");
        retried(&mut fixture, true);
        let lineages = fixture.snapshot.lineages();
        let item = |file: &str, id: &str, node: Option<&str>, category: &str| {
            let mut metadata = json!({ID_KEY: id});
            if let Some(node) = node {
                metadata[NODE_KEY] = json!(node);
            }
            serde_json::from_value::<super::DestinationTask>(json!({
                "id": format!("plans:board/{file}"),
                "item": {"labels": [], "metadata": metadata,
                         "status": {"category": category, "name": category}},
            }))
            .expect("the sibling's own task response")
        };
        let place = |items: Vec<super::DestinationTask>| {
            super::furthest_along(
                items
                    .into_iter()
                    .map(|task| super::Placed::of(&lineages, task).expect("an item places"))
                    .collect(),
            )
        };

        // What an older build wrote: one item per attempt, each plain at its own id.
        let older = || {
            vec![
                item("001-build", "build", None, "cancelled"),
                item("004-build-2", "build-2", None, "cancelled"),
                item("002-design", "design", None, "done"),
                item("009-elsewhere", "elsewhere", None, "todo"),
            ]
        };
        let origins = place(older()).expect("an older board resolves");
        assert_eq!(
            origins.keys().cloned().collect::<Vec<_>>(),
            ["build", "design", "elsewhere"],
            "the fold keyed something other than lineage roots and strays"
        );
        assert_eq!(
            origins["build"].id.as_str(),
            "plans:board/004-build-2",
            "the lineage was not resolved to its furthest-along item"
        );
        assert_eq!(origins["design"].id.as_str(), "plans:board/002-design");
        assert!(
            !origins.contains_key("ship"),
            "a lineage the board holds nothing for was given an origin"
        );
        let mut reversed = older();
        reversed.reverse();
        assert_eq!(
            place(reversed).expect("resolves")["build"].id.as_str(),
            "plans:board/004-build-2",
            "the order the page answered in decided the fold"
        );

        // The same board once this build has projected over it: the head rewritten to the
        // root's id, naming its attempt, beside the older item still plain at the root.
        let rewritten = vec![
            item("001-build", "build", None, "cancelled"),
            item("004-build-2", "build", Some("build-3"), "queued"),
        ];
        let origins = place(rewritten).expect("a board this build wrote over resolves again");
        assert_eq!(
            origins["build"].id.as_str(),
            "plans:board/004-build-2",
            "the rewritten head lost to the older item at the root"
        );

        // Two items at one position are two items for one attempt.
        for (said, twice) in [
            (
                "build-2",
                vec![
                    item("004-build-2", "build-2", None, "cancelled"),
                    item("005-build-2", "build", Some("build-2"), "cancelled"),
                ],
            ),
            (
                "build",
                vec![
                    item("001-build", "build", None, "cancelled"),
                    item("003-build", "build", None, "cancelled"),
                ],
            ),
        ] {
            let refused = match place(twice) {
                Err(refused) => refused,
                Ok(resolved) => panic!(
                    "two items at one position resolved to {:?}",
                    resolved.keys().collect::<Vec<_>>()
                ),
            };
            assert_eq!(
                refused,
                format!("project has more than one task for node '{said}'")
            );
        }

        // An item naming an attempt its lineage does not hold places at the root.
        let stray = place(vec![
            item("001-build", "build", Some("nobody"), "cancelled"),
            item("004-build-2", "build-2", None, "cancelled"),
        ])
        .expect("resolves");
        assert_eq!(stray["build"].id.as_str(), "plans:board/004-build-2");

        // With the fold, the shadow task is written onto the furthest-along item.
        fixture.origins = place(older()).expect("resolves");
        fixture.project();
        let (build, _) = fixture.task_document("build");
        assert_eq!(
            build["metadata"]["onetaskgraph.origin"], "plans:board/004-build-2",
            "the lineage was projected onto an item other than the furthest-along one"
        );
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
            json!({"whole": super::TASK_LIST, "members": super::TASK_SHOW})
        );
        assert_eq!(block["reopened"]["store_vocabulary_extended"], false);
        assert_eq!(
            block["reopened"]["record"]["from_schema_version"].as_u64(),
            Some(u64::from(crate::cli::WRITEBACK_PROJECTIONS_SCHEMA_VERSION))
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
                    category: Some(DestinationCategory::Cancelled),
                },
            ),
            (
                "design".to_owned(),
                Origin {
                    id: "plans:board/003-design".parse().expect("a qualified task"),
                    labels: Vec::new(),
                    category: Some(DestinationCategory::Done),
                },
            ),
        ]);
        let item = |root: &str, action: &str| {
            json!({"source": member_id(snapshot, root), "action": action,
                   "destination": format!("plans:board/00x-{root}")})
        };
        let report = |items: Vec<Value>| -> CopyReport {
            serde_json::from_value(json!({"items": items})).expect("the store's report reads")
        };
        let counted = report(vec![
            item("build", "updated"),
            item("design", "updated"),
            item("ship", "created"),
            json!({"source": "elsewhere:board/gone", "action": "updated",
                   "destination": "plans:board/009-gone"}),
        ])
        .actions(snapshot, &before);
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
                .actions(snapshot, &unknown)
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
                .actions(&dropped, &before)
                .reopened,
            0,
            "a cancelled item rewritten cancelled was counted reopened"
        );
        // A store older than `queued` writes `todo`, which is open too.
        let mut released = fixture.snapshot.clone();
        released.claim = Claim::Released;
        assert_eq!(
            report(vec![item("build", "updated")])
                .actions(&released, &before)
                .reopened,
            1
        );
    }
}
