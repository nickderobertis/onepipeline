//! Folding the journal into the state the engine loop and every view read from.
//!
//! **The plan of record is the graph the run is executing.** The run's own
//! `plan.json` is its launch record and is never rewritten — and the project it
//! was read out of is never re-read — so a reader that derived the live graph
//! from either would lose every live edit the reconciler committed — a `retry`
//! replacement's new id, an amended budget, a branch pin. This module folds the
//! run's own authoritative journal instead.
//!
//! There is no round here, and nothing is per-round: the frontier is continuous,
//! so what a node last recorded stands until it records something else.

use std::collections::{BTreeMap, BTreeSet};
use std::num::NonZeroU64;

use serde_json::Value;

use crate::edits::{self, Frontier, Operation};
use crate::event::{Envelope, Source};
use crate::graph::{Graph, Landing, NodeStatus};
use crate::journal;
use crate::plan::Plan;

/// How a run's `stop` left it.
///
/// One value rather than a pair of flags, because two booleans admit a state
/// nothing can mean — a run not stopped whose workers outlived the stop — and
/// every view that reports an in-flight node has to choose exactly one of these
/// sentences about it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum StopState {
    /// No stop has been recorded.
    #[default]
    NotStopped,
    /// A stop was recorded and every process the run had started was signalled.
    WorkersSignalled,
    /// A stop was recorded, but nothing established what became of the run's
    /// workers: this host gave no listing its process tree could be read from,
    /// or the driver is on a host this one cannot reach. They may still be
    /// running.
    WorkersUndetermined,
}

/// Everything the journal says about a run.
#[derive(Debug, Clone, Default)]
pub struct RunState {
    /// The desired graph the loop is converging toward, with every committed
    /// edit applied.
    pub graph: Graph,
    /// The plan the run was launched with, for the fields a graph does not
    /// carry — the goal and the name.
    pub plan: Option<Plan>,
    /// What the journal recorded about each node. A node absent from this map
    /// has not started, which is what `reparent` and `cancel` test for.
    pub recorded: BTreeMap<String, Recorded>,
    /// Each settled node's outcome, when it recorded one.
    pub outcomes: BTreeMap<String, String>,
    /// The branch each settled node left behind, as its dispatch reported it.
    ///
    /// Not the same thing as the branch a node's *plan* pins: this is what the
    /// work actually landed on, which for an unpinned node the sibling named,
    /// and it is the only record of where preserved work is.
    pub branches: BTreeMap<String, String>,
    /// The `onevcs` session each node's **current** dispatch opened.
    ///
    /// Cleared when the node is dispatched again, so what it holds is the
    /// session that dispatch is working in rather than one an earlier attempt
    /// finished with. Read for a node still recorded `running`, which is the
    /// only time the question — *where is the work being done right now?* — has
    /// an answer at all.
    pub sessions: BTreeMap<String, crate::vcs::DispatchSession>,
    /// The dispatch each node had in flight when an adoption cleared it.
    ///
    /// A driver's death does not end the session its dispatch opened: the
    /// branch, the worktree, and whatever the worker had committed are all still
    /// there, and this is the only record of where. Without it the node becomes
    /// ready again, is dispatched into a fresh session, and the previous one is
    /// left unnamed anywhere a manager looks.
    pub abandoned: BTreeMap<String, crate::vcs::DispatchSession>,
    /// Where a human reads the change each published node opened.
    pub change_urls: BTreeMap<String, String>,
    /// Why each dispatch that ended for a reason other than the agent's verdict
    /// ended, in the words its producer classified it with.
    ///
    /// Written only by a settlement that carried one, like
    /// [`branches`](Self::branches) and [`change_urls`](Self::change_urls): most
    /// settlements carry none, and an entry cleared on any other event would turn
    /// a dispatch that died into one whose agent failed its task.
    //
    // llmlint: ignore[invalid_states_unrepresentable] a newtype here would have to hold the
    // producer's own word, and there is no vocabulary to hold it against: the classifications
    // belong to `oneharness`, which is deliberately not a dependency of this crate, so one
    // that layer adds has to arrive without this crate learning it. What could go wrong with
    // an unchecked one — a paragraph, or a line of its own, where a reader looks for a word —
    // is checked on both edges it crosses, by `engine::is_a_classification`: where it is
    // lifted off the dispatch's own detail, and again where this map is folded out of a
    // journal another build wrote and a person can edit.
    pub causes: BTreeMap<String, String>,
    /// The commit each node's branch was left at, as its settlement recorded it.
    ///
    /// Not the same thing as [`landing_commits`](Self::landing_commits): that one
    /// is where a change *reached its base*, and this is what the node's own
    /// branch carries — which for work that never landed is the only commit
    /// anybody can go and read.
    //
    // llmlint: ignore[invalid_states_unrepresentable] a commit is the plain string every
    // identifier in this crate is, for the reason `landing_commits` records. What could go
    // wrong with an unchecked one is checked where it enters, by `vcs::branch_head_in`.
    pub heads: BTreeMap<String, String>,
    /// The commit each node's change reached its base at, as `onevcs` reported
    /// it on the session's own stream.
    ///
    // llmlint: ignore[invalid_states_unrepresentable] a commit is the plain string every
    // identifier in this crate is, for the reason `src/plan.rs` and `src/crossdag.rs`
    // record: it is what the journal payload carries and what the sibling's own reference
    // grammar takes. What could go wrong with an unchecked one — a value that forges a row
    // where it is rendered — is checked where it enters, by `vcs::landing_commit_of`, which
    // is the only thing that writes this map.
    ///
    ///
    /// The one thing that names *where* a node's work landed, and therefore the
    /// only thing a release can be measured against: a baseline is captured at a
    /// landing, so asking whether a release carries this work means naming that
    /// landing. Folded from the relayed `merge-completed` rather than from a
    /// settlement, because the settlement records that a change landed and not
    /// what it landed as.
    pub landing_commits: BTreeMap<String, String>,
    /// Whether each published node's change reached its base branch.
    ///
    /// Written only by a settlement that observed one, like
    /// [`branches`](Self::branches) and [`change_urls`](Self::change_urls): an
    /// unmerged change request outlives the settlement that opened it, so the
    /// fact stands until the node settles again and overwrites its own entry,
    /// which is the only way the answer changes. A landing dropped on any other
    /// event would report an open change as one nobody had anything to say
    /// about, which is precisely when a planner starts deciding there is nothing
    /// left to do.
    pub landings: BTreeMap<String, Landing>,
    /// The declared steps each node's attempt finished.
    ///
    /// What a continuation may skip, and the only record of it: a step is not a
    /// node, so nothing else in the journal says one finished.
    pub completed_steps: BTreeMap<String, Vec<String>>,
    /// When each node was dispatched, in epoch milliseconds.
    pub dispatched_at: BTreeMap<String, u64>,
    /// When each node settled, in epoch milliseconds.
    pub settled_at: BTreeMap<String, u64>,
    /// Human actions attested across the whole run.
    pub attestations: BTreeSet<String>,
    /// The decision points reported as holding dependents back and not yet
    /// reported as released, by the reference that clears each.
    pub decisions_pending: BTreeMap<String, PendingDecision>,
    /// The nodes reported as held and not yet reported as released, carrying the
    /// `reasons` array the record was written with.
    ///
    /// Held as the payload rather than as a parsed reason, for the same reason
    /// every other fold here keeps the producer's own words: a driver reading a
    /// record a later build wrote must not normalise a reason it has no variant
    /// for into one it has. What a fresh driver does with it is seed what it
    /// believes it is already holding, so it neither restates a hold its
    /// predecessor opened nor loses the release of one.
    ///
    /// The typed reading happens where the reasons are *used*, not here:
    /// `engine`'s `HoldReason::of_payload` refuses a payload it cannot read, and
    /// a node whose reasons do not all parse is dropped from what the driver
    /// believes it holds rather than folded in malformed.
    // llmlint: ignore[invalid_states_unrepresentable] a typed enum here cannot
    // reject anything this one accepts: the array is a persisted record a *later*
    // build may have written, so any enum able to round-trip it needs an
    // `Unknown(Value)` variant, which is this type again behind one more parse.
    // The rejecting boundary is `HoldReason::of_payload` at the point of use,
    // named above; making it the fold's boundary instead would silently discard a
    // reason a newer driver wrote, which is the bug this field exists to avoid.
    pub holds: BTreeMap<String, Vec<Value>>,
    /// The completion reasons the planner has journalled.
    pub completion_requests: Vec<String>,
    /// Surfaces sent, and surfaces a planner actually read.
    pub surfaces_queued: u64,
    /// Surfaces consumed through `next`. This is what resets the pacemaker.
    pub surfaces_read: u64,
    /// When the last surface was read, in epoch milliseconds.
    pub last_surface_at: Option<u64>,
    /// The last event of any kind, in epoch milliseconds — the run's own
    /// evidence that something is still writing to it.
    pub last_write_at: Option<u64>,
    /// What `stop` left the run as.
    pub stop: StopState,
    /// Whether the fold met a line it could not read. Strict replay reports
    /// rather than silently folding an incomplete graph.
    pub strict: bool,
    /// Every node this run's nodes named across another run's DAG.
    pub cross_dag_watches: BTreeMap<String, u64>,
    /// Where each resolved upstream had got when this run first resolved it.
    ///
    /// Folded from the journal rather than held in a process, because a watch
    /// outlives the process that captured it: a baseline a fresh driver
    /// re-derived would never see the upstream move.
    pub cross_dag_baselines: BTreeMap<String, u64>,
    /// The `(dependency, consumer)` pairs already reported as moved, so a watch
    /// reports once rather than once per reconcile pass.
    pub cross_dag_reported: BTreeSet<(String, String)>,
    /// How each cross-DAG dependency resolved, for the caller that went and
    /// looked.
    ///
    /// Empty by default, and an absent reference derives as blocked — so a
    /// reader that cannot reach another run's ledger reports a consumer as
    /// waiting rather than inventing an answer about it. `crate::crossdag` is
    /// what fills this in.
    pub cross_dag: BTreeMap<String, NodeStatus>,
    /// The notes still owed to a node's **next dispatch**.
    ///
    /// A note carries exactly one dispatch: it attaches to the node's next one
    /// and is consumed when that dispatch takes it, so a correction is stated
    /// once rather than repeated at every later attempt. A note the running turn
    /// already took never enters this set at all.
    pub pending_context: BTreeMap<String, String>,
    /// Every candidate each node's identity chains stepped past, in the order
    /// the store recorded them.
    ///
    /// The only record in the merged store that says *which* identity refused
    /// and *which side* of the conversation asked it. Without it a node that
    /// failed on an exhausted subscription reads exactly like one that failed
    /// its own gate, and the two sides of a member prefer different identities —
    /// so a fix aimed at the wrong chain changes nothing and the run fails the
    /// same way again.
    pub refusals: BTreeMap<String, Vec<Refusal>>,
    /// Every oneharness invocation each node's members actually **ran**, in the
    /// order the store recorded them.
    ///
    /// The other half of [`refusals`](Self::refusals), and the only thing that
    /// tells a chain which recovered from one which ran out: an advance names
    /// the candidate a chain stepped past, and the invocation published beside
    /// it names the identity that went on to serve that side's turn. Without it
    /// every fall-through reads as fatal, and a reader is sent at a
    /// subscription that never blocked a turn.
    pub served: BTreeMap<String, Vec<Served>>,
    // llmlint: ignore-block[invalid_states_unrepresentable] both sides are node ids, and a
    // node id is the plain `String` every map on this struct is keyed by — `recorded`,
    // `outcomes`, `branches`, and the rest — for the reason `src/error.rs`'s file-level
    // suppression states: a `NodeId` newtype is a public item `docs/contract.md` does not
    // name. Neither side is unchecked: both come off an `edit-committed` the reconciler
    // wrote, which validated the superseded node against the live graph and refused a
    // replacement id that was blank or already taken.
    /// The replacement each superseded node was retried under.
    ///
    /// **The one record that says a failure was answered.** A `retry` takes the
    /// node it supersedes out of the graph in the same edit, so every view built
    /// from the graph loses it — while the store still carries the `node-settled`
    /// that failed it, with nothing beside that record saying it was replaced. A
    /// reader of the stream, the run's own monitor included, met a `failed` node
    /// and proposed retrying work that had already been redone and merged; on one
    /// run eleven entries read `failed` and not one was a node anybody could
    /// retry. This is what every reader of that settlement is qualified by.
    pub superseded: BTreeMap<String, String>,
    // llmlint: ignore-end[invalid_states_unrepresentable]
    /// What each node's dispatch is doing *now*, from the relayed stream.
    ///
    /// The one question no event of this crate's own can answer: a
    /// `node-dispatched` says a dispatch started and nothing after it, so a node
    /// in flight for half an hour reads the same whether it is working or
    /// wedged. The siblings say the rest, and this is where it is read.
    ///
    /// Cumulative per node, because execution is: a node is dispatched, retried,
    /// and requeued within one continuous run, and what the store holds for it
    /// is everything it has recorded. How long the *current* attempt has been
    /// going is [`dispatched_at`](Self::dispatched_at), which a view reports
    /// beside this.
    pub activity: BTreeMap<String, NodeActivity>,
}

/// What the journal recorded about one node: the status it stated, and — for the
/// one status where "what the node is" and "what is running for it" are
/// different questions — when the cancellation that parked it was asked for.
///
/// A `cancel` parks the node at once and *asks* the live turn to commit and end,
/// so `parked` is the only status that can still have a dispatch behind it — and
/// it is the same word for the opposite situation, a node the planner idled with
/// nothing running.
///
/// One value rather than a status beside a map of cancellation times, so every
/// transition writes the whole of it and a wait cannot outlive the park that
/// carried it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Recorded {
    /// The status the journal stated, with nothing left running for the node.
    At(NodeStatus),
    /// A node a `cancel` parked while its dispatch was still running.
    ///
    /// A variant rather than a timestamp beside `At(Parked)`: there is no status
    /// field left to disagree with the wait, so a cancellation in flight cannot
    /// be recorded against a node that is not parked.
    Cancelling {
        /// When the cancellation was asked for, in epoch milliseconds.
        since: u64,
    },
}

impl Recorded {
    /// The status the journal stated. A cancellation in flight is a park: the
    /// flag holding the node out of every later dispatch is already set, and
    /// only a requeue clears it.
    pub fn status(self) -> NodeStatus {
        match self {
            Self::At(status) => status,
            Self::Cancelling { .. } => NodeStatus::Parked,
        }
    }

    /// When the cancellation that parked this node was asked for, while the
    /// dispatch it asked to stop is still out there.
    pub fn cancelling_since(self) -> Option<u64> {
        match self {
            Self::At(_) => None,
            Self::Cancelling { since } => Some(since),
        }
    }
}

/// A decision point a driver reported as holding dependents back and has not
/// reported as released.
///
/// Folded from the journal rather than held in a process, because a decision
/// outlives the driver that reported it: an adoption picks up a run parked on
/// one, and a fresh loop that did not know what its predecessor was holding
/// would clear it silently — the pause reported, the release never.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PendingDecision {
    /// What kind of decision it is, in the vocabulary its raiser used.
    pub kind: String,
    /// The nodes it was reported as holding back.
    pub unblocks: Vec<String>,
}

/// What one node's dispatch has recorded, and what it last said it was doing.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NodeActivity {
    /// The tool the last `turn-activity` named, with its bounded detail.
    ///
    /// `None` until one arrives, and never a stand-in for one: a dispatch that
    /// has recorded something without naming a tool is not a dispatch doing
    /// "nothing", and reporting it as one is the misreading this whole readout
    /// exists to prevent.
    pub doing: Option<String>,
    /// What this node's dispatch has done, once it has done anything.
    ///
    /// Named for progress rather than for events because a heartbeat is an event
    /// and is not one of these — see [`evidences_progress`] — so this is what the
    /// dispatch has done rather than how long it has been alive.
    pub progress: Option<Progress>,
    /// When this node's dispatch last said it was **alive** without having
    /// produced anything, in epoch milliseconds.
    ///
    /// Held apart from [`progress`](Self::progress) because the two answer
    /// opposite questions, and their union answers neither.
    pub last_heartbeat_at: Option<u64>,
}

/// What one node's dispatch has done that this build can date: how many
/// envelopes evidencing progress have arrived since the first one it could place
/// in time, and when the last of those did.
///
/// One value, because they are one fact. Apart, they admit a dispatch that has
/// recorded events at no time and one that has recorded none at a time, and a
/// view rendering either claims an age nothing measured — which is the misreading
/// this whole readout exists to prevent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Progress {
    events: NonZeroU64,
    last_at: u64,
}

impl Progress {
    /// The first envelope evidencing progress, where this build can place it in
    /// time.
    ///
    /// A dispatch whose every envelope so far carries a stamp this build cannot
    /// read has recorded nothing it can age, and is reported as having recorded
    /// nothing rather than as having recorded something a moment ago. Those
    /// arrivals are left out of the count as well, because there is nowhere to
    /// hold them that does not also assert a moment — a count standing alone is
    /// the half of this pair a view renders as an age.
    pub(crate) fn first(at: Option<u64>) -> Option<Self> {
        Some(Self {
            events: NonZeroU64::MIN,
            last_at: at?,
        })
    }

    /// This record with one more envelope in it.
    ///
    /// One whose stamp this build cannot read still counts — it happened — and
    /// leaves the age standing at the last arrival that could be placed, which is
    /// the only thing there is to age it by.
    pub(crate) fn and(self, at: Option<u64>) -> Self {
        Self {
            events: self.events.saturating_add(1),
            last_at: at.unwrap_or(self.last_at),
        }
    }

    /// How many have arrived since the first one this build could place in time.
    pub fn events(self) -> u64 {
        self.events.get()
    }

    /// When the last of those did, in epoch milliseconds.
    pub fn last_at(self) -> u64 {
        self.last_at
    }
}

/// One candidate a node's identity chain stepped past, and how often it was
/// recorded.
///
/// The advance itself is `oneagentgraph`'s **own** payload type, held whole
/// rather than copied field by field: the identity, `oneharness`'s
/// classification of why the candidate could not run, and — for a two-party
/// member — which side of the conversation the chain belonged to are that
/// library's contract, and a second declaration of them here is a second thing
/// to keep true. What this crate adds is what the *envelope* carried around it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refusal {
    /// The advance exactly as `oneagentgraph` published it.
    pub advanced: oneagentgraph::event::FallbackAdvanced,
    /// The member whose chain it was, as the producer labelled the envelope.
    ///
    /// What names the side when the advance's own `role` does not: a
    /// single-sided member has one side and stamps none, and a record that
    /// named neither is one this crate must not invent a side for. That is the
    /// whole failure being fixed — a fix aimed at the wrong side of a
    /// conversation changes nothing.
    pub member: MemberLabel,
    /// How many records carried this same side, identity, reason, and turn.
    ///
    /// Non-zero because a refusal exists only by having been recorded once.
    /// Deliberately not a count of *turns*: one turn's chain can record the same
    /// candidate more than once, so this counts records and a view that said
    /// turns would be making a measurement nothing here made.
    pub records: std::num::NonZeroU64,
}

/// One oneharness invocation a node's member actually **ran**, and the member
/// it ran for.
///
/// The invocation is `oneagentgraph`'s **own** payload type, held whole for the
/// reason [`Refusal`] holds its advance whole: the side, the turn, and the
/// composed identity that reproduces the run are that library's contract, and a
/// second declaration of them here is a second thing to keep true.
///
/// Only a two-party member publishes these — a single-sided one has one chain
/// and attributes nothing per side or per turn — so a node whose records carry
/// none is a node this crate cannot say either way about, never one whose chains
/// ran out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Served {
    /// The invocation exactly as `oneagentgraph` published it.
    pub session: oneagentgraph::event::OneharnessSession,
    /// The member whose invocation it was, as the producer labelled the
    /// envelope — read exactly as [`Refusal::member`] is, so the two pair on
    /// the same fact rather than on two readings of it.
    pub member: MemberLabel,
}

/// The member an envelope named, as far as this build could read it.
///
/// Three answers rather than an [`Option`], because the third is a different
/// fact and reading it as either of the others is a claim nothing supports: a
/// label a producer stamped and this build cannot read is **not** a producer
/// that stamped none, and a view saying "the record does not name a side" about
/// one would be denying a record that does name one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MemberLabel {
    /// The producer stamped one, and it reads.
    Named(String),
    /// The producer stamped none. A single-sided member's envelope is the
    /// ordinary case.
    Unstamped,
    /// The producer stamped something this build cannot read as a member.
    Unreadable,
}

/// Whether a relayed envelope is `oneagentgraph`'s "a chain stepped past a
/// candidate".
///
/// Read **through that library's own enum** rather than compared against a
/// string of this crate's: the kind is the sibling's vocabulary, and a literal
/// here would be a second copy of a contract it owns — one that keeps matching
/// after the producer renames the kind, and silently stops attributing anything.
/// A kind this build has no reading of simply is not one.
fn is_fallback_advanced(kind: &crate::event::EventKind) -> bool {
    serde_json::from_value::<oneagentgraph::event::EventKind>(Value::String(kind.0.clone()))
        .is_ok_and(|known| known == oneagentgraph::event::EventKind::FallbackAdvanced)
}

/// Whether one relayed kind is `oneagentgraph`'s `oneharness-session`.
///
/// Read the same way [`is_fallback_advanced`] is, and for the same reason: the
/// kind is that library's own, so it is recognised by parsing the wire string
/// back into that library's set rather than by a spelling of it kept here.
fn is_oneharness_session(kind: &crate::event::EventKind) -> bool {
    serde_json::from_value::<oneagentgraph::event::EventKind>(Value::String(kind.0.clone()))
        .is_ok_and(|known| known == oneagentgraph::event::EventKind::OneharnessSession)
}

/// The kind `oneagentgraph` reports a bounded tool summary as.
///
/// A wire string rather than one of [`journal::PipelineKind`]'s: it is the
/// sibling's vocabulary, and this crate reads that half of the merged store
/// without closing it.
const TURN_ACTIVITY: &str = "turn-activity";

/// Whether one relayed envelope is evidence that its dispatch **did**
/// something, rather than only that it is still there.
///
/// Every kind but one is: fetching, gating, and publishing are work as much as
/// a turn naming a tool. `member-heartbeat` is the exception by its producer's
/// own definition — "a member is alive but has produced nothing since the last
/// heartbeat" — and arrives every few seconds from any live process regardless
/// of progress, so a clock it advanced could never read a stall.
///
/// The spelling is read off the producing library's enum, so a rename there is
/// a compile error rather than a clock that silently stops reading.
pub(crate) fn evidences_progress(event: &Envelope) -> bool {
    event.kind.0 != oneagentgraph::event::EventKind::MemberHeartbeat.as_str()
}

impl RunState {
    /// Whether a stop has been recorded at all, however it went.
    ///
    /// Not "the run's work has ended": a recorded stop may have reached nothing.
    /// [`stop`](Self::stop) is what says which.
    pub fn stop_recorded(&self) -> bool {
        self.stop != StopState::NotStopped
    }

    /// The frontier an edit is judged against, as far as the *ledger* says.
    ///
    /// Which dispatches are still running is not in it, because the journal
    /// cannot say: a dispatch is a live process, and only the loop driving it
    /// knows. That half is filled in by the reconciler, which is the caller that
    /// has it — see [`Frontier::in_flight`].
    pub fn frontier(&self) -> Frontier {
        Frontier {
            recorded: self.statuses_recorded(),
            attestations: self.attestations.clone(),
            in_flight: BTreeMap::new(),
            // The launch's, and only a caller holding the launch record has it.
            node_validator: None,
        }
    }

    /// The statuses alone, for the two readers that judge a node by where it got
    /// to and have no use for what may still be running for it: an edit, which
    /// asks [`Frontier::in_flight`] that question instead, and the derivation,
    /// which is about the graph rather than about any dispatch.
    fn statuses_recorded(&self) -> BTreeMap<String, NodeStatus> {
        self.recorded
            .iter()
            .map(|(id, recorded)| (id.clone(), recorded.status()))
            .collect()
    }

    /// The session each node still recorded `running` is working in.
    ///
    /// The **sessions**, and so not every dispatch in flight: a node whose
    /// dispatch opened none is not in it, because a direct agent node has no
    /// repository and so has no branch to be anywhere. Nor is a settled node —
    /// a session an earlier attempt finished with is closed, and naming it would
    /// send a reader to a worktree that is gone.
    pub fn sessions_in_flight(&self) -> BTreeMap<String, crate::vcs::DispatchSession> {
        self.recorded
            .iter()
            .filter(|(_, recorded)| recorded.status() == NodeStatus::Running)
            .filter_map(|(id, _)| Some((id.clone(), self.sessions.get(id)?.clone())))
            .collect()
    }

    /// Whether a ready human action is outstanding: one nobody has attested.
    ///
    /// Half of what makes a stalled run *waiting on a person* rather than
    /// abandoned, re-derived from the graph rather than from any round state. The
    /// other half is a blocking surface, which lives in the channel rather than
    /// the graph and so cannot be read here — [`views::decision_outstanding`] is
    /// the whole question, and every verdict about a stalled run asks that one.
    ///
    /// [`views::decision_outstanding`]: crate::views::decision_outstanding
    pub fn awaiting_human_action(&self) -> bool {
        self.statuses()
            .values()
            .any(|status| *status == NodeStatus::Waiting)
    }

    /// Every node's status, with the derived gates recomputed against the graph
    /// as it stands now.
    pub fn statuses(&self) -> BTreeMap<String, NodeStatus> {
        self.statuses_with(&|dependency| self.cross_dag.get(dependency).copied())
    }

    /// Every node's status, resolving cross-DAG references through `upstream`.
    pub fn statuses_with(
        &self,
        upstream: &dyn Fn(&str) -> Option<NodeStatus>,
    ) -> BTreeMap<String, NodeStatus> {
        crate::loopstats::statuses_derived();
        crate::graph::derive(&self.graph, &self.statuses_recorded(), upstream)
    }
}

/// Fold a run's whole journal.
pub fn fold(events: &[Envelope]) -> RunState {
    let mut state = RunState {
        strict: true,
        ..RunState::default()
    };
    for event in events {
        fold_one(&mut state, event);
    }
    state
}

fn fold_one(state: &mut RunState, event: &Envelope) {
    state.last_write_at = Some(
        millis_of(&event.ts)
            .unwrap_or(0)
            .max(state.last_write_at.unwrap_or(0)),
    );
    if event.source != Source::Pipeline {
        // A relayed envelope does not decide this crate's graph state — a
        // sibling library does not settle a node — but it is the only evidence
        // of what the node it belongs to is doing while it runs, the only
        // evidence of which identity refused when it stops running, and the only
        // evidence of where it is doing it.
        fold_activity(state, event);
        fold_refusal(state, event);
        fold_invocation(state, event);
        fold_session(state, event);
        fold_landing_commit(state, event);
        return;
    }
    let payload = &event.payload;
    match journal::PipelineKind::from_wire(&event.kind) {
        Some(journal::PipelineKind::RunStarted) => {
            if let Some(plan) = plan_of(payload) {
                state.graph = Graph::from_plan(&plan);
                state.plan = Some(plan);
            }
        }
        Some(journal::PipelineKind::ConcurrentAcknowledged) => {}
        // A node becoming ready changes nothing about the state: it is derived
        // from the graph and the recorded settlements, and this record is how a
        // reader sees the moment it happened.
        Some(journal::PipelineKind::NodeReady) => {}
        Some(journal::PipelineKind::NodeDispatched) => {
            if let Some(node) = &event.labels.node {
                state
                    .recorded
                    .insert(node.clone(), Recorded::At(NodeStatus::Running));
                if let Some(ts) = millis_of(&event.ts) {
                    state.dispatched_at.insert(node.clone(), ts);
                }
                // A dispatch has not opened its session yet, and the one the
                // attempt before it worked in is finished with. Left standing,
                // it would name a re-dispatched node's work on the branch its
                // *previous* attempt used.
                state.sessions.remove(node);
                // The note rode this dispatch, so it is spent. Carrying it into
                // a later one would repeat a correction the worker has already
                // been given.
                state.pending_context.remove(node);
                if let Some(dispatched) = state.graph.get_mut(node) {
                    dispatched.context = None;
                }
            }
        }
        Some(journal::PipelineKind::NodeSettled) => {
            let Some(node) = &event.labels.node else {
                return;
            };
            let status = payload
                .get("status")
                .and_then(Value::as_str)
                .and_then(NodeStatus::parse);
            if let Some(status) = status {
                // The whole record, so the dispatch a cancellation was waiting
                // on is not still awaited by a node that has settled.
                state.recorded.insert(node.clone(), Recorded::At(status));
            }
            if let Some(outcome) = payload.get("outcome").and_then(Value::as_str) {
                state.outcomes.insert(node.clone(), outcome.to_string());
            }
            // What the dispatch left behind, which nothing else records: a
            // later continuation has no other way to find the branch the work
            // is on.
            if let Some(branch) = payload.get("branch").and_then(Value::as_str) {
                state.branches.insert(node.clone(), branch.to_string());
            }
            if let Some(steps) = payload.get("completed_steps").and_then(Value::as_array) {
                state.completed_steps.insert(
                    node.clone(),
                    steps
                        .iter()
                        .filter_map(Value::as_str)
                        .map(str::to_string)
                        .collect(),
                );
            }
            if let Some(url) = payload.get("change_url").and_then(Value::as_str) {
                state.change_urls.insert(node.clone(), url.to_string());
            }
            // The classification the dispatch died under, and the commit its
            // branch was left at. Both are the settlement's own — nothing else in
            // the journal says either — so a fold that dropped them would leave a
            // reader of the result with the word and none of its evidence.
            //
            // Checked here as well as where each was produced, because *this* is
            // the boundary they cross: a journal is a file on disk that another
            // build wrote and a person can edit, and both of these are rendered
            // onto a line and written back into the run's own result document. A
            // value that is not one is folded as none at all rather than carried,
            // which reads as the settlement saying nothing — what a record written
            // before either field existed already reads as.
            //
            // llmlint: ignore-block[changed_behavior_has_e2e] neither refusal has an
            // invocation a user can type behind it: this crate's own writers are the only
            // producers of these two fields and both check what they write, so reaching a
            // refusal means a journal edited by hand, which would prove the fixture. Held
            // by this module's fold test, exactly as the unreadable landing below it is.
            if let Some(cause) = payload
                .get(journal::SETTLED_CAUSE)
                .and_then(Value::as_str)
                .filter(|cause| crate::engine::is_a_classification(cause))
            {
                state.causes.insert(node.clone(), cause.to_string());
            }
            if let Some(head) = payload
                .get(journal::SETTLED_HEAD)
                .and_then(Value::as_str)
                .and_then(crate::vcs::usable)
            {
                state.heads.insert(node.clone(), head);
            } // llmlint: ignore-end[changed_behavior_has_e2e]
              // Only a word this build can interpret: an unreadable landing leaves
              // the node with none, which reads as nothing observed.
              //
              // llmlint: ignore-block[changed_behavior_has_e2e] no invocation a user can type
              // reaches this half: an unreadable value needs a journal a *newer build* wrote.
              // Held by this module's fold test instead; what a user can reach is held in
              // `tests/e2e/lifecycle.rs`.
            if let Some(landing) = payload
                .get(journal::SETTLED_LANDING)
                .and_then(Value::as_str)
                .and_then(Landing::parse)
            {
                state.landings.insert(node.clone(), landing);
            } // llmlint: ignore-end[changed_behavior_has_e2e]
            if let Some(ts) = millis_of(&event.ts) {
                state.settled_at.insert(node.clone(), ts);
            }
            if let Some(status) = status {
                pin_preserved_branch(state, node, status);
            }
        }
        Some(journal::PipelineKind::EditCommitted) => {
            let operations = payload
                .get("operations")
                .and_then(|value| serde_json::from_value::<Vec<Operation>>(value.clone()).ok());
            let Some(operations) = operations else {
                // An `edit-committed` whose operations this build cannot fold
                // might have been an authoritative graph mutation.
                state.strict = false;
                return;
            };
            for operation in &operations {
                edits::apply(&mut state.graph, operation);
                match operation {
                    Operation::HumanAttested { node } => {
                        state.attestations.insert(node.clone());
                        state
                            .recorded
                            .insert(node.clone(), Recorded::At(NodeStatus::Done));
                    }
                    // A completion request is recorded as its own event by
                    // whichever side took it, so folding it here too would
                    // count one request twice.
                    Operation::CompletionRequested { .. } => {}
                    Operation::RetryRequested {
                        node, replacement, ..
                    } => {
                        // What the supersession did to the node it replaced. The
                        // node itself leaves the graph with the same edit, so
                        // this is what the run's record says became of it.
                        state
                            .recorded
                            .insert(node.clone(), Recorded::At(NodeStatus::Cancelled));
                        // And which node carries its work now, which is the half
                        // no status word can say: `cancelled` is also what a
                        // `drop` leaves, and the two take opposite actions.
                        state.superseded.insert(node.clone(), replacement.clone());
                    }
                    Operation::NodeParked { node } => {
                        // What the node was *before* the park decides which park
                        // this is: a cancel of a running node asks a dispatch to
                        // stop and leaves it running, and one of a node that
                        // never started stops nothing. Both become `parked`, and
                        // the difference rides the same single write.
                        let was = state.recorded.get(node).map(|recorded| recorded.status());
                        let parked = match (was, millis_of(&event.ts)) {
                            (Some(NodeStatus::Running), Some(since)) => {
                                Recorded::Cancelling { since }
                            }
                            _ => Recorded::At(NodeStatus::Parked),
                        };
                        state.recorded.insert(node.clone(), parked);
                    }
                    Operation::NodeRequeued { node, .. } => {
                        state.recorded.remove(node);
                    }
                    // Only a note that is still owed to a dispatch. One the
                    // running turn already took has been read, and holding it
                    // for the next dispatch would re-state a correction the
                    // worker has acted on.
                    Operation::ContextAdded {
                        node,
                        note,
                        delivery: edits::Delivery::Deferred,
                    } => {
                        state.pending_context.insert(node.clone(), note.clone());
                    }
                    Operation::ContextAdded { .. } => {}
                    _ => {}
                }
            }
        }
        // A note delivered onto the node's *next* dispatch is owed to it, exactly
        // as a deferred planner note is, and this record is the only thing that
        // says so: the note itself is reconstructed from the versions the payload
        // names, by the same function that composed the one that was sent, so a
        // replayed note is the note the node was told.
        Some(journal::PipelineKind::ReleaseAdopted) => {
            let Some(node) = &event.labels.node else {
                return;
            };
            if payload.get("delivery").and_then(Value::as_str) != Some("next") {
                return;
            }
            let Some(versions) = payload.get("versions") else {
                return;
            };
            let released = crate::release::Released::of_payload(versions);
            // A record naming no release this build can read cannot say what the
            // node was told, and a note listing nothing is worse than none: it
            // would tell a worker its releases had arrived and name not one.
            if released.is_empty() {
                return;
            }
            let note = crate::release::arrival_note(&released);
            state.pending_context.insert(node.clone(), note.clone());
            if let Some(waiting) = state.graph.get_mut(node) {
                waiting.context = Some(note);
            }
            // For a node this run held back, the note also ends the hold:
            // clearing the record returns it to the frontier on the branch its own
            // settlement pinned it to, so the dispatch that takes the note
            // continues the published branch rather than cutting a fresh one
            // beside a draft nothing would then lift. Only that status — clearing
            // any other would dispatch finished work all over again.
            if state.recorded.get(node).copied().map(Recorded::status)
                == Some(NodeStatus::CompleteDraft)
            {
                state.recorded.remove(node);
            }
        }
        // Reports, both: what a run is waiting on and what has arrived are
        // derived afresh by whatever is driving it, so neither changes the graph.
        Some(journal::PipelineKind::ReleaseWait | journal::PipelineKind::ReleaseArrived) => {}
        Some(journal::PipelineKind::HumanAttested) => {
            if let Some(reference) = payload.get("ref").and_then(Value::as_str) {
                state.attestations.insert(reference.to_string());
                state
                    .recorded
                    .insert(reference.to_string(), Recorded::At(NodeStatus::Done));
            }
        }
        Some(journal::PipelineKind::CompletionRequested) => {
            if let Some(reason) = payload.get("reason").and_then(Value::as_str) {
                state.completion_requests.push(reason.to_string());
            }
        }
        // A fresh driver means no dispatch the previous one started survives:
        // the process that was running them is gone, and this crate's dispatches
        // are threads of that process. A node still recorded `running` would
        // otherwise be a node nothing is running and nothing will ever settle —
        // never ready, so never dispatched again, and never terminal, so the
        // loop that adopted it would spin on it forever. The record stands as
        // history; what it means for the *frontier* ends here.
        //
        // The *work* is a different question from the process, and it does
        // survive: what the dispatch had committed is on the branch its session
        // opened, and that session is still open. So it is named before the
        // record clearing it, and the node is pinned there — see
        // [`abandon_the_dispatch_in_flight`].
        Some(journal::PipelineKind::DriverAdopted) => {
            abandon_the_dispatch_in_flight(state);
            state
                .recorded
                .retain(|_, recorded| recorded.status() != NodeStatus::Running);
            // A cancellation in flight is a wait on a *process*, and the same
            // proof ends it: the dispatch it asked to stop went with the driver
            // that started it. The park itself stands — only a requeue clears
            // that — and a run reporting a stop that nothing is still
            // converging on would be a supervisor waiting for good.
            for recorded in state.recorded.values_mut() {
                if matches!(recorded, Recorded::Cancelling { .. }) {
                    *recorded = Recorded::At(NodeStatus::Parked);
                }
            }
        }
        // llmlint: ignore-block[boundary_inputs_validated] the journal is this crate's own
        // record rather than external input, on the ruling `journal.rs` already carries for
        // the same reader: a record written by a build this one does not know is *skipped and
        // reported*, never refused. Validating a reason's shape here would refuse the whole
        // fold over a reason a newer driver wrote, which is the case this field exists to
        // survive; the rejecting boundary is `engine`'s `HoldReason::of_payload`, which drops
        // a node whose reasons it cannot read rather than admitting a malformed one.
        Some(journal::PipelineKind::NodeHeld) => {
            if let (Some(node), Some(reasons)) = (
                event.labels.node.clone(),
                payload.get("reasons").and_then(Value::as_array),
            ) {
                state.holds.insert(node, reasons.clone());
            }
        } // llmlint: ignore-end[boundary_inputs_validated]
        Some(journal::PipelineKind::NodeUnheld) => {
            if let Some(node) = event.labels.node.as_ref() {
                state.holds.remove(node);
            }
        }
        Some(journal::PipelineKind::DecisionPending) => {
            if let Some(reference) = payload.get("reference").and_then(Value::as_str) {
                state.decisions_pending.insert(
                    reference.to_string(),
                    PendingDecision {
                        kind: payload
                            .get("kind")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                        unblocks: payload
                            .get("unblocks")
                            .and_then(Value::as_array)
                            .map(|held| {
                                held.iter()
                                    .filter_map(Value::as_str)
                                    .map(str::to_string)
                                    .collect()
                            })
                            .unwrap_or_default(),
                    },
                );
            }
        }
        Some(journal::PipelineKind::DecisionCleared) => {
            if let Some(reference) = payload.get("reference").and_then(Value::as_str) {
                state.decisions_pending.remove(reference);
            }
        }
        Some(journal::PipelineKind::PlannerSurfaceQueued) => state.surfaces_queued += 1,
        Some(journal::PipelineKind::PlannerSurfaced) => {
            state.surfaces_read += 1;
            state.last_surface_at = millis_of(&event.ts);
        }
        Some(journal::PipelineKind::RunStopped) => {
            state.stop = match journal::StopTeardown::of(payload) {
                journal::StopTeardown::Signalled => StopState::WorkersSignalled,
                // A stop that found nothing to aim at established nothing about
                // this run's workers either. It very likely means they had
                // already ended — but "ended when the run was stopped" is a
                // claim about a signal nobody sent, and a worker orphaned by a
                // dead driver is exactly the case that would make it false.
                journal::StopTeardown::NothingToStop
                | journal::StopTeardown::IdentityDeclined
                | journal::StopTeardown::NotAttempted
                | journal::StopTeardown::PartlySignalled
                // llmlint: ignore-block[changed_behavior_has_e2e] this pattern is forced by
                // exhaustiveness over a value the journal newly carries, and it renders
                // nothing new: `refused` lands on `WorkersUndetermined`, the same state the
                // four variants beside it map to, and three of those — `nothing-to-stop`,
                // `not-attempted`, and `elsewhere` — are driven through this projection end to
                // end in `tests/e2e/driver.rs`. A journey could not reach it in any case, for the
                // reason the `stop` arm that writes the value carries the same directive: a
                // `run-stopped` payload only says `refused` when every process of a run's tree
                // refused this user's signal, and a process this user may not signal is not a
                // thing for a suite to go and make. What the value is established from is
                // proved where it can be, at `sys::established`'s
                // `a_teardown_refused_by_everything_it_aimed_at_reports_no_signal_at_all` and
                // `a_stop_that_could_signal_nothing_it_aimed_at_says_so`.
                | journal::StopTeardown::Refused
                // llmlint: ignore-end[changed_behavior_has_e2e]
                | journal::StopTeardown::Elsewhere => StopState::WorkersUndetermined,
            };
        }
        Some(journal::PipelineKind::CrossDagSatisfied) => {
            if let (Some(dependency), Some(last)) = (
                payload.get("dependency").and_then(Value::as_str),
                payload.get("last_seq").and_then(Value::as_u64),
            ) {
                // The *first* baseline stands. A later one would move the mark
                // the watch measures from, which is the one thing that would
                // make a moved upstream unreportable.
                state
                    .cross_dag_baselines
                    .entry(dependency.to_string())
                    .or_insert(last);
            }
        }
        Some(journal::PipelineKind::UpstreamModified) => {
            if let Some(dependency) = payload.get("dependency").and_then(Value::as_str) {
                *state
                    .cross_dag_watches
                    .entry(dependency.to_string())
                    .or_insert(0) += 1;
                if let Some(consumer) = event.labels.node.as_deref() {
                    state
                        .cross_dag_reported
                        .insert((dependency.to_string(), consumer.to_string()));
                }
            }
        }
        _ => {}
    }
}

/// Record where each dispatch an adoption is clearing was working, and pin its
/// node there.
///
/// Only the nodes the arm above is about to drop: a node not recorded `running`
/// has no dispatch in flight, and an adoption of a run that had none changes
/// nothing at all.
///
/// **Naming it is the floor, and pinning it is what recovers the work.** The
/// session `onevcs` opened for the dead driver's dispatch is still open, and
/// that library takes an open session up again when the branch it holds is the
/// branch the next dispatch pins — so a node pinned here continues on the branch
/// its previous dispatch committed to instead of cutting a second one beside it
/// and leaving the first unreferenced.
///
/// There is one branch to pin rather than two to choose between, which is why
/// nothing here weighs a `branch` the *planner* wrote against the session's: a
/// pin is honoured or refused, so a session that opened at all opened on the
/// branch the node named, and a node that named none has the branch `onevcs`
/// gave it. The session is asked either way — it is the one that knows.
fn abandon_the_dispatch_in_flight(state: &mut RunState) {
    // The same answer the adopting driver records in the journal, so what a
    // reader folds and what the run's own `driver-adopted` says are one
    // derivation rather than two that can come to disagree. A dispatch that had
    // opened no session — a direct agent node, or a lifecycle node the driver
    // died ahead of — is not in it: there is no branch to name and none to pin.
    for (id, session) in state.sessions_in_flight() {
        state.sessions.remove(&id);
        if let Some(node) = state.graph.get_mut(&id) {
            node.branch = Some(session.branch().as_str().to_owned());
        }
        state.abandoned.insert(id, session);
    }
}

/// Fold one relayed `session-opened` into where the node's dispatch is working.
///
/// Only `onevcs` opens a session, so only an envelope of that library's own
/// source and kind is one; both writers of it land here — the sibling's own
/// record and the copy this crate writes beside it — and the last one wins,
/// because they describe the same session.
///
/// Which node it belongs to is the *enricher's* stamp, and an envelope naming
/// none belongs to no dispatch. That is all the label decides here: it is a
/// **key**, and whether its node still has a dispatch running is asked at read
/// time, by [`RunState::sessions_in_flight`].
///
/// What the record has to be for this crate to act on it is
/// [`DispatchSession::read_from`](crate::vcs::DispatchSession::read_from)'s
/// question, which is where it is answered.
/// Record where one node's change reached its base.
///
/// Off the sibling's own `merge-completed`, which is the one record that carries
/// the landing commit: `landing_of` reads it out of git on the direct path and
/// out of the host's answer on the change-request one, and neither reaches this
/// crate's settlement. A payload without a usable `sha` records nothing, which
/// leaves the run naming a dependency's branch instead of its landing.
fn fold_landing_commit(state: &mut RunState, event: &Envelope) {
    let Some(node) = event.labels.node.as_deref() else {
        return;
    };
    let Some(commit) = crate::vcs::landing_commit_of(event) else {
        return;
    };
    state.landing_commits.insert(node.to_string(), commit);
}

fn fold_session(state: &mut RunState, event: &Envelope) {
    if event.source != Source::Vcs || !crate::vcs::is_session_opened(&event.kind) {
        return;
    }
    let Some(node) = event.labels.node.as_deref() else {
        return;
    };
    let Some(session) = crate::vcs::DispatchSession::read_from(event) else {
        return;
    };
    state.sessions.insert(node.to_string(), session);
}

/// Statuses whose work is still on the branch the attempt left behind.
///
/// A node that settled one of these ran, committed, and stopped — so the branch
/// holds work, and anything that runs the node again has to continue it rather
/// than cut a fresh one beside it. `done` never reaches here, and `waiting` and
/// `skipped` never dispatched, so there is nothing to preserve.
fn preserves_its_branch(status: NodeStatus) -> bool {
    matches!(
        status,
        NodeStatus::Failed
            | NodeStatus::Cancelled
            | NodeStatus::Parked
            // A draft-complete node is the one of these that is *coming back*
            // rather than waiting to be sent back: the release it awaits lifts
            // the draft and puts a worker on the branch again to move the pin.
            // Unpinned, that worker would cut a fresh branch and republish it
            // beside the draft change request nobody would then ever lift.
            | NodeStatus::CompleteDraft
    )
}

/// Pin a settled node to the branch its attempt left behind.
///
/// Without this a `requeue` cuts a fresh branch beside committed work nothing
/// points at any more: the publication that failed is retried against an empty
/// tree, and the branch that holds the work is left for a person to find. It is
/// folded rather than held in a process, because the graph a later driver reads
/// is this fold and nothing else.
///
/// A `branch` the *planner* wrote wins outright — naming one is a decision
/// somebody made after reading the result — and the `resume` follows it rather
/// than pointing somewhere else, which is the same agreement `retry` refuses to
/// break.
fn pin_preserved_branch(state: &mut RunState, id: &str, status: NodeStatus) {
    if !preserves_its_branch(status) {
        return;
    }
    let Some(preserved) = state.branches.get(id).cloned() else {
        return;
    };
    let completed = state.completed_steps.get(id).cloned().unwrap_or_default();
    let Some(node) = state.graph.get_mut(id) else {
        return;
    };
    let branch = node.branch.clone().unwrap_or(preserved);
    node.resume = Some(crate::plan::Resume {
        // The checkpoint is the sibling's to name; this crate records the branch
        // it was told about and nothing it was not.
        checkpoint: node.resume.as_ref().and_then(|r| r.checkpoint.clone()),
        branch: branch.clone(),
        // What the attempt actually finished, so a continuation re-runs only
        // what is left.
        completed_steps: completed,
    });
    node.branch = Some(branch);
}

/// Fold one relayed envelope into the node's live activity.
///
/// Every envelope [`evidences_progress`] admits counts, and only a
/// `turn-activity` names a tool, because that is the only kind carrying one. A
/// heartbeat is recorded as the separate fact it is: the node still reads as
/// one something is driving, without its arrival reading as work.
fn fold_activity(state: &mut RunState, event: &Envelope) {
    let Some(node) = event.labels.node.as_deref() else {
        return;
    };
    let activity = state.activity.entry(node.to_string()).or_default();
    let at = millis_of(&event.ts);
    if !evidences_progress(event) {
        activity.last_heartbeat_at = at.or(activity.last_heartbeat_at);
        return;
    }
    activity.progress = match activity.progress {
        Some(progress) => Some(progress.and(at)),
        None => Progress::first(at),
    };
    if event.kind.0 != TURN_ACTIVITY {
        return;
    }
    let text = |key: &str| {
        event
            .payload
            .get(key)
            .and_then(Value::as_str)
            .unwrap_or_default()
    };
    // The producer's own two fields, joined the way its own renderer joins
    // them. An activity naming neither leaves the last one that did standing
    // rather than blanking the line: the dispatch is still doing what it said.
    let summary = [text("name"), text("detail")]
        .into_iter()
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    if !summary.is_empty() {
        activity.doing = Some(summary);
    }
}

/// Record one candidate a node's identity chain stepped past.
///
/// Only `oneagentgraph` publishes these: a `fallback-advanced` from any other
/// source is a kind this crate has no reading of, and attributing a provider
/// refusal to whatever wrote it would be the invented attribution this exists to
/// replace.
fn fold_refusal(state: &mut RunState, event: &Envelope) {
    // llmlint: ignore-block[changed_behavior_has_e2e] the three guards here are about
    // envelopes no producer in this stack writes: only `oneagentgraph` publishes this kind,
    // it labels a dispatch's envelopes with the node, and the payload is its own declared
    // type. Reaching any of them needs a store a *newer* build — or something wearing a
    // producer's clothes — wrote. Held by this module's and `src/views.rs`'s own tests;
    // what a user can reach is driven in `tests/e2e/views.rs`.
    if event.source != Source::Agentgraph || !is_fallback_advanced(&event.kind) {
        return;
    }
    let Some(node) = event.labels.node.as_deref() else {
        return;
    };
    // Read into `oneagentgraph`'s **own** declaration of an advance rather than
    // by field name, exactly as that library reads `oneharness`'s: the shape
    // this crate expects is then the shape the producer publishes, and a payload
    // that is not one is a record this build has no reading of. Dropping it is
    // the right direction — an attribution assembled out of whatever fields
    // happened to be present is the invented attribution this exists to replace.
    let Ok(advanced) = serde_json::from_value::<oneagentgraph::event::FallbackAdvanced>(
        Value::Object(event.payload.clone()),
    ) else {
        return;
    };
    // llmlint: ignore-end[changed_behavior_has_e2e]
    let refusal = Refusal {
        advanced,
        member: member_label(event),
        records: std::num::NonZeroU64::MIN,
    };
    let recorded = state.refusals.entry(node.to_string()).or_default();
    // The turn **is** part of what makes two records the same, because it is
    // what pairs an advance with the invocation that went on to run that side's
    // turn: two turns of one chain can end differently — one recovered, one out
    // of candidates — and a record that had collapsed them could only be
    // rendered as one of the two. Collapsing what a *reader* sees is a
    // rendering, and `src/views.rs` does it there, after each record's ending is
    // known.
    if let Some(same) = recorded.iter_mut().find(|seen| {
        seen.advanced.identity == refusal.advanced.identity
            && seen.advanced.role == refusal.advanced.role
            && seen.advanced.reason == refusal.advanced.reason
            && seen.advanced.turn == refusal.advanced.turn
            && seen.member == refusal.member
    }) {
        same.records = same.records.saturating_add(1);
        return;
    }
    recorded.push(refusal);
}

/// Record one oneharness invocation a node's member actually ran.
///
/// The mirror of [`fold_refusal`], and read under the same rules: only
/// `oneagentgraph` publishes this kind, the envelope has to name the node it
/// belongs to, and the payload is that library's own declared type rather than
/// a set of field names read off whatever arrived. An invocation assembled out
/// of whatever fields happened to be present would name an identity as having
/// served a turn on no better evidence than that something used the word.
fn fold_invocation(state: &mut RunState, event: &Envelope) {
    // llmlint: ignore-block[changed_behavior_has_e2e] the three guards are the three
    // `fold_refusal` carries, about envelopes no producer in this stack writes: reaching
    // any of them needs a store a *newer* build — or something wearing a producer's
    // clothes — wrote. Held by this module's own tests; what a user can reach is driven
    // in `tests/e2e/views.rs`.
    if event.source != Source::Agentgraph || !is_oneharness_session(&event.kind) {
        return;
    }
    let Some(node) = event.labels.node.as_deref() else {
        return;
    };
    let Ok(session) = serde_json::from_value::<oneagentgraph::event::OneharnessSession>(
        Value::Object(event.payload.clone()),
    ) else {
        return;
    };
    // llmlint: ignore-end[changed_behavior_has_e2e]
    let served = Served {
        session,
        member: member_label(event),
    };
    // Appended rather than keyed: the producer publishes one invocation per
    // side per turn, and the first record of a side and a turn is the one a
    // reader pairs an advance on that turn with.
    state
        .served
        .entry(node.to_string())
        .or_default()
        .push(served);
}

/// The member an envelope's labels named, as far as this build can read it.
///
/// The label arrives in `extra`, because this crate's own envelope does not
/// declare `member` — so it is checked here rather than by a schema. A value
/// that is not a member name is kept apart from a producer that stamped none:
/// they are different facts about the record.
fn member_label(event: &Envelope) -> MemberLabel {
    match event.labels.extra.get("member") {
        None => MemberLabel::Unstamped,
        Some(Value::String(member)) => MemberLabel::Named(member.clone()),
        Some(_) => MemberLabel::Unreadable,
    }
}

fn plan_of(payload: &serde_json::Map<String, Value>) -> Option<Plan> {
    payload
        .get("plan")
        .and_then(|value| serde_json::from_value::<Plan>(value.clone()).ok())
}

/// Parse an envelope timestamp back to epoch milliseconds.
///
/// The envelope fixes one format, so this reads exactly that one: anything else
/// is `None` rather than a guess, and a caller treats an untimed event as
/// carrying no timing evidence.
pub fn millis_of(ts: &str) -> Option<u64> {
    let bytes = ts.as_bytes();
    // `YYYY-MM-DDThh:mm:ss.sssZ` exactly: every separator in its own place, and
    // every other position a digit. Without the separator check a string that
    // merely *starts* like a timestamp parses, and `str::parse` would take a
    // signed field like `+12` as well — either way a stranger's malformed clock
    // becomes this run's timing evidence.
    if bytes.len() != 24 {
        return None;
    }
    for (at, separator) in [
        (4, b'-'),
        (7, b'-'),
        (10, b'T'),
        (13, b':'),
        (16, b':'),
        (19, b'.'),
        (23, b'Z'),
    ] {
        if bytes[at] != separator {
            return None;
        }
    }
    let field = |from: usize, to: usize| -> Option<i64> {
        let text = ts.get(from..to)?;
        if !text.bytes().all(|byte| byte.is_ascii_digit()) {
            return None;
        }
        text.parse().ok()
    };
    let (year, month, day) = (field(0, 4)?, field(5, 7)?, field(8, 10)?);
    let (hour, minute, second) = (field(11, 13)?, field(14, 16)?, field(17, 19)?);
    // Three digits, so the millisecond field cannot leave its own range.
    let ms = field(20, 23)?;
    if !(1..=12).contains(&month)
        || !(1..=days_in_month(year, month)).contains(&day)
        || hour > 23
        || minute > 59
        // 60 is a leap second, which is a time a sibling may legitimately render.
        || second > 60
    {
        return None;
    }
    let days = days_from_civil(year, month, day);
    let total = days * 86_400 + hour * 3_600 + minute * 60 + second;
    u64::try_from(total.checked_mul(1_000)?.checked_add(ms)?).ok()
}

/// How many days that month of that year has.
///
/// A blanket 1..=31 would accept 31 February, and `days_from_civil` would
/// silently normalise it into early March — a timestamp that never existed,
/// carried forward as this run's timing evidence.
fn days_in_month(year: i64, month: i64) -> i64 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if year % 4 == 0 && (year % 100 != 0 || year % 400 == 0) => 29,
        2 => 28,
        _ => 0,
    }
}

/// Howard Hinnant's `days_from_civil`, the inverse of the renderer's.
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let y = if month <= 2 { year - 1 } else { year };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = if month > 2 { month - 3 } else { month + 9 };
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::{Labels, ENVELOPE_VERSION};
    use crate::journal::{labels, payload};
    use crate::plan::{Node, PLAN_SCHEMA_VERSION};
    use serde_json::json;

    fn agent(id: &str, deps: &[&str]) -> Node {
        Node {
            id: id.into(),
            persona: Some("engineer".into()),
            task: Some("## What\ndo it".into()),
            deps: deps.iter().map(|d| (*d).to_string()).collect(),
            ..Node::default()
        }
    }

    fn plan_of_nodes(nodes: Vec<Node>) -> Plan {
        Plan {
            schema_version: PLAN_SCHEMA_VERSION,
            goal: None,
            name: Some("demo".into()),
            concurrency: 4,
            tasks: nodes,
        }
    }

    fn pipeline(
        kind: journal::PipelineKind,
        seq: u64,
        node: Option<&str>,
        fields: &[(&str, Value)],
    ) -> Envelope {
        Envelope {
            v: ENVELOPE_VERSION,
            ts: crate::sys::rfc3339_from_millis(1_786_000_000_000 + seq * 1_000),
            stream: "s".into(),
            seq,
            source: Source::Pipeline,
            kind: kind.into(),
            phase: None,
            labels: Labels {
                node: node.map(str::to_string),
                ..labels("demo", None)
            },
            payload: payload(fields),
            artifacts: Vec::new(),
        }
    }

    #[test]
    fn a_timestamp_round_trips_through_the_envelope_format() {
        for millis in [0u64, 1_786_296_585_678, 1_709_164_800_000] {
            let rendered = crate::sys::rfc3339_from_millis(millis);
            assert_eq!(millis_of(&rendered), Some(millis), "{rendered}");
        }
        assert_eq!(millis_of("nope"), None);
        assert_eq!(millis_of("2026-08-08T13:29:45.678+00:00"), None);
        assert_eq!(millis_of("2026-13-08T13:29:45.678Z"), None);
        assert_eq!(millis_of("2026-08-00T13:29:45.678Z"), None);
        assert_eq!(millis_of("20x6-08-08T13:29:45.678Z"), None);
    }

    /// A timestamp is a *stranger's* clock — the two sibling libraries render
    /// it — and it is what the whole telemetry timeline is computed from. One
    /// that reads as a plausible number without being a time would put a run's
    /// wall clock somewhere in the wrong century, silently.
    #[test]
    fn a_timestamp_shaped_string_that_is_not_a_time_carries_no_timing_evidence() {
        // Right length, wrong separators: a stranger's format that merely
        // starts like this one.
        assert_eq!(millis_of("2026-08-08 13:29:45.678Z"), None);
        assert_eq!(millis_of("2026/08/08T13:29:45.678Z"), None);
        assert_eq!(millis_of("2026-08-08T13-29-45.678Z"), None);
        assert_eq!(millis_of("2026-08-08T13:29:45,678Z"), None);
        // Signed fields, which an integer parse would otherwise take.
        assert_eq!(millis_of("2026-08-08T+3:29:45.678Z"), None);
        assert_eq!(millis_of("+026-08-08T13:29:45.678Z"), None);
        // Out of range, digit by digit.
        assert_eq!(millis_of("2026-08-08T24:29:45.678Z"), None);
        assert_eq!(millis_of("2026-08-08T13:60:45.678Z"), None);
        assert_eq!(millis_of("2026-08-08T13:29:61.678Z"), None);
        // A leap second is a real time, and is read as one.
        assert!(millis_of("2026-08-08T23:59:60.000Z").is_some());
        // A day its month does not have would otherwise normalise into the next
        // month — a timestamp that never existed, read as timing evidence.
        assert_eq!(millis_of("2026-02-31T00:00:00.000Z"), None);
        assert_eq!(millis_of("2026-04-31T00:00:00.000Z"), None);
        assert_eq!(
            millis_of("2026-02-29T00:00:00.000Z"),
            None,
            "2026 is not a leap year"
        );
        assert!(millis_of("2024-02-29T00:00:00.000Z").is_some(), "2024 is");
        assert_eq!(millis_of("2100-02-29T00:00:00.000Z"), None, "2100 is not");
        assert!(millis_of("2000-02-29T00:00:00.000Z").is_some(), "2000 is");
    }

    /// A dispatch death's own two fields are folded where they are usable and
    /// dropped where they are not.
    ///
    /// The journal is a file on disk that another build wrote and a person can
    /// edit, and both of these are rendered onto an operator's line and written
    /// back into the run's result document. A classification that is a paragraph,
    /// or a commit carrying a newline, forges a row wherever it lands — so what
    /// this build cannot use it folds as nothing said, which is what a record
    /// written before either field existed already reads as.
    #[test]
    fn a_dispatch_deaths_cause_and_commit_are_folded_only_where_they_are_usable() {
        let plan = plan_of_nodes(vec![agent("good", &[]), agent("forged", &[])]);
        let settled = |seq: u64, node: &str, cause: Value, head: Value| {
            pipeline(
                journal::PipelineKind::NodeSettled,
                seq,
                Some(node),
                &[
                    ("status", json!("failed")),
                    ("outcome", json!(crate::engine::DISPATCH_DIED)),
                    (journal::SETTLED_CAUSE, cause),
                    (journal::SETTLED_HEAD, head),
                ],
            )
        };
        let state = fold(&[
            pipeline(
                journal::PipelineKind::RunStarted,
                0,
                None,
                &[("plan", json!(plan))],
            ),
            settled(1, "good", json!("rate_limit"), json!("abc123")),
            // A classification that is a sentence, and a commit carrying a line
            // of its own — the two ways a hand-edited record forges a row.
            settled(
                2,
                "forged",
                json!("the harness said a great many things about this"),
                json!("abc123\n  ship        done"),
            ),
        ]);

        assert_eq!(
            state.causes.get("good").map(String::as_str),
            Some("rate_limit")
        );
        assert_eq!(state.heads.get("good").map(String::as_str), Some("abc123"));
        assert_eq!(
            state.causes.get("forged"),
            None,
            "a classification this build cannot use was folded anyway"
        );
        assert_eq!(
            state.heads.get("forged"),
            None,
            "a commit carrying a line of its own was folded onto a view"
        );
        // The word itself is not conditional on either: what the dispatch was is
        // still what it was.
        assert_eq!(
            state.outcomes.get("forged").map(String::as_str),
            Some(crate::engine::DISPATCH_DIED)
        );
    }

    /// A landing outlives the dispatch that recorded it, only a re-settlement
    /// moves it, and an unreadable one records nothing.
    ///
    /// Execution is continuous, so the only thing that can change what this run
    /// says about a published change is the node settling again. An open change
    /// request that stopped being reported while the run carried on is precisely
    /// when a planner starts deciding there is nothing left to do, and a landing
    /// a *newer* build spelled a word this one cannot read has to fold as
    /// nothing observed rather than as a guess.
    #[test]
    fn a_landing_outlives_its_dispatch_moves_only_on_a_re_settlement_and_an_unreadable_one_records_nothing(
    ) {
        let plan = plan_of_nodes(vec![agent("open", &[]), agent("guessy", &[])]);
        let settled = |seq: u64, node: &str, landing: Value| {
            pipeline(
                journal::PipelineKind::NodeSettled,
                seq,
                Some(node),
                &[
                    ("status", json!("done")),
                    ("outcome", json!("change-open")),
                    (journal::SETTLED_LANDING, landing),
                ],
            )
        };
        let events = vec![
            pipeline(
                journal::PipelineKind::RunStarted,
                0,
                None,
                &[("plan", json!(plan))],
            ),
            settled(1, "open", json!("unlanded")),
            // A word a newer writer used and this build cannot interpret.
            settled(2, "guessy", json!("half-landed")),
            // Work the loop kept doing after both settled. None of it is about
            // either node's change, so neither claim may move.
            pipeline(journal::PipelineKind::NodeDispatched, 3, Some("later"), &[]),
            pipeline(
                journal::PipelineKind::NodeSettled,
                4,
                Some("later"),
                &[("status", json!("done"))],
            ),
        ];

        let state = fold(&events);
        assert_eq!(
            state.landings.get("open"),
            Some(&Landing::Unlanded),
            "the open change stopped being reported while the run carried on"
        );
        assert_eq!(
            state.landings.get("guessy"),
            None,
            "a landing this build cannot read was folded as one it could"
        );

        // The one thing that moves it: the node settling again, on a change the
        // host has since merged.
        let mut relanded = events;
        relanded.push(settled(5, "open", json!("landed")));
        assert_eq!(
            fold(&relanded).landings.get("open"),
            Some(&Landing::Landed),
            "a node that settled again did not overwrite its own landing"
        );
    }

    #[test]
    fn the_fold_reconstructs_the_graph_the_run_is_executing() {
        let plan = plan_of_nodes(vec![agent("build", &[]), agent("ship", &["build"])]);
        let retry = Operation::NodeAdded {
            node: Box::new(agent("build-2", &[])),
            retry_of: Some("build".into()),
        };
        let events = vec![
            pipeline(
                journal::PipelineKind::RunStarted,
                0,
                None,
                &[("plan", json!(plan))],
            ),
            pipeline(journal::PipelineKind::NodeReady, 1, Some("build"), &[]),
            pipeline(journal::PipelineKind::NodeDispatched, 2, Some("build"), &[]),
            pipeline(
                journal::PipelineKind::NodeSettled,
                3,
                Some("build"),
                &[
                    ("status", json!("failed")),
                    ("outcome", json!("gate-failed")),
                ],
            ),
            pipeline(
                journal::PipelineKind::EditCommitted,
                4,
                None,
                &[
                    ("command", json!({"op": "retry"})),
                    (
                        "operations",
                        json!([
                            Operation::RetryRequested {
                                node: "build".into(),
                                replacement: "build-2".into(),
                                reset: vec!["ship".into()],
                            },
                            retry,
                        ]),
                    ),
                ],
            ),
        ];

        let state = fold(&events);
        assert!(state.strict);
        assert!(
            state.graph.contains("build-2"),
            "the replacement is not in the plan of record"
        );
        assert_eq!(state.recorded["build"].status(), NodeStatus::Cancelled);
        assert_eq!(state.outcomes["build"], "gate-failed");
        assert!(state.dispatched_at.contains_key("build"));
        assert!(state.settled_at.contains_key("build"));
    }

    /// The graph a run's journal replays is the graph the reconciler compiled —
    /// release targets included.
    ///
    /// `deps` is reconstructed here from `reparent`, `edge-added` and
    /// `edge-removed` rather than copied, so a `consumes` change the reconciler
    /// made and the operation stream did not carry would leave the projected
    /// graph disagreeing with the executing one about which artifact a node
    /// builds against. Each of the three ops that moves a node's `deps` is driven
    /// through the reconciler and then through this module's own fold, and the
    /// two maps are compared.
    #[test]
    fn a_replayed_journal_reconstructs_the_consumes_the_reconciler_compiled() {
        let target = |name: &str| {
            name.parse::<onevcs::releases::TargetName>()
                .expect("a release target name")
        };
        let consuming_plan = || {
            let mut ship = agent("ship", &["engine", "packager"]);
            ship.consumes.insert("engine".into(), target("crate"));
            ship.consumes.insert("packager".into(), target("wheel"));
            plan_of_nodes(vec![
                agent("engine", &[]),
                agent("packager", &[]),
                agent("docs", &[]),
                ship,
            ])
        };

        for (what, command, recorded) in [
            (
                "retry",
                crate::channel::Command::Retry {
                    id: "engine".into(),
                    node: agent("engine-2", &[]),
                },
                vec![("engine", NodeStatus::Failed)],
            ),
            (
                "drop",
                crate::channel::Command::Drop {
                    id: "engine".into(),
                    dependents: crate::channel::Dependents::Detach,
                },
                Vec::new(),
            ),
            (
                "reparent",
                crate::channel::Command::Reparent {
                    id: "ship".into(),
                    deps: vec!["packager".into(), "docs".into()],
                },
                Vec::new(),
            ),
        ] {
            let plan = consuming_plan();
            let mut live = Graph::from_plan(&plan);
            let frontier = Frontier {
                recorded: recorded
                    .iter()
                    .map(|(id, status)| ((*id).to_string(), *status))
                    .collect(),
                ..Frontier::default()
            };
            let operations = edits::compile(&mut live, &frontier, &command)
                .unwrap_or_else(|e| panic!("the {what} is accepted: {e}"));

            let mut events = vec![pipeline(
                journal::PipelineKind::RunStarted,
                0,
                None,
                &[("plan", json!(plan))],
            )];
            for (seq, (id, _)) in recorded.iter().enumerate() {
                let seq = seq as u64 + 1;
                events.push(pipeline(
                    journal::PipelineKind::NodeDispatched,
                    seq,
                    Some(id),
                    &[],
                ));
                events.push(pipeline(
                    journal::PipelineKind::NodeSettled,
                    seq + 1,
                    Some(id),
                    &[
                        ("status", json!("failed")),
                        ("outcome", json!(crate::engine::DISPATCH_DIED)),
                    ],
                ));
            }
            events.push(pipeline(
                journal::PipelineKind::EditCommitted,
                9,
                None,
                &[("operations", json!(operations))],
            ));

            let replayed = fold(&events).graph;
            assert_eq!(
                replayed
                    .iter()
                    .map(|node| (node.id.clone(), node.consumes.clone()))
                    .collect::<BTreeMap<_, _>>(),
                live.iter()
                    .map(|node| (node.id.clone(), node.consumes.clone()))
                    .collect::<BTreeMap<_, _>>(),
                "the {what} the journal replays consumes different targets than the one \
                 the reconciler compiled"
            );
            // And the maps compared are not two empty ones: the edit moved a
            // target rather than leaving the graph without any.
            assert!(
                replayed.iter().any(|node| !node.consumes.is_empty()),
                "the {what} left no target for this comparison to be about"
            );
        }
    }

    #[test]
    fn an_edit_whose_operations_cannot_be_folded_ends_strict_replay() {
        let plan = plan_of_nodes(vec![agent("build", &[])]);
        let events = vec![
            pipeline(
                journal::PipelineKind::RunStarted,
                0,
                None,
                &[("plan", json!(plan))],
            ),
            pipeline(
                journal::PipelineKind::EditCommitted,
                1,
                None,
                &[("operations", json!([{"kind": "from-the-future"}]))],
            ),
        ];
        let state = fold(&events);
        assert!(!state.strict, "an unfoldable operation was folded anyway");
    }

    /// The frontier is continuous: what a node last recorded stands until it
    /// records something else, and a settled node stays settled without a round
    /// boundary to clear it.
    #[test]
    fn a_settled_node_keeps_its_status_and_its_dependent_becomes_ready() {
        let plan = plan_of_nodes(vec![agent("build", &[]), agent("ship", &["build"])]);
        let state = fold(&[
            pipeline(
                journal::PipelineKind::RunStarted,
                0,
                None,
                &[("plan", json!(plan))],
            ),
            pipeline(journal::PipelineKind::NodeDispatched, 1, Some("build"), &[]),
            pipeline(
                journal::PipelineKind::NodeSettled,
                2,
                Some("build"),
                &[("status", json!("done"))],
            ),
        ]);
        assert_eq!(state.recorded["build"].status(), NodeStatus::Done);
        assert_eq!(
            state.statuses()["ship"],
            NodeStatus::Ready,
            "a dependent did not become ready on its dependency's settlement"
        );
    }

    /// A note carries exactly one dispatch. The dispatch that takes it consumes
    /// it, so a later attempt is not handed a correction the worker has already
    /// acted on.
    #[test]
    fn a_carried_note_attaches_to_the_next_dispatch_and_is_consumed_by_it() {
        let plan = plan_of_nodes(vec![agent("build", &[])]);
        let note = pipeline(
            journal::PipelineKind::EditCommitted,
            1,
            None,
            &[(
                "operations",
                json!([Operation::ContextAdded {
                    node: "build".into(),
                    note: "the gate needs the lockfile".into(),
                    delivery: edits::Delivery::Deferred,
                }]),
            )],
        );
        let started = pipeline(
            journal::PipelineKind::RunStarted,
            0,
            None,
            &[("plan", json!(plan))],
        );
        let attached = fold(&[started.clone(), note.clone()]);
        assert_eq!(
            attached.pending_context["build"],
            "the gate needs the lockfile"
        );
        assert_eq!(
            attached
                .graph
                .get("build")
                .expect("build")
                .context
                .as_deref(),
            Some("the gate needs the lockfile"),
            "the note did not reach the node it is for"
        );

        let consumed = fold(&[
            started,
            note,
            pipeline(journal::PipelineKind::NodeDispatched, 2, Some("build"), &[]),
        ]);
        assert!(
            !consumed.pending_context.contains_key("build"),
            "a note outlived the dispatch that took it"
        );
        assert_eq!(
            consumed.graph.get("build").expect("build").context,
            None,
            "a note outlived the dispatch that took it"
        );
    }

    /// A live delivery is not also owed to the next dispatch.
    #[test]
    fn a_note_the_running_turn_took_is_never_owed_to_a_later_dispatch() {
        let plan = plan_of_nodes(vec![agent("build", &[])]);
        let state = fold(&[
            pipeline(
                journal::PipelineKind::RunStarted,
                0,
                None,
                &[("plan", json!(plan))],
            ),
            pipeline(
                journal::PipelineKind::EditCommitted,
                1,
                None,
                &[(
                    "operations",
                    json!([Operation::ContextAdded {
                        node: "build".into(),
                        note: "look at the lockfile".into(),
                        delivery: edits::Delivery::Live,
                    }]),
                )],
            ),
        ]);
        assert!(state.pending_context.is_empty());
        assert_eq!(state.graph.get("build").expect("build").context, None);
    }

    /// An arrival note the running turn could not take is owed to the node's next
    /// dispatch, and is reconstructed from the versions the record names.
    ///
    /// The record is the only durable thing: nothing submitted this note, so
    /// there is no `edit-committed` and no author to attribute it to. What makes
    /// a replayed note the note that was sent is that both come out of
    /// `release::arrival_note`.
    #[test]
    fn an_arrival_note_that_did_not_reach_a_running_turn_is_owed_to_the_next_dispatch() {
        let plan = plan_of_nodes(vec![agent("build", &[])]);
        let started = pipeline(
            journal::PipelineKind::RunStarted,
            0,
            None,
            &[("plan", json!(plan))],
        );
        let versions = json!([{
            "identity": "github.com/nickderobertis/onevcs",
            "target": "crate",
            "version": "0.13.0"
        }]);
        let adopted = |seq: u64, delivery: &str| {
            pipeline(
                journal::PipelineKind::ReleaseAdopted,
                seq,
                Some("build"),
                &[
                    ("node", json!("build")),
                    ("delivery", json!(delivery)),
                    ("versions", versions.clone()),
                ],
            )
        };

        let owed = fold(&[started.clone(), adopted(1, "next")]);
        let note = owed
            .pending_context
            .get("build")
            .expect("the note is owed to the next dispatch");
        assert!(
            note.contains("github.com/nickderobertis/onevcs — crate 0.13.0"),
            "{note}"
        );
        assert_eq!(
            owed.graph.get("build").expect("build").context.as_deref(),
            Some(note.as_str()),
            "the note did not reach the node it is for"
        );

        // A record naming no release this build can read is skipped: a note that
        // told a worker its releases had arrived and named not one of them would
        // be worse than none.
        let unreadable = fold(&[
            started.clone(),
            pipeline(
                journal::PipelineKind::ReleaseAdopted,
                1,
                Some("build"),
                &[
                    ("node", json!("build")),
                    ("delivery", json!("next")),
                    ("versions", json!([{"identity": "", "target": "crate"}])),
                ],
            ),
        ]);
        assert!(unreadable.pending_context.is_empty());
        assert_eq!(unreadable.graph.get("build").expect("build").context, None);

        // A note the running turn took is not also owed to the next dispatch —
        // the same rule a planner's own live note is folded under.
        let taken = fold(&[started.clone(), adopted(1, "live")]);
        assert!(taken.pending_context.is_empty());
        assert_eq!(taken.graph.get("build").expect("build").context, None);

        // And the dispatch that takes it consumes it.
        let consumed = fold(&[
            started,
            adopted(1, "next"),
            pipeline(journal::PipelineKind::NodeDispatched, 2, Some("build"), &[]),
        ]);
        assert!(!consumed.pending_context.contains_key("build"));

        // The two reports beside it change nothing about the graph.
        for kind in [
            journal::PipelineKind::ReleaseWait,
            journal::PipelineKind::ReleaseArrived,
        ] {
            let reported = fold(&[
                pipeline(
                    journal::PipelineKind::RunStarted,
                    0,
                    None,
                    &[("plan", json!(plan_of_nodes(vec![agent("build", &[])])))],
                ),
                pipeline(kind, 1, Some("build"), &[("node", json!("build"))]),
            ]);
            assert!(
                reported.pending_context.is_empty(),
                "{kind} changed the graph"
            );
            assert_eq!(reported.graph.get("build").expect("build").context, None);
        }
    }

    /// A driver that takes a run over ends the dispatches the one before it left
    /// in flight: they were threads of a process that is gone.
    ///
    /// Left recorded as running, such a node is never ready — so nothing
    /// dispatches it again — and never terminal, so the loop that adopted the
    /// run spins on it for good. This is the boundary at which the frontier
    /// learns that.
    #[test]
    fn an_adoption_ends_the_dispatches_the_driver_before_it_left_running() {
        let plan = plan_of_nodes(vec![agent("build", &[]), agent("ship", &["build"])]);
        let events = vec![
            pipeline(
                journal::PipelineKind::RunStarted,
                0,
                None,
                &[("plan", json!(plan))],
            ),
            pipeline(journal::PipelineKind::NodeDispatched, 1, Some("build"), &[]),
        ];
        let held = fold(&events);
        assert_eq!(held.recorded["build"].status(), NodeStatus::Running);
        assert_eq!(held.statuses()["build"], NodeStatus::Running);

        let mut adopted = events;
        adopted.push(pipeline(
            journal::PipelineKind::DriverAdopted,
            2,
            None,
            &[("adoption", json!(1))],
        ));
        let state = fold(&adopted);
        assert!(!state.recorded.contains_key("build"));
        assert_eq!(
            state.statuses()["build"],
            NodeStatus::Ready,
            "a node the dead driver left running was not offered to the fresh one"
        );
    }

    /// The `session-opened` a lifecycle dispatch's session produces, as it is
    /// relayed into the merged store.
    ///
    /// Built through [`crate::vcs::session_opened_event`] rather than by naming
    /// the kind and the fields here: what a session opening is spelled as, and
    /// which fields carry the token and the branch, is `onevcs`'s vocabulary —
    /// and a fixture that restated it would keep folding after the producer
    /// changed and prove nothing about what arrives.
    fn opened(seq: u64, node: &str, token: &str, branch: &str) -> Envelope {
        let session = onevcs::Session {
            token: onevcs::SessionToken(token.into()),
            worktree: std::path::PathBuf::from("/tmp/worktree"),
            branch: branch.into(),
            base: "main".into(),
        };
        Envelope {
            seq,
            ..crate::vcs::session_opened_event(
                &session,
                &Labels {
                    node: Some(node.to_string()),
                    ..labels("demo", None)
                },
            )
        }
    }

    /// A driver dying does not end the *work* its dispatch was doing: the branch
    /// holds whatever the worker committed and the session still knows where.
    ///
    /// Both halves of what the adoption owes are held here — the session is
    /// named, so a manager can find the branch, and the node is pinned to it, so
    /// the re-dispatch continues that branch rather than cutting a second one
    /// beside committed work nothing points at.
    #[test]
    fn an_adoption_names_the_dispatch_it_cleared_and_pins_its_node_to_that_branch() {
        let plan = plan_of_nodes(vec![agent("service", &[]), agent("audit", &[])]);
        let events = vec![
            pipeline(
                journal::PipelineKind::RunStarted,
                0,
                None,
                &[("plan", json!(plan))],
            ),
            pipeline(
                journal::PipelineKind::NodeDispatched,
                1,
                Some("service"),
                &[],
            ),
            opened(2, "service", "s-abc", "onevcs/s-abc"),
            // A second node dispatched with no session of its own: a direct
            // agent node has no repository, so there is no branch to name.
            pipeline(journal::PipelineKind::NodeDispatched, 3, Some("audit"), &[]),
            pipeline(
                journal::PipelineKind::DriverAdopted,
                4,
                None,
                &[("adoption", json!(1))],
            ),
        ];
        let state = fold(&events);

        let left = &state.abandoned["service"];
        assert_eq!(left.token(), &onevcs::SessionToken("s-abc".into()));
        assert_eq!(left.branch().as_str(), "onevcs/s-abc");
        assert!(
            !state.abandoned.contains_key("audit"),
            "a dispatch that opened no session was reported as work left somewhere"
        );
        assert_eq!(
            state
                .graph
                .get("service")
                .and_then(|node| node.branch.clone()),
            Some("onevcs/s-abc".to_string()),
            "the cleared node was not pinned to the branch its dispatch committed on"
        );
        // And it is offered to the fresh driver exactly as it always was.
        assert_eq!(state.statuses()["service"], NodeStatus::Ready);
        assert!(state.sessions.is_empty());
    }

    /// The session an *earlier* attempt worked in is not where the current one
    /// is.
    ///
    /// A node dispatched again after settling has a new dispatch and no session
    /// yet, and an adoption that read the old one would send a manager to a
    /// branch this attempt never touched — and pin the node to it.
    #[test]
    fn a_session_an_earlier_attempt_finished_with_is_not_where_the_next_one_is_working() {
        let plan = plan_of_nodes(vec![agent("service", &[])]);
        let events = vec![
            pipeline(
                journal::PipelineKind::RunStarted,
                0,
                None,
                &[("plan", json!(plan))],
            ),
            pipeline(
                journal::PipelineKind::NodeDispatched,
                1,
                Some("service"),
                &[],
            ),
            opened(2, "service", "s-first", "onevcs/s-first"),
            pipeline(
                journal::PipelineKind::NodeSettled,
                3,
                Some("service"),
                &[("status", json!("failed"))],
            ),
            pipeline(
                journal::PipelineKind::NodeDispatched,
                4,
                Some("service"),
                &[],
            ),
            pipeline(
                journal::PipelineKind::DriverAdopted,
                5,
                None,
                &[("adoption", json!(1))],
            ),
        ];
        let state = fold(&events);
        assert!(
            state.abandoned.is_empty(),
            "an adoption named a session the current dispatch never opened: {:?}",
            state.abandoned
        );
    }

    /// A record this fold cannot place, or cannot read a whole session out of,
    /// names nothing.
    ///
    /// Which node a session belongs to is stamped by the *enricher* rather than
    /// by the producer that opened it, so a record naming none is not a dispatch
    /// of this run's. And half a session is worse than none: a branch with no
    /// token leaves a manager unable to find the worktree, and a token with no
    /// branch is a pin this crate would have to invent. What makes a *value*
    /// usable is decided where it is read, in
    /// [`DispatchSession::read_from`](crate::vcs::DispatchSession::read_from).
    #[test]
    fn a_session_record_this_run_cannot_place_is_left_out_of_the_fold() {
        let plan = plan_of_nodes(vec![agent("service", &[])]);
        let mut unlabelled = opened(2, "service", "s-abc", "onevcs/s-abc");
        unlabelled.labels.node = None;
        let mut branchless = opened(3, "service", "s-abc", "");
        branchless.payload.remove("branch");
        let state = fold(&[
            pipeline(
                journal::PipelineKind::RunStarted,
                0,
                None,
                &[("plan", json!(plan))],
            ),
            pipeline(
                journal::PipelineKind::NodeDispatched,
                1,
                Some("service"),
                &[],
            ),
            unlabelled,
            branchless,
            pipeline(
                journal::PipelineKind::DriverAdopted,
                4,
                None,
                &[("adoption", json!(1))],
            ),
        ]);
        assert!(state.abandoned.is_empty(), "{:?}", state.abandoned);
        assert!(
            state
                .graph
                .get("service")
                .and_then(|node| node.branch.clone())
                .is_none(),
            "a node was pinned to a branch no record named"
        );
    }

    /// The same proof ends a cancellation the previous driver was waiting on:
    /// the dispatch it asked to stop was a thread of that process.
    ///
    /// The park stands, because a park is the planner's own idle and only a
    /// requeue clears it. What ends is the wait — left standing, the run reports
    /// a stop nothing is converging on for as long as it exists.
    #[test]
    fn an_adoption_ends_a_cancellation_the_driver_before_it_was_waiting_on() {
        let plan = plan_of_nodes(vec![agent("sweep", &[])]);
        let events = vec![
            pipeline(
                journal::PipelineKind::RunStarted,
                0,
                None,
                &[("plan", json!(plan))],
            ),
            pipeline(journal::PipelineKind::NodeDispatched, 1, Some("sweep"), &[]),
            pipeline(
                journal::PipelineKind::EditCommitted,
                2,
                None,
                &[(
                    "operations",
                    json!([Operation::NodeParked {
                        node: "sweep".into()
                    }]),
                )],
            ),
        ];
        assert!(
            fold(&events).recorded["sweep"].cancelling_since().is_some(),
            "the cancel left nothing for the adoption to end"
        );

        let mut adopted = events;
        adopted.push(pipeline(
            journal::PipelineKind::DriverAdopted,
            3,
            None,
            &[("adoption", json!(1))],
        ));
        let state = fold(&adopted);
        assert_eq!(
            state.recorded["sweep"],
            Recorded::At(NodeStatus::Parked),
            "the run is still waiting on a dispatch that went with its driver"
        );
    }

    /// A node that failed with work on a branch is pinned to it, so whatever
    /// runs it again continues that branch rather than cutting a fresh one
    /// beside committed work nothing points at.
    #[test]
    fn a_settlement_that_preserved_a_branch_pins_the_node_to_it() {
        let plan = plan_of_nodes(vec![agent("build", &[])]);
        let state = fold(&[
            pipeline(
                journal::PipelineKind::RunStarted,
                0,
                None,
                &[("plan", json!(plan))],
            ),
            pipeline(
                journal::PipelineKind::NodeSettled,
                1,
                Some("build"),
                &[
                    ("status", json!("failed")),
                    ("branch", json!("onepipeline/build")),
                    ("completed_steps", json!(["implement"])),
                ],
            ),
        ]);
        let node = state.graph.get("build").expect("build");
        assert_eq!(node.branch.as_deref(), Some("onepipeline/build"));
        let resume = node.resume.as_ref().expect("the node resumes its branch");
        assert_eq!(resume.branch, "onepipeline/build");
        assert_eq!(resume.completed_steps, vec!["implement".to_string()]);
    }

    /// A node that finished has nothing to preserve: pinning one would make
    /// every later reader believe there is work on a branch nobody wrote.
    #[test]
    fn a_node_that_completed_is_not_pinned_to_a_branch_to_continue() {
        let plan = plan_of_nodes(vec![agent("build", &[])]);
        let state = fold(&[
            pipeline(
                journal::PipelineKind::RunStarted,
                0,
                None,
                &[("plan", json!(plan))],
            ),
            pipeline(
                journal::PipelineKind::NodeSettled,
                1,
                Some("build"),
                &[
                    ("status", json!("done")),
                    ("branch", json!("onepipeline/build")),
                ],
            ),
        ]);
        assert_eq!(state.graph.get("build").expect("build").resume, None);
    }

    #[test]
    fn attestations_completions_surfaces_and_stops_are_all_folded() {
        let plan = plan_of_nodes(vec![Node {
            id: "approve".into(),
            kind: crate::plan::NodeKind::Human,
            task: Some("approve it".into()),
            ..Node::default()
        }]);
        let events = vec![
            pipeline(
                journal::PipelineKind::RunStarted,
                0,
                None,
                &[("plan", json!(plan))],
            ),
            pipeline(journal::PipelineKind::PlannerSurfaceQueued, 1, None, &[]),
            pipeline(journal::PipelineKind::PlannerSurfaced, 2, None, &[]),
            pipeline(
                journal::PipelineKind::HumanAttested,
                3,
                None,
                &[("ref", json!("approve"))],
            ),
            pipeline(
                journal::PipelineKind::CompletionRequested,
                4,
                None,
                &[("reason", json!("verified"))],
            ),
            pipeline(
                journal::PipelineKind::UpstreamModified,
                5,
                Some("consumer"),
                &[("dependency", json!("run:o#n"))],
            ),
            pipeline(journal::PipelineKind::RunStopped, 6, None, &[]),
        ];
        let state = fold(&events);
        assert_eq!(state.surfaces_queued, 1);
        assert_eq!(state.surfaces_read, 1);
        assert!(state.last_surface_at.is_some());
        assert!(state.attestations.contains("approve"));
        assert_eq!(state.recorded["approve"].status(), NodeStatus::Done);
        assert_eq!(state.completion_requests, vec!["verified".to_string()]);
        assert_eq!(state.cross_dag_watches["run:o#n"], 1);
        assert!(state.stop_recorded());
    }

    #[test]
    fn an_identity_declined_stop_leaves_worker_state_undetermined() {
        let state = fold(&[pipeline(
            journal::PipelineKind::RunStopped,
            0,
            None,
            &[("teardown", json!("identity-declined"))],
        )]);

        assert_eq!(state.stop, StopState::WorkersUndetermined);
        assert!(state.stop_recorded());
    }

    #[test]
    fn a_relayed_sibling_envelope_is_evidence_of_work_and_nothing_more() {
        let plan = plan_of_nodes(vec![agent("build", &[])]);
        let mut relayed = pipeline(
            journal::PipelineKind::NodeSettled,
            1,
            Some("build"),
            &[("status", json!("done"))],
        );
        relayed.source = Source::Agentgraph;
        let state = fold(&[
            pipeline(
                journal::PipelineKind::RunStarted,
                0,
                None,
                &[("plan", json!(plan))],
            ),
            relayed,
        ]);
        assert!(
            state.recorded.is_empty(),
            "a sibling's envelope decided this crate's graph state"
        );
        assert!(state.last_write_at.is_some());
        assert_eq!(
            state.activity["build"]
                .progress
                .expect("the relay counted")
                .events(),
            1
        );
    }

    /// The readout an operator decides between cancel, retry, and wait on. A
    /// node in flight for half an hour reads identically to a wedged one
    /// without it, and a healthy node has been reported dead on exactly that.
    #[test]
    fn a_relayed_turn_activity_says_what_the_node_is_doing_now() {
        let plan = plan_of_nodes(vec![agent("build", &[])]);
        let activity = |seq: u64, name: &str, detail: &str| {
            let mut event = pipeline(
                journal::PipelineKind::NodeDispatched,
                seq,
                Some("build"),
                &[("name", json!(name)), ("detail", json!(detail))],
            );
            event.source = Source::Agentgraph;
            event.kind = crate::event::EventKind(TURN_ACTIVITY.into());
            event
        };
        let state = fold(&[
            pipeline(
                journal::PipelineKind::RunStarted,
                0,
                None,
                &[("plan", json!(plan))],
            ),
            pipeline(journal::PipelineKind::NodeDispatched, 1, Some("build"), &[]),
            activity(2, "Bash", "cargo llvm-cov --workspace"),
            activity(3, "Read", "src/engine.rs"),
        ]);

        let seen = &state.activity["build"];
        assert_eq!(seen.doing.as_deref(), Some("Read src/engine.rs"));
        assert_eq!(
            seen.progress.expect("the activities counted").events(),
            2,
            "the node-dispatched was counted as activity"
        );
        assert_eq!(
            Some(seen.progress.expect("the activities counted").last_at()),
            millis_of(&activity(3, "Read", "x").ts)
        );
    }

    /// An invocation is recorded only where the whole of it reads: the
    /// producing library's own kind, an envelope naming the node it belongs to,
    /// and that library's own declared payload.
    ///
    /// An identity assembled out of whatever fields happened to be present would
    /// name something as having served a turn on no better evidence than that it
    /// used the word — which is the invented attribution this whole record
    /// exists to replace.
    #[test]
    fn an_invocation_is_recorded_only_where_the_whole_record_reads() {
        let published = |member: Option<Value>, node: Option<&str>, payload: Value| {
            let mut event = pipeline(journal::PipelineKind::NodeDispatched, 1, node, &[]);
            event.source = Source::Agentgraph;
            event.kind = crate::event::EventKind("oneharness-session".into());
            event.payload = match payload {
                Value::Object(fields) => fields,
                other => panic!("a payload is not an object: {other:?}"),
            };
            if let Some(member) = member {
                event.labels.extra.insert("member".into(), member);
            }
            event
        };
        let whole = || {
            json!({
                "role": "judge", "turn": 2, "identity": "codex:alternate",
                "history_id": "record-1", "history_dir": "/store",
                "history_project": "project", "history_session": "record-1",
            })
        };

        // A member the producer stamped, one it did not, and one that is not a
        // member name: three different facts about the record, kept apart
        // exactly as an advance's are.
        let state = fold(&[
            published(Some(json!("worker")), Some("build"), whole()),
            published(None, Some("build"), whole()),
            published(Some(json!(7)), Some("build"), whole()),
        ]);
        assert_eq!(
            state.served["build"]
                .iter()
                .map(|served| served.member.clone())
                .collect::<Vec<_>>(),
            vec![
                MemberLabel::Named("worker".into()),
                MemberLabel::Unstamped,
                MemberLabel::Unreadable,
            ]
        );
        let first = &state.served["build"][0].session;
        assert_eq!(first.role, oneagentgraph::event::Role::Judge);
        assert_eq!(first.turn, 2);
        assert_eq!(first.identity, "codex:alternate");

        // A kind this build has no reading of, an envelope belonging to no
        // node, and a payload the producing library's own type refuses.
        let mut wrong_kind = published(None, Some("build"), whole());
        wrong_kind.kind = crate::event::EventKind("oneharness-sessions".into());
        let mut wrong_source = published(None, Some("build"), whole());
        wrong_source.source = Source::Vcs;
        assert!(fold(&[
            wrong_kind,
            wrong_source,
            published(None, None, whole()),
            published(None, Some("build"), json!({"identity": "codex"})),
        ])
        .served
        .is_empty());
    }

    /// A tool the producer named nothing for leaves the last thing it *did*
    /// name standing: a blanked line reads as a dispatch that stopped working.
    #[test]
    fn an_activity_naming_no_tool_does_not_erase_the_one_before_it() {
        let mut nameless = pipeline(journal::PipelineKind::NodeDispatched, 3, Some("build"), &[]);
        nameless.source = Source::Agentgraph;
        nameless.kind = crate::event::EventKind(TURN_ACTIVITY.into());
        let mut named = nameless.clone();
        named.seq = 2;
        named.payload = payload(&[("name", json!("Bash")), ("detail", json!("just check"))]);

        let state = fold(&[named, nameless]);
        assert_eq!(
            state.activity["build"].doing.as_deref(),
            Some("Bash just check")
        );
        assert_eq!(
            state.activity["build"]
                .progress
                .expect("the relays counted")
                .events(),
            2
        );
    }

    /// Activity is the node's whole record, not one attempt's: a dispatch that
    /// starts again does not erase what the node has already been seen doing.
    #[test]
    fn a_nodes_activity_accumulates_across_the_attempts_it_was_dispatched_for() {
        let activity = |seq: u64| {
            let mut event = pipeline(
                journal::PipelineKind::NodeDispatched,
                seq,
                Some("build"),
                &[],
            );
            event.source = Source::Agentgraph;
            event.kind = crate::event::EventKind(TURN_ACTIVITY.into());
            event
        };
        let state = fold(&[
            activity(1),
            pipeline(journal::PipelineKind::NodeDispatched, 2, Some("build"), &[]),
            activity(3),
        ]);
        assert_eq!(
            state.activity["build"]
                .progress
                .expect("the relays counted")
                .events(),
            2
        );
    }

    /// A heartbeat is evidence the dispatch is *there*, and evidence of nothing
    /// else. Counted as work it makes every live node look busy: the producer
    /// publishes one every few seconds whether or not anything happened.
    #[test]
    fn a_heartbeat_is_folded_as_liveness_rather_than_as_work() {
        let relayed = |seq: u64, kind: &str| {
            let mut event = pipeline(
                journal::PipelineKind::NodeDispatched,
                seq,
                Some("build"),
                &[("name", json!("Bash")), ("detail", json!("just check"))],
            );
            event.source = Source::Agentgraph;
            event.kind = crate::event::EventKind(kind.into());
            event
        };
        let beat = oneagentgraph::event::EventKind::MemberHeartbeat.as_str();
        let state = fold(&[
            relayed(1, TURN_ACTIVITY),
            relayed(2, beat),
            relayed(3, beat),
        ]);

        let seen = &state.activity["build"];
        let progress = seen.progress.expect("the activity counted");
        assert_eq!(progress.events(), 1, "a heartbeat was counted as work");
        assert_eq!(
            Some(progress.last_at()),
            millis_of(&relayed(1, TURN_ACTIVITY).ts),
            "a heartbeat advanced the age of the work"
        );
        assert_eq!(
            seen.last_heartbeat_at,
            millis_of(&relayed(3, beat).ts),
            "the dispatch's liveness was dropped rather than recorded"
        );
        // And it does not overwrite what the node was last seen doing: a turn
        // that is wedged is still wedged doing something.
        assert_eq!(seen.doing.as_deref(), Some("Bash just check"));
    }

    /// A cancel of a *running* node leaves a dispatch behind it; a cancel of one
    /// that never started leaves nothing. Both record `parked`, so the wait is
    /// the only thing that tells them apart.
    #[test]
    fn only_a_cancel_that_left_a_dispatch_behind_records_a_cancellation_in_flight() {
        let plan = plan_of_nodes(vec![agent("sweep", &[]), agent("later", &[])]);
        let started = pipeline(
            journal::PipelineKind::RunStarted,
            0,
            None,
            &[("plan", json!(plan))],
        );
        let park = |seq: u64, node: &str| {
            pipeline(
                journal::PipelineKind::EditCommitted,
                seq,
                None,
                &[(
                    "operations",
                    json!([Operation::NodeParked { node: node.into() }]),
                )],
            )
        };
        let dispatched = pipeline(journal::PipelineKind::NodeDispatched, 1, Some("sweep"), &[]);
        let state = fold(&[
            started.clone(),
            dispatched.clone(),
            park(2, "sweep"),
            park(3, "later"),
        ]);
        assert_eq!(
            state.recorded["sweep"],
            Recorded::Cancelling {
                since: millis_of(&park(2, "sweep").ts).expect("the park is stamped")
            },
            "a cancel that left a dispatch running recorded no wait"
        );
        assert_eq!(
            state.recorded["later"],
            Recorded::At(NodeStatus::Parked),
            "a cancel of a node that never started reported a dispatch to wait for"
        );

        // The settlement is what ends the wait: the dispatch has let go, so the
        // node's state and what is running for it agree again.
        let settled = pipeline(
            journal::PipelineKind::NodeSettled,
            4,
            Some("sweep"),
            &[("status", json!("cancelled"))],
        );
        let state = fold(&[started, dispatched, park(2, "sweep"), settled]);
        assert_eq!(
            state.recorded["sweep"],
            Recorded::At(NodeStatus::Cancelled),
            "the settlement left the node still waiting on the dispatch that settled it"
        );
    }

    #[test]
    fn parking_and_requeueing_move_the_node_in_and_out_of_the_frontier() {
        let plan = plan_of_nodes(vec![agent("sweep", &[])]);
        let park = pipeline(
            journal::PipelineKind::EditCommitted,
            1,
            None,
            &[(
                "operations",
                json!([Operation::NodeParked {
                    node: "sweep".into()
                }]),
            )],
        );
        let state = fold(&[
            pipeline(
                journal::PipelineKind::RunStarted,
                0,
                None,
                &[("plan", json!(plan))],
            ),
            park.clone(),
        ]);
        assert_eq!(state.recorded["sweep"].status(), NodeStatus::Parked);
        assert!(state.graph.get("sweep").expect("sweep").parked);

        let requeue = pipeline(
            journal::PipelineKind::EditCommitted,
            2,
            None,
            &[(
                "operations",
                json!([Operation::NodeRequeued {
                    node: "sweep".into(),
                    amend: None
                }]),
            )],
        );
        let state = fold(&[
            pipeline(
                journal::PipelineKind::RunStarted,
                0,
                None,
                &[("plan", json!(plan))],
            ),
            park,
            requeue,
        ]);
        assert!(!state.recorded.contains_key("sweep"));
        assert!(!state.graph.get("sweep").expect("sweep").parked);
    }

    #[test]
    fn the_frontier_and_derived_statuses_come_off_the_same_fold() {
        let plan = plan_of_nodes(vec![agent("build", &[]), agent("ship", &["build"])]);
        let state = fold(&[
            pipeline(
                journal::PipelineKind::RunStarted,
                0,
                None,
                &[("plan", json!(plan))],
            ),
            pipeline(
                journal::PipelineKind::NodeSettled,
                1,
                Some("build"),
                &[("status", json!("done"))],
            ),
        ]);
        assert_eq!(state.frontier().recorded["build"], NodeStatus::Done);
        assert_eq!(state.statuses()["ship"], NodeStatus::Ready);
        let upstream = |_: &str| Some(NodeStatus::Done);
        assert_eq!(state.statuses_with(&upstream)["ship"], NodeStatus::Ready);
    }
}
