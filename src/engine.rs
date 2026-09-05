//! The continuous engine: the single writer that drives a run's whole graph.
//!
//! Execution is one long-lived reconcile loop and nothing else. It compares the
//! live desired graph with the node state projected from the journal, dispatches
//! every node the moment its dependencies settle `done`, and reacts to each
//! completion until the graph is terminal — draining the planner's durable
//! command queue on every pass, so a live edit takes effect immediately.
//!
//! There are no rounds. A finished dependency starts its dependents on the pass
//! that observed the settlement; the only thing that pauses anything is a
//! **decision point**, and it pauses only the subtree that depends on it.
//!
//! Everything here runs under the run's ownership lock. The driving process is
//! the only writer of the graph and the journal's graph records; a second writer
//! would interleave with this loop and corrupt the ledger.

use std::collections::{BTreeMap, BTreeSet};
use std::num::{NonZeroU32, NonZeroU64};
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::agentgraph::{self, Interrupted, TurnAddress};
use crate::channel::{ChannelState, Command, CommandOutcome, Surface};
use crate::edits::{self, Frontier};
use crate::error::{Error, Result};
use crate::event::{Envelope, Labels};
use crate::executor::{
    CancelMode, CancellationToken, DispatchHandle, DispatchOutcome, DispatchRequest, Executor,
    WorkspaceSpec,
};
use crate::graph::{self, Graph, GraphState, Landing, NodeStatus};
use crate::journal::{self, Journal};
use crate::ledger::{self, LaunchRecord, OwnershipLock, RunPaths};
use crate::plan::{Node, NodeKind};
use crate::projection::{self, RunState};
use crate::rules::ExecutorRules;
use crate::sys;

/// The environment variable naming the node-scope agent graph a node dispatches
/// under when it names none.
pub const NODE_GRAPH_ENV: &str = "ONEPIPELINE_NODE_GRAPH";

/// The node-scope agent graph shipped with this crate.
pub const DEFAULT_NODE_GRAPH: &str = "graphs/node-scope.yaml";

/// The environment variable naming the executor-rules file.
pub const EXECUTOR_RULES_ENV: &str = "ONEPIPELINE_EXECUTOR_RULES";

/// The environment variable naming the command a live-edited node is checked by.
///
/// The middle rung of three: `--node-validator` beats it, it beats the launch
/// config's own `node_validator`, and beneath all three is the shipped default
/// of no validator at all. Read once, at the launch, and the answer is retained
/// in the launch record — so an `adopt` replays what its launch resolved rather
/// than whatever this variable happens to say later.
pub const NODE_VALIDATOR_ENV: &str = "ONEPIPELINE_NODE_VALIDATOR";

/// The environment variable naming the command a whole reply envelope is
/// reviewed by.
///
/// The middle rung of three, exactly as [`NODE_VALIDATOR_ENV`] is for the
/// per-node hook: `--envelope-reviewer` beats it, it beats the launch config's
/// own `envelope_reviewer`, and beneath all three is the shipped default of no
/// reviewer at all. Read once, at the launch, and retained in the launch record.
pub const ENVELOPE_REVIEWER_ENV: &str = "ONEPIPELINE_ENVELOPE_REVIEWER";

/// The environment variable naming the directory a direct agent node runs in.
pub const PROJECT_DIR_ENV: &str = "ONEPIPELINE_PROJECT_DIR";

/// The verb the retained driver of a detached launch is spelled with.
///
/// Hidden from `--help` and not part of the documented surface: it is how a
/// launcher that is about to exit hands the engine loop to a process that will
/// outlive it. Nothing but this crate's own launcher spells it.
pub const DRIVE_VERB: &str = "drive-run";

/// The environment variable overriding how long a dispatch may record nothing
/// before the loop surfaces a quiet-worker proposal.
pub const STALL_AFTER_ENV: &str = "ONEPIPELINE_STALL_AFTER_SECONDS";

/// How long a dispatch may record nothing before it is reported quiet.
///
/// Comfortably past the first turns this host runs, so an agent thinking is not
/// reported as an agent that stopped.
pub const DEFAULT_STALL_AFTER_SECONDS: u64 = 2_400;

/// The environment variable setting how many times a dispatch that produced
/// nothing is asked again.
pub const BOUNDARY_ATTEMPTS_ENV: &str = "ONEPIPELINE_BOUNDARY_ATTEMPTS";

/// The environment variable setting the first backoff between those attempts.
pub const BOUNDARY_BACKOFF_ENV: &str = "ONEPIPELINE_BOUNDARY_BACKOFF_SECONDS";

/// How many times a dispatch that produced nothing is asked again.
pub const DEFAULT_BOUNDARY_ATTEMPTS: u32 = 3;

/// The first backoff between those attempts, in seconds. It doubles, to a
/// two-minute ceiling.
pub const DEFAULT_BOUNDARY_BACKOFF_SECONDS: u64 = 5;

/// The ceiling that backoff doubles up to.
const BOUNDARY_BACKOFF_CEILING: Duration = Duration::from_secs(120);

/// The environment variable setting how many times a lifecycle node whose
/// publication keeps failing is dispatched.
pub const PUBLICATION_ATTEMPTS_ENV: &str = "ONEPIPELINE_PUBLICATION_ATTEMPTS";

/// How many times a lifecycle node whose publication keeps failing is dispatched.
///
/// The **whole** budget and not the retries beside it: `1` is the behaviour before
/// there was a loop at all — publish once, and settle on whatever that said.
///
/// Three, because each attempt is a node's entire workstream and the failures it
/// answers are ones a worker fixes by changing the tree: a red check usually goes
/// green on the second look at it, and a check that is still red on the third is
/// one a person has to decide about. A larger budget spends whole dispatches
/// reproducing the same refusal, which is the loop this bound exists to stop.
///
/// A [`NonZeroU32`], because a budget of zero is not a smaller budget — it is a
/// node settled having never been dispatched, which is not a state this loop has.
/// The same reason a turn budget is one, and the same place the check belongs:
/// at the boundary the value is read in from, not at the arithmetic downstream.
pub const DEFAULT_PUBLICATION_ATTEMPTS: NonZeroU32 = NonZeroU32::new(3).unwrap();

/// The environment variable setting how many times the merge path behind a push
/// that reached the remote is read before the node settles unverified.
pub const MERGE_PATH_READS_ENV: &str = "ONEPIPELINE_MERGE_PATH_READS";

/// The environment variable setting the first backoff between those reads.
pub const MERGE_PATH_BACKOFF_ENV: &str = "ONEPIPELINE_MERGE_PATH_BACKOFF_SECONDS";

/// How many times the merge path behind a push that reached the remote is read.
///
/// The **whole** budget and not the re-reads beside it: `1` is reading it once
/// and settling on whatever that said, which is the behaviour before there was a
/// re-read at all.
///
/// Three, because what this answers is a host that was briefly unreachable — an
/// API outage, a 503 — and the cost of another go is one call rather than a fresh
/// clone and a fresh gate. It is deliberately not the publication budget beside
/// it: that one spends whole dispatches, so it is small for a different reason.
///
/// A [`NonZeroU32`] for the reason [`DEFAULT_PUBLICATION_ATTEMPTS`] is: a budget
/// of zero is not a smaller budget, it is a node settled having never read the
/// path it is reporting on.
pub const DEFAULT_MERGE_PATH_READS: NonZeroU32 = NonZeroU32::new(3).unwrap();

/// The first backoff between those reads, in seconds. It doubles, to the same
/// two-minute ceiling every backoff in this crate doubles up to.
pub const DEFAULT_MERGE_PATH_BACKOFF_SECONDS: u64 = 5;

/// The environment variable setting how long a cancelled dispatch has to stop
/// itself before it is torn down.
pub const CANCEL_GRACE_ENV: &str = "ONEPIPELINE_CANCEL_GRACE_SECONDS";

/// How long a cancelled dispatch has to stop itself before the teardown reaps
/// it.
///
/// Long enough for a turn that took the redirection to finish the file it is
/// writing and commit it — which is the whole reason for asking rather than
/// killing — and short enough that a supervisor who cancelled a runaway is not
/// still watching it commit.
pub const DEFAULT_CANCEL_GRACE_SECONDS: u64 = 300;

/// What a cancelled dispatch's live turn is asked to do instead.
///
/// Three things, in the order they have to happen: stop taking on work, put what
/// is already done somewhere it survives, and end. A redirection that said only
/// "stop" would lose whatever the turn had not committed, which is the work a
/// cooperative cancel exists to keep.
pub const CANCEL_INPUT: &str = "Stop this task now. Do not start any new work, and do not begin \
     another file, command, or tool call. Commit anything you have not \
     committed yet, then end your turn.";

/// How long the loop keeps taking messages that are already queued, once
/// something has woken it.
///
/// A window, not a rate. Narration and settlement share one channel, so taking a
/// single message per pass made a settlement wait a whole pass for every envelope
/// queued ahead of it; this is how long the loop spends emptying the queue before
/// it decides what the batch means. Nothing about it says how often a pass
/// happens — [`CHANNEL_POLL`] and the paced intervals below say that.
const DRAIN_WINDOW: Duration = Duration::from_millis(25);

/// How long a teardown waits for the next envelope before looking at its
/// deadline again.
///
/// A dispatch that was asked to stop is read until it stops or its grace runs
/// out, and this is the granularity that grace is measured at.
const TEARDOWN_TICK: Duration = Duration::from_millis(25);

/// How long the loop waits before looking at the planner's channel again.
///
/// **Not a pass rate.** A look is two `stat` calls, and a pass runs only when one
/// of them moved, so a converged run costs the host two syscalls a fifth of a
/// second rather than a whole-state reconciliation.
///
/// What the interval bounds is how long an edit another process wrote waits to
/// be read. The contract states that as a second, and a fifth of it leaves the
/// pass it triggers the rest of the budget.
const CHANNEL_POLL: Duration = Duration::from_millis(200);

/// How often another run's ledger is re-read to answer a cross-DAG edge.
///
/// That ledger is written by a process this one does not control, which fixes how
/// **fresh** the answer must be rather than how often to fetch it. Half a second
/// sits inside the second the contract promises a consumer whose upstream settles
/// elsewhere. A graph naming no cross-DAG edge reads nothing at all.
pub(crate) const UPSTREAM_EVERY: Duration = Duration::from_millis(500);

/// The schema version a run result is written as.
///
/// `result.json` is a machine-read artifact this crate writes and **never reads
/// back**, so the number is a statement to its consumers rather than to a reader
/// here.
///
/// `5` is this document: `4` plus the nodes a `retry` superseded, each carrying
/// the replacement that took its place as
/// [`superseded_by`](NodeResult::superseded_by). Those nodes were in no earlier
/// version at all — a supersession takes the node out of the graph and the
/// document was built from the graph — so the number moves for what the document
/// now *holds* as much as for the key it added. `4` was `3` plus every node's
/// [`cause`](NodeResult::cause) and [`head`](NodeResult::head), the two a
/// settlement carries when a dispatch ended for a reason that is not the agent's
/// verdict on its task. `3` was one result per run, carrying no round and every
/// node's [`landing`](NodeResult::landing). `2` and `1` were the per-round
/// `round-NN/result.json` — `1` unversioned and saying only that a node had
/// settled, `2` where a landing was first recorded — and both named a round that
/// continuous execution does not have.
pub const RUN_RESULT_SCHEMA_VERSION: u32 = 5;

/// Read the version, refusing every number this build did not write.
///
/// One number rather than the range the landing's additive bump left, because
/// this shape is not additive over that one: a `2` names a round there is no
/// field for here, and read leniently it would be normalised into a run's result
/// that looks like every other — which is the shape of every defect this version
/// exists to make visible. A number *above* is refused for the reason it always
/// was: the document may state something this build has no field for. A document
/// carrying no key at all is a `1`, refused as the missing field it is. Every
/// refusal names the version found and the one this build reads.
fn readable_run_result_version<'de, D: serde::Deserializer<'de>>(
    reader: D,
) -> std::result::Result<u32, D::Error> {
    let found = u32::deserialize(reader)?;
    if found != RUN_RESULT_SCHEMA_VERSION {
        return Err(serde::de::Error::custom(format!(
            "run result schema_version {found}, and this build reads \
             {RUN_RESULT_SCHEMA_VERSION}"
        )));
    }
    Ok(found)
}

/// The run's recorded result, rewritten whenever the loop closes out.
///
/// One document per run rather than one per round: there are no rounds, and the
/// frontier the ledger records is the continuous one. `ok` is on the wire but
/// not on the type — it is `state == complete` and nothing else, so storing it
/// would let a result claim a failed run succeeded. It is derived on the way out
/// and re-derived on the way in, which is also what makes a hand-edited result
/// file impossible to disagree with itself. `schema_version` is on the wire and
/// not on the type for the same reason and one more: this crate writes exactly
/// one version, so a result it produces states that one rather than whichever it
/// happened to read.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(into = "RunResultWire", from = "RunResultWire")]
pub struct RunResult {
    /// The run.
    pub run_id: String,
    /// How the graph settled.
    pub state: GraphState,
    /// Every node's status, in the order the plan wrote them.
    pub nodes: Vec<NodeResult>,
}

impl RunResult {
    /// True only for `complete`, as the recorded result renders it.
    pub fn ok(&self) -> bool {
        self.state == GraphState::Complete
    }
}

/// The shape a run result is written and read as.
// llmlint: ignore-block[invalid_states_unrepresentable] `ok` and `schema_version` beside
// `state` are the wire's shape, not states this crate can hold. The type is private, its
// only constructor is the `From<RunResult>` below — which computes both — and the `From`
// back drops them, so a file claiming `state: failed, ok: true` is normalised rather than
// believed. Removing `ok` from the wire is a different change: consumers filter on it, so
// it would be its own breaking bump. Raise that with the planner who owns the contract.
// llmlint: ignore-block[changed_behavior_has_e2e] no invocation a user can type reads a
// run result back: this crate writes the document and every consumer of it is outside this
// repo, so the version rules below have no product path to be driven through. Held by this
// module's golden and version tests, against the same bytes a consumer parses.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RunResultWire {
    /// Required, and never defaulted: a document with no key is a `1`, which
    /// named a round this shape has no field for.
    #[serde(deserialize_with = "readable_run_result_version")]
    schema_version: u32,
    run_id: String,
    state: GraphState,
    ok: bool,
    nodes: Vec<NodeResult>,
} // llmlint: ignore-end[changed_behavior_has_e2e]

// llmlint: ignore-end[invalid_states_unrepresentable]

impl From<RunResult> for RunResultWire {
    fn from(result: RunResult) -> Self {
        Self {
            schema_version: RUN_RESULT_SCHEMA_VERSION,
            ok: result.ok(),
            run_id: result.run_id,
            state: result.state,
            nodes: result.nodes,
        }
    }
}

impl From<RunResultWire> for RunResult {
    fn from(wire: RunResultWire) -> Self {
        Self {
            run_id: wire.run_id,
            state: wire.state,
            nodes: wire.nodes,
        }
    }
}

/// One node's settlement, with its own evidence.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NodeResult {
    /// The node.
    pub id: String,
    /// How it settled.
    pub status: NodeStatus,
    /// The named outcome, when it had one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outcome: Option<String>,
    /// Whether the change this node published reached its base branch.
    ///
    /// The one field on this record that a `done` node's *status* does not
    /// already imply: a change request open for review settles the node exactly
    /// as a merge does, and a reader closing work on the status alone would
    /// close it on a change that reached nobody. Absent where there was no
    /// change of this node's to land — see [`Landing`].
    ///
    /// Omitted when absent rather than written as `null`, so a node with no
    /// change of its own reads as one making no claim — and a consumer branches
    /// on the key's presence instead of on a field that is there for every node
    /// and meaningless for most. This field is what
    /// [`RUN_RESULT_SCHEMA_VERSION`] `2` first recorded, and `3` carried into
    /// the run's own document.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub landing: Option<Landing>,
    /// What a ready human action asks for.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub action: Option<String>,
    /// What that action unblocks.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub unblocks: Vec<String>,
    /// The ready human references gating a blocked node.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub blocked_by: Vec<String>,
    /// The branch a lifecycle node left behind.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    /// Where a human reads the change it published.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub change_url: Option<String>,
    // llmlint: ignore-block[invalid_states_unrepresentable] both are the *wire* shape of
    // this document, which builds other than this one parse: `cause` is an open vocabulary
    // the harness below this crate owns and grows, so a newtype validating it here would
    // refuse a classification that layer added and report it as none at all — the value is
    // checked for what this crate does with it, at the boundary it enters, by
    // `is_a_classification`. `head` is the plain string every identifier in this crate is,
    // for the reason `crate::projection`'s `landing_commits` records, and is checked the
    // same way by `vcs::branch_head_in`. The sibling's `Sha` would put that library's type
    // on a document consumers parse without it.
    /// Why a dispatch that ended for a reason other than the agent's verdict
    /// ended, in the words its producer classified it with.
    ///
    /// Absent on every settlement that carries no classification, which is most
    /// of them: an agent that failed its own task was classified by nobody.
    /// [`RUN_RESULT_SCHEMA_VERSION`] `4` is what first recorded it, beside
    /// [`head`](Self::head).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cause: Option<String>,
    /// The commit the node's branch was left at, when `onevcs` recorded one.
    ///
    /// Beside [`branch`](Self::branch) rather than folded into it, because the
    /// two answer different questions: the branch is where the work is, and this
    /// is what is on it. Absent where nothing recorded a commit — a node that
    /// produced no branch at all, and one whose branch nothing committed to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub head: Option<String>,
    // llmlint: ignore-end[invalid_states_unrepresentable]
    // llmlint: ignore-block[invalid_states_unrepresentable] a node id is the plain string
    // every identifier on this record already is — `id` above all — for the reason the
    // block above states of `cause` and `head`: this is the *wire* shape of a document
    // builds other than this one parse. The value is not unchecked either: it is a
    // replacement id the reconciler validated against the live graph — non-blank, and not
    // already taken — before it committed the retry that produced it.
    /// The node that was retried in this one's place, when a `retry` superseded
    /// it.
    ///
    /// The one field on this record that says a node is **not** the run's to act
    /// on — see [`crate::projection::RunState::superseded`] for what it reads as
    /// without one. [`status`](Self::status) is `cancelled` beside it, which is
    /// what happened to the *dispatch*, while this is what happened to the
    /// *node*. Absent on every node nothing superseded, and omitted when absent
    /// so a consumer branches on the key rather than on a field that is there for
    /// every node and meaningless for most. This field is what
    /// [`RUN_RESULT_SCHEMA_VERSION`] `5` records.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub superseded_by: Option<String>,
    // llmlint: ignore-end[invalid_states_unrepresentable]
}

/// How one node settled, as its dispatch reports it.
///
// llmlint: ignore-block[invalid_states_unrepresentable] `outcome`, `branch`, `change_url`,
// `cause`, and `head` are optional strings because they are exactly what goes into the
// journal payload, and the journal is read by builds other than this one. An outcome enum
// here would make a record written by a newer build unreadable by an older one, which is
// the failure the schema-skipping rule elsewhere in this crate exists to prevent. `cause`
// is a string for one more reason: the word is the *harness's*, and a set declared here
// would refuse a classification that layer added and report it as none at all. `status` is
// the part that *is* narrowed, because scheduling depends on it.
#[derive(Debug, Clone, PartialEq)]
pub struct Settlement {
    /// The node.
    pub node: String,
    /// Its terminal status.
    pub status: NodeStatus,
    /// The named outcome, when it had one.
    pub outcome: Option<String>,
    /// Whether the change this node published reached its base branch.
    ///
    /// Narrowed like `status` and unlike the three strings above, because this
    /// one is a claim rather than a label: an unrecognised word here would have
    /// to be read as *some* landing, and both readings are a false report. The
    /// journal writes it as a word and [`Landing::parse`] reads an unknown one
    /// back as no observation at all.
    pub landing: Option<Landing>,
    /// The failure's own words.
    pub detail: Option<String>,
    /// The branch a lifecycle node left behind.
    pub branch: Option<String>,
    /// Where a human reads the change it published.
    pub change_url: Option<String>,
    /// Why a dispatch that ended for a reason other than the agent's verdict
    /// ended, as its producer classified it.
    ///
    /// The producer's own word, carried rather than re-vocabularised here: which
    /// classifications a harness draws is that layer's business, and a mapping in
    /// this one would rename a cause an operator has to look up in the harness's
    /// own documentation. Taken from the death the producer published where it
    /// published one — see [`MemberDeath`] — and read out of the failure's own
    /// sentence where it did not: see [`dispatch_death_cause`].
    pub cause: Option<String>,
    /// The commit the node's branch was left at, when `onevcs` recorded one.
    pub head: Option<String>,
    /// The declared steps this attempt finished, for a continuation to skip.
    pub completed_steps: Vec<String>,
}
// llmlint: ignore-end[invalid_states_unrepresentable]

impl Settlement {
    /// A node that settled without a dispatch.
    pub fn plain(node: &str, status: NodeStatus, outcome: Option<&str>) -> Self {
        Self {
            node: node.to_string(),
            status,
            outcome: outcome.map(str::to_string),
            // A settlement nothing published has nothing to say about landing.
            landing: None,
            detail: None,
            branch: None,
            change_url: None,
            cause: None,
            head: None,
            completed_steps: Vec::new(),
        }
    }
}

/// What a dispatch thread sends back to the single writer.
pub(crate) enum Message {
    /// One envelope, relayed from wherever the dispatch ran.
    Event(Box<Envelope>),
    /// A dispatch that produced nothing is being started again.
    ///
    /// Recorded as another `node-dispatched`, because that is what it is: the
    /// executor is being asked for the node a second time. The attempt number
    /// rides the payload, so a reader can tell a first try from a recovery.
    Redispatched(Box<Redispatch>),
    /// A cancellation reached a dispatch, or ran out of patience with one.
    Cancelling(Box<Cancelling>),
    /// A configured drafting dispatch produced no change request body.
    BodyNotDrafted(Box<UndraftedBody>),
    /// One acceptance criterion was compared against the branch its node is
    /// settling on.
    CriterionChecked(Box<CriterionChecked>),
    /// The dispatch settled.
    Settled(Box<Settlement>),
}

/// One acceptance criterion, read against the branch the node settled on.
///
/// Handed over rather than written where it is measured, for [`UndraftedBody`]'s
/// reason.
///
/// It carries no verdict, because it **is** no verdict: the node's settlement is
/// decided by its dispatches and its publication exactly as it was before this
/// check existed, and what a mismatch buys a reader is a finding beside that
/// settlement rather than a different one.
pub(crate) struct CriterionChecked {
    /// The node whose branch was read.
    ///
    /// A [`NodeRef`](crate::graph::NodeRef) and not a `String`, because this
    /// crosses a thread boundary: what arrives at the single writer is a value
    /// nothing here can check any more, so it arrives already being the identity
    /// of a node the graph carries rather than a field with a name in it.
    pub node: crate::graph::NodeRef,
    /// The criterion, and what it said the branch holds.
    pub check: crate::criteria::Checkable,
    /// What reading the branch answered.
    pub answer: crate::criteria::Answer,
}

/// A change request whose body a configured drafting dispatch did not produce.
///
/// The loop owns the pipeline stream and the sequence it is numbered in, so a
/// dispatch thread with something of its own to record hands over *what
/// happened* rather than composing an envelope beside that series — which is
/// what a relayed envelope is, and what a `seq` this side did not issue would
/// make a reader read as loss.
///
/// The fields and not a kind with a payload map beside it: this is the one of
/// this crate's own kinds a dispatch thread emits, and a kind selected
/// independently of the payload it is paired with is a mismatch nothing would
/// catch. The ending travels as the drafting side's own type for the same
/// reason — the wire name and the sentence are read off it here, so an ending
/// this build does not have is not a value this message can carry.
pub(crate) struct UndraftedBody {
    /// The node whose change request opened without one.
    pub node: String,
    /// Which ending it was.
    pub ending: crate::lifecycle::Undrafted,
}

/// One transition of a cancellation, on its way to the planner.
///
/// Sent rather than written, because the thread that cancels a dispatch is not
/// the run's single writer: the loop is, and it is what turns this into a
/// surface the planner reads.
pub(crate) struct Cancelling {
    /// The node whose dispatch it is.
    pub node: String,
    /// Which transition this is.
    pub phase: CancelPhase,
    /// What happened, in the words of whatever answered.
    pub detail: String,
}

/// The two transitions of a cancellation a supervisor has to be able to tell
/// apart.
///
/// The follow-up differs. A turn that stopped when it was asked ended on its own
/// terms and committed what it had; one the deadline reaped stopped wherever it
/// was, and whatever it had not committed is gone. A run that reported only "the
/// node was cancelled" left a supervisor unable to tell which had happened.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CancelPhase {
    /// The interrupt was asked for, and this is what the delivery said.
    Interrupted,
    /// The grace period expired and the teardown reaped the dispatch.
    Killed,
}

impl CancelPhase {
    /// The surface kind this transition is raised under.
    fn kind(self) -> &'static str {
        match self {
            Self::Interrupted => "dispatch-interrupted",
            Self::Killed => "dispatch-killed",
        }
    }
}

/// One re-asked dispatch, as the journal records it.
pub(crate) struct Redispatch {
    /// The node being asked again.
    pub node: String,
    /// Which attempt this is, counting from one — so never zero, which is an
    /// attempt nobody made.
    pub attempt: NonZeroU32,
    /// How many the budget allows, which is never zero either: a budget of zero
    /// would settle the node having never dispatched it.
    pub attempts: NonZeroU32,
    /// A bounded reason, as the failing attempt reported it.
    pub reason: String,
}

/// Everything a dispatch needs, resolved before it leaves the writer's thread.
struct Dispatch {
    node: Node,
    cancel: CancellationToken,
    started: Instant,
    /// When this dispatch last recorded anything evidencing progress, for the
    /// quiet-worker watch. Named for progress rather than for activity because
    /// a heartbeat is activity and does not move it: see
    /// [`projection::evidences_progress`].
    last_progress: Instant,
    /// Whether it has already been reported quiet in this quiet stretch. A
    /// worker that wakes up, works, and goes quiet again is reported again; one
    /// that simply stays quiet is not repeated.
    reported_quiet: bool,
    /// Where this dispatch's in-flight turn is addressed, once its stream has
    /// said. `None` until then, which is the same answer as a turn there is no
    /// lever for: a `context` note has nothing to be delivered into.
    control: Option<TurnAddress>,
}

impl Dispatch {
    /// This dispatch as an edit judging the node has to see it.
    fn live(&self) -> edits::LiveDispatch {
        edits::LiveDispatch {
            graph_run: self.control.as_ref().map(|at| at.run().to_string()),
            running_for_seconds: self.started.elapsed().as_secs(),
        }
    }
}

/// Take the run's ownership lock, or report who holds it.
///
/// Taken by the caller rather than by the loop, because a caller that is about
/// to *claim* the run — in the launch record, or by launching an observer for
/// it — has to lose the race first: writing its own pid there and then failing
/// on the lock would leave the record naming a process that is gone, and every
/// reader afterwards would call the run undriven while the driver that won was
/// still working on it.
pub fn claim(paths: &RunPaths) -> Result<OwnershipLock> {
    OwnershipLock::acquire(paths, "drive")
}

/// Drive one run's graph to settlement, in this process, under a lock the
/// caller already holds.
///
/// Returns the state the graph settled in, whose exit code the binary carries:
/// 0 for `complete`, 1 for `waiting` or `failed`. The loop returns when the
/// graph is terminal or when nothing can move without something arriving over
/// the channel — a decision point cleared while the loop is still running
/// resumes the subtree it held, without any external driver action.
pub fn drive_holding(paths: &RunPaths, lock: OwnershipLock) -> Result<GraphState> {
    let launch: LaunchRecord = ledger::read_json(&paths.launch())?;
    // llmlint: ignore-block[boundary_inputs_validated] graph-reference syntax and
    // contents are oneagentgraph's validation boundary. Here the ledger boundary
    // validates the launch schema and the one invariant onepipeline owns: a launch must
    // carry the nonempty reference resolved before any workspace existed.
    if launch.node_graph.is_empty() {
        return Err(Error::Invalid(format!(
            "launch record for run '{}' has no resolved node graph",
            paths.run
        )));
    }
    // llmlint: ignore-end[boundary_inputs_validated]
    let mut journal = Journal::open(paths);
    let mut state = projection::fold(&journal::read(&paths.journal()));
    report_unreadable_records(paths, &state);

    let outcome = converge(paths, &mut journal, &mut state, &launch)?;
    record_result(paths, &state, outcome)?;
    lock.release();
    Ok(outcome)
}

/// The reconcile loop: converge the actual frontier toward the desired graph.
fn converge(
    paths: &RunPaths,
    journal: &mut Journal,
    state: &mut RunState,
    launch: &LaunchRecord,
) -> Result<GraphState> {
    // Resolving write-back is deliberately best effort. A run launched by an older build
    // may name no project, and a sibling unavailable after launch cannot become a run
    // failure. The worker never feeds anything it reads back into this loop.
    // llmlint: ignore-block[changed_behavior_has_e2e] The real-store outage journey covers
    // every failure after this optional worker exists. Making resolution itself fail only
    // after `start` already resolved the same executable requires replacing or deleting the
    // real sibling between two adjacent calls; that is a host sabotage fixture, not a user
    // journey, and the compatibility behavior here is intentionally the absence of a writer.
    let writeback = crate::taskgraph::Store::resolve()
        .ok()
        .and_then(|store| crate::writeback::Writeback::start(store.binary(), paths, launch));
    // llmlint: ignore-end[changed_behavior_has_e2e]
    let channel = ChannelState::new(paths);
    let rules = executor_rules()?;
    let (tx, rx): (Sender<Message>, Receiver<Message>) = mpsc::channel();
    let mut in_flight: BTreeMap<String, Dispatch> = BTreeMap::new();
    let stall_after = Duration::from_secs(stall_after_seconds());
    let mut upstreams = crate::crossdag::Observer::of_run(paths, state);
    // What each node's dependencies have released, and what this run has already
    // said about it. It asks `onevcs` on a thread of its own: an automated
    // target's answer is a probe, and a slow one asked inline would stall the
    // loop every other node in the run depends on.
    let mut releases = crate::release::Watch::of_run(paths);
    // What the loop has already said out loud, so each fact is announced once
    // and again only when it becomes true again.
    let mut announced_ready: BTreeSet<String> = BTreeSet::new();
    // Seeded from the journal, not started empty: a decision outlives the
    // driver that reported it, and a fresh loop that did not know what its
    // predecessor was holding would release it without saying so.
    let mut held: BTreeMap<DecisionRef, Decision> = state
        .decisions_pending
        .iter()
        .map(|(reference, pending)| {
            (
                DecisionRef::of_wire(reference),
                Decision {
                    reference: DecisionRef::of_wire(reference),
                    kind: pending.kind.clone(),
                    unblocks: pending.unblocks.clone(),
                },
            )
        })
        .collect();

    // What the loop is holding back, and why. Diffed pass to pass exactly as the
    // decisions above are, so a hold is written when it begins, again when what
    // it is held by changes, and once when it clears — and seeded from the
    // journal for the same reason they are: a hold outlives the driver that
    // reported it, so a fresh one that started empty would restate every span
    // already open and never close one that cleared while nothing was driving.
    let mut holding: BTreeMap<String, Vec<HoldReason>> = state
        .holds
        .iter()
        .filter_map(|(node, reasons)| {
            let read: Option<Vec<HoldReason>> =
                reasons.iter().map(HoldReason::of_payload).collect();
            read.map(|reasons| (node.clone(), reasons))
        })
        .collect();
    // The graph's derived statuses, cached so the fixpoint over every node and
    // edge is paid at most once per change to the folded state rather than once
    // per caller that wants it.
    let mut derived: Option<BTreeMap<String, NodeStatus>> = None;
    // When each piece of paced work was last done. `None` is due now, which is
    // what makes the first pass do all of it.
    let mut read_upstreams: Option<Instant> = None;
    // Assigned by every pass before the wait reads it: taking the release watch
    // up is part of what a pass *is*, so there is no "not yet" for it to be in.
    let mut took_up_releases;
    // Whether the board has been told what the run looks like now. Publishing
    // folds the run's whole journal, so it is paid once per change to what the
    // snapshot is made of — the number of times the board can actually be
    // behind — rather than once per pass.
    let mut unpublished = true;
    // What the channel looked like when this loop last read it.
    let mut channel_seen = channel.fingerprint();
    // And what every upstream ledger this graph's edges are answered by looked
    // like. Compared rather than re-read: see [`crossdag::Observer::marks`].
    let mut upstream_seen = upstreams.marks(&state.graph);
    let mut upstream_looked = Instant::now();

    loop {
        crate::loopstats::pass();
        // Whether this pass moved the run's own state, which is the one change
        // the wait below cannot be woken by: what a pass settles or applies itself
        // has no dispatch left to report it and writes nothing to the channel, so
        // what it readies is the next pass's to start. Each of the three below
        // reports `true` only for work it *consumed*, which bounds this at one
        // extra pass per change and leaves a converged run running none.
        let mut moved = false;
        if reconcile_edits(paths, journal, state, &channel, launch, &mut in_flight)? {
            derived = None;
            unpublished = true;
            moved = true;
        }

        // Another run's ledger is the only thing that can answer a cross-DAG
        // edge, and it is written by a process this one does not control — which
        // is a statement about how **fresh** the answer has to be and not about
        // how often to go and get it. So it is re-read on [`UPSTREAM_EVERY`]
        // rather than on every pass, and a graph naming no cross-DAG edge never
        // reads it at all. This is also where an upstream that moved past what a
        // consumer recorded is noticed.
        let has_upstreams = !crate::crossdag::edges(&state.graph).is_empty();
        if has_upstreams && due(read_upstreams, UPSTREAM_EVERY) {
            read_upstreams = Some(Instant::now());
            let resolved = upstreams.resolve(&state.graph, paths, journal)?;
            if resolved != state.cross_dag {
                state.cross_dag = resolved;
                derived = None;
                unpublished = true;
            }
        }

        let statuses = statuses_of(&mut derived, state);
        announce_ready(paths, journal, &statuses, &mut announced_ready)?;
        // The decision points holding subtrees back, and the nodes they hold.
        // Diffed against the last pass, so each one is reported when it begins
        // holding dependents back and again when it releases them.
        let decisions = decisions_now(state, &statuses, &channel);
        report_decisions(paths, journal, &decisions, &mut held)?;
        let mut paused = paused_by(&decisions);

        // The releases this pass cares about: every node ready to start, where a
        // `published` hold applies and where a `fast` node's reference block is
        // composed, and every node still running, where an arrival note is
        // delivered.
        //
        // Taken up on this pass either because something about the run changed —
        // a node became ready, a dispatch settled — or because the interval an
        // answer may sit unread for came due. A run with no out-of-repository
        // dependency and nothing landed in a repository that releases pays
        // neither: it asks nothing and takes nothing up, ever.
        let watching = crate::release::watching(
            state,
            &statuses,
            &in_flight.keys().cloned().collect::<BTreeSet<String>>(),
        );
        releases.refresh(paths, state, &watching);
        let held_for_release = releases.held(&watching);
        releases.report(paths, journal, &held_for_release, &watching)?;
        // What the sibling recorded about the releases carrying this run's own
        // landed work. Not part of the wait: a release is reported whether or
        // not anything is waiting on it, because the node whose work it carries
        // has settled and its own follow ended with the session it watched.
        releases.relay_releases(
            paths,
            journal,
            state,
            &statuses,
            launch.filters.vcs.as_ref(),
        )?;
        took_up_releases = Some(Instant::now());
        // One hold, beside the decision points rather than in place of them: a
        // node a person is holding and a node a release is holding are both nodes
        // this pass does not start, and neither shortens the other's wait. Read
        // twice: as what the pass may not start, and — with the dependency ids
        // each is waiting on — as what to say about why.
        let awaiting_release: BTreeMap<String, Vec<String>> = held_for_release
            .iter()
            .map(|node| (node.clone(), releases.awaited_deps(node)))
            .collect();
        paused.extend(held_for_release);
        if adopt_releases(paths, journal, state, &statuses, &mut releases, &in_flight)? {
            derived = None;
            unpublished = true;
            moved = true;
        }

        // Start what became actionable *before* asking whether the run is over.
        // A ready human action derives as `waiting`, which is a settled status —
        // so a check that ran first would call the graph terminal and leave that
        // settlement unrecorded, with nothing for a later `attest` to validate
        // against.
        let statuses = statuses_of(&mut derived, state);
        if start_ready(
            paths,
            journal,
            state,
            &statuses,
            &rules,
            launch,
            &tx,
            &mut in_flight,
            &paused,
            &releases,
        )? {
            derived = None;
            unpublished = true;
            moved = true;
        }

        // Why every node the loop is not running and has not settled is not
        // running, after the dispatches this pass started: a node the frontier
        // moved past is not held by what held it a moment ago.
        let statuses = statuses_of(&mut derived, state);
        if unpublished {
            if let Some(writeback) = &writeback {
                writeback.publish(paths, launch, state, &statuses);
            }
            unpublished = false;
        }
        report_holds(
            paths,
            journal,
            &holds_now(state, &statuses, &in_flight, &decisions, &awaiting_release),
            &mut holding,
        )?;

        if let Some(writeback) = &writeback {
            report_unprojected(paths, journal, writeback)?;
        }

        if in_flight.is_empty() {
            // Nothing is running and nothing became ready, so no further
            // message can arrive: the graph is as converged as it will get.
            if graph::is_terminal(&statuses) {
                break;
            }
            // A node that is neither settled nor startable is gated by
            // something only an edit or an attestation can clear, and both
            // arrive through the channel.
            if !any_node_can_still_move(&statuses) {
                break;
            }
        }

        // Nothing more to do until something happens — unless this pass is what
        // happened, in which case the next one is due now. The longest this loop
        // may go without a pass is otherwise stated here rather than inside the
        // wait: the two paced reads, and the earliest stall threshold. Each is
        // `Duration::MAX` where this run can never need it — a graph naming no
        // cross-DAG edge, a run with no release business, nothing in flight — so a
        // converged driver waits on the channel alone.
        let next = [
            if has_upstreams {
                until_due(read_upstreams, UPSTREAM_EVERY)
            } else {
                Duration::MAX
            },
            if releases.names_a_release_dependency() || releases.relays_anything(state) {
                until_due(took_up_releases, releases.take_up_every())
            } else {
                Duration::MAX
            },
            next_quiet(&in_flight, stall_after),
        ];
        let deadline = if moved {
            Duration::ZERO
        } else {
            next.into_iter().min().unwrap_or(Duration::MAX)
        };
        // Scoped, because the two things it reads are things the pass writes:
        // the closure's borrows end with the wait, before the messages it hands
        // back are applied.
        let arrived = {
            // What the loop can be woken by that nothing here writes: a release
            // probe that answered, a projection that failed, and an upstream
            // ledger that grew. Each is read the cheap way — two queues this
            // thread already owns the other end of, and a `stat` — so a wake that
            // finds none of them goes straight back to waiting.
            let mut outside = || {
                if releases.take_up_answers() {
                    return true;
                }
                if writeback
                    .as_ref()
                    .is_some_and(crate::writeback::Writeback::has_unprojected)
                {
                    return true;
                }
                if !has_upstreams || upstream_looked.elapsed() < UPSTREAM_EVERY {
                    return false;
                }
                upstream_looked = Instant::now();
                let now = upstreams.marks(&state.graph);
                if now == upstream_seen {
                    return false;
                }
                upstream_seen = now;
                true
            };
            wait_for_work(
                paths,
                &rx,
                &channel,
                &mut channel_seen,
                deadline,
                &mut outside,
            )?
        };
        let Some(arrived) = arrived else {
            break;
        };

        // Everything that had **already arrived**, applied in this one pass:
        // narration and settlement share the channel, so taking one message a
        // pass would make a settlement wait a whole pass for every envelope
        // queued ahead of it.
        //
        // llmlint: ignore-block[changed_behavior_has_e2e] the batch is applied in arrival
        // order, so no journal a journey can read differs by a record; what differs is how
        // long a settlement waits, which is proportional to what a pass costs on the host.
        // Every journey drives this code — it is the only path a message reaches the
        // journal by — and one written to prove the wait passed against both builds.
        for message in arrived {
            // llmlint: ignore-end[changed_behavior_has_e2e]
            match message {
                Message::Event(envelope) => {
                    if let Some(node) = envelope.labels.node.clone() {
                        if let Some(dispatch) = in_flight.get_mut(&node) {
                            // Only an envelope evidencing progress moves the stall
                            // clock. A heartbeat says the process is alive, which is
                            // a different question with its own deadline one layer
                            // down — and a stall watch it reset could never fire for
                            // the wedged-but-alive turn it exists to catch.
                            if projection::evidences_progress(&envelope) {
                                dispatch.last_progress = Instant::now();
                                dispatch.reported_quiet = false;
                            }
                            // Addressing is not progress: a turn a heartbeat names
                            // is still the turn a `context` note is delivered into.
                            if let Some(address) = addressed_by(&envelope) {
                                dispatch.control = Some(address);
                            }
                        }
                    }
                    journal.relay(&envelope)?;
                }
                // A dispatch asked again is a dispatch started again, and it reaches
                // the run's own record as one rather than only a log.
                Message::Redispatched(again) => journal.emit(
                    journal::PipelineKind::NodeDispatched,
                    journal::labels(&paths.run, Some(&again.node)),
                    journal::payload(&[
                        ("attempt", json!(again.attempt)),
                        ("attempts", json!(again.attempts)),
                        ("reason", json!(bounded(&again.reason))),
                    ]),
                )?,
                // A cancellation that reached a live turn, and one that ran out of
                // patience and reaped it. Surfaced rather than only journalled
                // because a planner reading its own updates is who decides what to
                // do next, and what to do next is not the same for the two.
                Message::Cancelling(step) => raise(paths, journal, cancelling_surface(&step))?,
                // Emitted rather than relayed: it is this crate's own kind, so it
                // belongs in this crate's own stream, numbered by the writer that
                // owns it.
                Message::BodyNotDrafted(undrafted) => journal.emit(
                    journal::PipelineKind::BodyNotDrafted,
                    journal::labels(&paths.run, Some(&undrafted.node)),
                    journal::payload(&[
                        ("ending", json!(undrafted.ending.ending())),
                        ("detail", json!(undrafted.ending.why())),
                    ]),
                )?,
                Message::CriterionChecked(checked) => {
                    journal.emit(
                        journal::PipelineKind::CriterionChecked,
                        journal::labels(&paths.run, Some(checked.node.as_str())),
                        criterion_payload(&checked),
                    )?;
                    // A mismatch is reported *beside* the settlement and never
                    // as part of it: the node settles on its dispatches and its
                    // publication exactly as it would have, and a manager reads
                    // this and decides. A match and an unread answer are on the
                    // run's record above and raise nothing — a tier that
                    // surfaced every comparison it made is one a reader learns
                    // to skim.
                    if let crate::criteria::Answer::Mismatch { holds } = &checked.answer {
                        raise(paths, journal, criterion_finding(&checked, holds))?;
                    }
                }
                Message::Settled(settlement) => {
                    in_flight.remove(&settlement.node);
                    settle(paths, journal, &settlement)?;
                    *state = projection::fold(&journal::read(&paths.journal()));
                    // A node that settled may have readied its dependents, and a
                    // node that is ready again — a requeue, a retry — is announced
                    // again. `announce_ready` retains against the frontier at the
                    // top of the next pass, which is that same fact asked once
                    // rather than once per settlement in a batch.
                    derived = None;
                    unpublished = true;
                }
            }
        }

        // A dispatch that has recorded nothing for long enough is reported, and
        // the wait above ends when the earliest of those thresholds comes due —
        // so this is asked when it can have an answer rather than forty times a
        // second when it cannot.
        watch_for_quiet(paths, journal, stall_after, &mut in_flight)?;
    }

    // llmlint: ignore-block[changed_behavior_has_e2e] the real-store journey drives the
    // terminal projection failing and the surface reaching the planner before the run
    // settles. What it cannot drive is a projection *slow* enough to outlast this bounded
    // window without failing: that needs the real sibling suspended mid-command, which is
    // host-level process control rather than an input either CLI exposes, and the window
    // itself is the best-effort boundary the contract already fixes — a store that has not
    // answered has said nothing to report.
    // Every node's status as the teardown leaves it, which is not the same as
    // every node being settled: this also runs when the channel disconnected
    // under a run that still has pending and blocked nodes.
    let final_statuses = statuses_of(&mut derived, state);
    crate::loopstats::flush(paths)?;
    if let Some(writeback) = &writeback {
        writeback.publish(paths, launch, state, &final_statuses);
        writeback.wait_briefly();
        // The last thing this loop does, and the reason it is here rather than
        // only at the top: a run whose *terminal* projection failed is exactly
        // the run that settles and is read as the record of what happened, so
        // the failure has to reach the planner before this driver stops.
        report_unprojected(paths, journal, writeback)?;
    }
    // llmlint: ignore-end[changed_behavior_has_e2e]
    Ok(graph::state_of(&final_statuses))
}

/// `None` is due now, which is what makes the first pass do everything once.
pub(crate) fn due(last: Option<Instant>, every: Duration) -> bool {
    last.is_none_or(|last| last.elapsed() >= every)
}

fn until_due(last: Option<Instant>, every: Duration) -> Duration {
    last.map_or(Duration::ZERO, |last| every.saturating_sub(last.elapsed()))
}

/// How long until the earliest in-flight dispatch could be reported quiet.
///
/// [`Duration::MAX`] where nothing is in flight or every dispatch has already
/// been reported: there is then no deadline of the loop's own, so it waits on
/// the channel alone.
fn next_quiet(in_flight: &BTreeMap<String, Dispatch>, stall_after: Duration) -> Duration {
    in_flight
        .values()
        .filter(|dispatch| !dispatch.reported_quiet)
        .map(|dispatch| stall_after.saturating_sub(dispatch.last_progress.elapsed()))
        .min()
        .unwrap_or(Duration::MAX)
}

/// The graph's derived statuses, computed once per change to the folded state.
///
/// Deriving is a fixpoint over every node and every edge of the graph, and the
/// loop wanted the answer in four places a pass. Cached behind the one thing
/// that can change it — a fold — so what the run pays for is what it recorded
/// rather than how often the loop looked.
fn statuses_of(
    cache: &mut Option<BTreeMap<String, NodeStatus>>,
    state: &RunState,
) -> BTreeMap<String, NodeStatus> {
    cache.get_or_insert_with(|| state.statuses()).clone()
}

/// Wait until there is a reason to reconcile, and hand back what arrived.
///
/// The four things that can be one: a dispatch thread said something, the
/// planner's channel moved, `outside` says state this run does not write has
/// moved — an upstream ledger, an answered release probe — or `deadline`, the
/// longest this loop may go without a pass, came due. A wake that finds none of
/// them **goes back to waiting**, which is what makes a converged run run no
/// passes at all rather than one per look.
///
/// `Ok(None)` when every dispatch thread has gone and no message can ever arrive
/// again, which is the loop's own `Disconnected`. `Err` when a driver that was
/// asked to report what its loop did could not write the report.
fn wait_for_work(
    paths: &RunPaths,
    rx: &Receiver<Message>,
    channel: &ChannelState,
    seen: &mut crate::channel::Fingerprint,
    deadline: Duration,
    outside: &mut dyn FnMut() -> bool,
) -> Result<Option<Vec<Message>>> {
    let waiting_since = Instant::now();
    loop {
        // The one place a measured driver reports what its loop has done: every
        // wait rather than every pass, because the counts a wait is long enough
        // to change are the ones another thread keeps — a release probe answers
        // while this loop sits here. A write that fails is handed back: the only
        // way to reach it is a host that asked for the counts, and it is owed an
        // answer rather than a file that never appears.
        crate::loopstats::flush(paths)?;
        let left = deadline.saturating_sub(waiting_since.elapsed());
        match rx.recv_timeout(CHANNEL_POLL.min(left)) {
            Ok(message) => {
                // Everything else already queued, applied in the one pass this
                // message is about to cause. Narration and settlement share this
                // channel, so taking one message a pass made a settlement wait a
                // whole pass for every envelope queued ahead of it.
                let mut batch = vec![message];
                let drain_started = Instant::now();
                while drain_started.elapsed() < DRAIN_WINDOW {
                    // One arm for both refusals: nothing is queued, or nothing
                    // ever will be again. The second is answered one pass later,
                    // which is where it has always been answered.
                    let Ok(message) = rx.try_recv() else {
                        break;
                    };
                    batch.push(message);
                }
                return Ok(Some(batch));
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => return Ok(None),
        }
        let now = channel.fingerprint();
        if now != *seen {
            *seen = now;
            return Ok(Some(Vec::new()));
        }
        if outside() {
            return Ok(Some(Vec::new()));
        }
        if waiting_since.elapsed() >= deadline {
            return Ok(Some(Vec::new()));
        }
    }
}

/// One reason the loop is not running a node it has not settled.
///
/// **They compose.** A node can be held by more than one at once, so a hold
/// carries one entry per reason in one record rather than one record per reason:
/// "behind three running nodes", "a dependency has not settled", and both at
/// once are three different answers, told apart from the `reasons` array alone.
///
/// **Two of the four are reported elsewhere, and this does not restate them.**
/// [`DecisionPending`](journal::PipelineKind::DecisionPending) and
/// [`ReleaseWait`](journal::PipelineKind::ReleaseWait) stay authoritative for
/// what that decision and that wait *are*; the entries here name only which one
/// holds the node. This record is authoritative for the hold itself.
#[derive(Debug, Clone, PartialEq, Eq)]
enum HoldReason {
    /// Its dependencies have not all settled `done`.
    ///
    /// `blocking` is what it is waiting for, as the graph names it — a node id,
    /// or a whole `run:<run>#<node>` reference for a dependency in another run.
    /// This is the shrinking set a reader watches: a node behind three running
    /// dependencies is held first by all three, then by the two that are left,
    /// then by the last, and then it dispatches.
    Dependencies { blocking: Vec<String> },
    /// It could dispatch, and the run has reached its concurrency.
    Concurrency {
        /// The dispatches in flight ahead of it.
        ahead: Vec<String>,
        /// The run's concurrency.
        limit: usize,
    },
    /// A decision point is holding the subtree it is in.
    ///
    /// The reference alone, and the type that already models one: a decision is
    /// cleared by an `attest` or by a reply, and which of the two it is has to be
    /// readable off the reference rather than guessed from a string. What that
    /// decision *is* stays on `decision-pending`.
    Decision { reference: DecisionRef },
    /// It adopts published releases, and not all of them have happened.
    ///
    /// The dependencies awaited, by id. What each wait is stays on `release-wait`.
    Release { awaiting: Vec<String> },
}

impl HoldReason {
    fn payload(&self) -> Value {
        match self {
            Self::Dependencies { blocking } => {
                json!({ "kind": "dependencies", "blocking": blocking })
            }
            Self::Concurrency { ahead, limit } => {
                json!({ "kind": "concurrency", "ahead": ahead, "limit": limit })
            }
            Self::Decision { reference } => {
                json!({ "kind": "decision", "reference": reference.as_wire() })
            }
            Self::Release { awaiting } => json!({ "kind": "release", "awaiting": awaiting }),
        }
    }

    /// One entry a `node-held` record carried, read back.
    ///
    /// `None` for an entry this build cannot read whole, which is what a reason a
    /// later build added looks like from here. The caller then treats the whole
    /// hold as one it does not know about and states its own, which is the safe
    /// direction: a hold restated is a duplicate span, and a hold silently taken
    /// as understood is one whose release is never reported.
    fn of_payload(entry: &Value) -> Option<Self> {
        let ids = |key: &str| -> Option<Vec<String>> {
            entry
                .get(key)?
                .as_array()?
                .iter()
                .map(|id| id.as_str().map(str::to_string))
                .collect()
        };
        match entry.get("kind")?.as_str()? {
            "dependencies" => Some(Self::Dependencies {
                blocking: ids("blocking")?,
            }),
            "concurrency" => Some(Self::Concurrency {
                ahead: ids("ahead")?,
                limit: usize::try_from(entry.get("limit")?.as_u64()?).ok()?,
            }),
            "decision" => Some(Self::Decision {
                reference: DecisionRef::of_wire(entry.get("reference")?.as_str()?),
            }),
            "release" => Some(Self::Release {
                awaiting: ids("awaiting")?,
            }),
            _ => None,
        }
    }
}

/// Every node the loop is not running and has not settled, and what is holding
/// it.
///
/// **Not narrowed to what the graph calls ready.** A node whose dependencies have
/// not settled is as much a queued span as one the run's concurrency is holding,
/// and a reader opening a timeline to find out why their run has not started
/// wants both — so the subject is any node that is neither in flight nor at an
/// outcome, whatever status the graph derives for it.
///
/// A node **no** stated reason is holding is absent, not present with an empty
/// array: a node that is dispatchable and merely waiting for this pass to reach
/// it is held by nothing, and so is a human action waiting on the person who has
/// to take it — that node is waiting on the world rather than on this loop.
///
/// Called after the pass's dispatches, so `ahead` is what is really in flight and
/// a node that just started is not reported as held by the run it just joined.
fn holds_now(
    state: &RunState,
    statuses: &BTreeMap<String, NodeStatus>,
    in_flight: &BTreeMap<String, Dispatch>,
    decisions: &BTreeMap<DecisionRef, Decision>,
    awaiting_release: &BTreeMap<String, Vec<String>>,
) -> BTreeMap<String, Vec<HoldReason>> {
    let concurrency = state.graph.concurrency as usize;
    let ahead: Vec<String> = in_flight.keys().cloned().collect();
    let mut holds: BTreeMap<String, Vec<HoldReason>> = BTreeMap::new();
    for node in state.graph.iter() {
        if in_flight.contains_key(&node.id) {
            continue;
        }
        // `Waiting` and `Parked` are left out with the outcomes on purpose. A
        // waiting human action is held by the person who has to take it, and a
        // parked node by the planner that parked it; neither is this loop
        // declining to run something, and neither is one of the four reasons.
        let status = statuses
            .get(&node.id)
            .copied()
            .unwrap_or(NodeStatus::Pending);
        if !matches!(
            status,
            NodeStatus::Pending
                | NodeStatus::Ready
                | NodeStatus::Blocked
                | NodeStatus::CompleteDraft
        ) {
            continue;
        }
        let mut reasons: Vec<HoldReason> = Vec::new();
        let blocking = unsettled_deps(state, statuses, node);
        if !blocking.is_empty() {
            reasons.push(HoldReason::Dependencies { blocking });
        }
        if status == NodeStatus::Ready
            && node.kind != NodeKind::Human
            && in_flight.len() >= concurrency
        {
            reasons.push(HoldReason::Concurrency {
                ahead: ahead.clone(),
                limit: concurrency,
            });
        }
        // One entry per decision, because two decisions holding one node are two
        // reasons it is not running and clearing either one leaves the other.
        for decision in decisions.values() {
            if decision.unblocks.contains(&node.id) {
                reasons.push(HoldReason::Decision {
                    reference: decision.reference.clone(),
                });
            }
        }
        if let Some(awaiting) = awaiting_release.get(&node.id) {
            reasons.push(HoldReason::Release {
                awaiting: awaiting.clone(),
            });
        }
        if !reasons.is_empty() {
            holds.insert(node.id.clone(), reasons);
        }
    }
    holds
}

/// The dependencies of one node that have not settled `done`, as the graph
/// names them.
///
/// A dependency the graph no longer holds was detached by a `drop` and is not
/// holding anything, which is the same reading [`graph::derive`] takes of it.
fn unsettled_deps(
    state: &RunState,
    statuses: &BTreeMap<String, NodeStatus>,
    node: &Node,
) -> Vec<String> {
    node.deps
        .iter()
        .filter(|dep| {
            let status = if crate::crossdag::is_reference(dep) {
                state.cross_dag.get(*dep).copied()
            } else if state.graph.contains(dep) {
                statuses.get(*dep).copied()
            } else {
                return false;
            };
            status != Some(NodeStatus::Done)
        })
        .cloned()
        .collect()
}

/// Report every hold that began, every one that changed, and every one that
/// cleared.
///
/// **Transitions only.** The loop's floor is a wait rather than a rate, but even
/// one record a pass would be one per settlement and one per edit for every node
/// standing still — so this diffs against what it said last, exactly as
/// [`report_decisions`] and [`announce_ready`] do, and a pass on which a node is
/// held by what it was held by before says nothing at all.
fn report_holds(
    paths: &RunPaths,
    journal: &mut Journal,
    holds: &BTreeMap<String, Vec<HoldReason>>,
    reported: &mut BTreeMap<String, Vec<HoldReason>>,
) -> Result<()> {
    for (node, reasons) in holds {
        if reported.get(node) == Some(reasons) {
            continue;
        }
        journal.emit(
            journal::PipelineKind::NodeHeld,
            journal::labels(&paths.run, Some(node)),
            journal::payload(&[(
                "reasons",
                Value::Array(reasons.iter().map(HoldReason::payload).collect()),
            )]),
        )?;
    }
    let cleared: Vec<(String, Vec<HoldReason>)> = reported
        .iter()
        .filter(|(node, _)| !holds.contains_key(*node))
        .map(|(node, reasons)| (node.clone(), reasons.clone()))
        .collect();
    for (node, released) in cleared {
        journal.emit(
            journal::PipelineKind::NodeUnheld,
            journal::labels(&paths.run, Some(&node)),
            journal::payload(&[(
                "released",
                Value::Array(released.iter().map(HoldReason::payload).collect()),
            )]),
        )?;
    }
    *reported = holds.clone();
    Ok(())
}

/// Say so when the graph this loop is about to converge was folded from a
/// journal it could not read whole.
///
/// A record this build cannot read might have been an authoritative graph
/// mutation — a `drop` that removed a node this loop is about to dispatch — so a
/// driver that meets one reports rather than quietly executing a graph it knows
/// is incomplete. It still drives: refusing would leave the run with nothing
/// driving it, which is strictly worse than driving it with the operator told.
fn report_unreadable_records(paths: &RunPaths, state: &RunState) {
    if state.strict && !journal::has_unreadable_lines(&paths.journal()) {
        return;
    }
    eprintln!(
        "onepipeline: run '{}' has a journal record this build cannot read; the graph \
         it is driving may be missing a committed edit.",
        paths.run
    );
}

/// One decision point: a blocking surface, and the dependents it holds back.
///
/// Two things are decision points, and they are the only things that pause
/// anything. A node waiting on a person is one — a `kind: human` node, or a
/// lifecycle node held at a human *step*, which settles the same way and is
/// cleared by the same `attest` — and its dependents wait with it. A
/// **blocking** planner surface is the other, and it holds whatever depends on
/// the node that raised it. A non-blocking surface is a report and holds
/// nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Decision {
    /// What clears it.
    reference: DecisionRef,
    /// What kind of decision it is, in the vocabulary its raiser used.
    kind: String,
    /// The nodes it is holding back, in graph order.
    unblocks: Vec<String>,
}

/// What clears one decision point.
///
/// The two are answered by different people through different verbs — a person
/// attests the action, a planner replies to the surface — and the reference a
/// reader sees has to say which. Spelled as the alternatives rather than as a
/// string, so a surface reference can only be a surface's own id and a node
/// reference can only be a node's.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum DecisionRef {
    /// A node waiting on a person, cleared by `attest`.
    ///
    /// Named for what clears it rather than for the node's kind, because two
    /// shapes reach it: a `kind: human` node, and a lifecycle node whose
    /// workstream stopped at a human step. Both settle `waiting`, both are
    /// attested by the reference below, and neither is a thing the harness may
    /// infer happened.
    Attestation(String),
    /// A blocking surface, cleared by the reply that answers it.
    Surface(u64),
}

impl DecisionRef {
    /// The reference as the journal and the node label spell it.
    fn as_wire(&self) -> String {
        match self {
            Self::Attestation(node) => node.clone(),
            Self::Surface(id) => format!("surface:{id}"),
        }
    }

    /// The reference a journal record spelled, read back.
    ///
    /// A node id cannot contain the separator — `graph::validate` refuses one —
    /// so the two spellings cannot be confused for each other.
    fn of_wire(reference: &str) -> Self {
        reference
            .strip_prefix("surface:")
            .and_then(|id| id.parse().ok())
            .map_or_else(|| Self::Attestation(reference.to_string()), Self::Surface)
    }
}

/// The decision points outstanding right now.
///
/// The node half comes from the derived statuses, and the surface half from the
/// **channel queue** rather than from the journal: a surface is outstanding
/// until somebody answers it, and the queue is the only thing that knows that.
/// A surface waiting to be read counts exactly as one already read and
/// unanswered does — an unread question is not an answered one.
fn decisions_now(
    state: &RunState,
    statuses: &BTreeMap<String, NodeStatus>,
    channel: &ChannelState,
) -> BTreeMap<DecisionRef, Decision> {
    let mut decisions = BTreeMap::new();
    for (id, status) in statuses {
        if *status != NodeStatus::Waiting {
            continue;
        }
        decisions.insert(
            DecisionRef::Attestation(id.clone()),
            Decision {
                reference: DecisionRef::Attestation(id.clone()),
                kind: "attestation".to_string(),
                unblocks: descendants(&state.graph, std::slice::from_ref(id)),
            },
        );
    }
    let queue = channel.queue();
    for surface in queue.waiting.iter().chain(queue.pending.iter()) {
        if !surface.blocking {
            continue;
        }
        let reference = DecisionRef::Surface(surface.id);
        let unblocks = surface
            .workstream
            .clone()
            .map(|node| descendants(&state.graph, std::slice::from_ref(&node)))
            .unwrap_or_default();
        decisions.insert(
            reference.clone(),
            Decision {
                reference,
                kind: surface.kind.clone(),
                unblocks,
            },
        );
    }
    decisions
}

/// Every node reachable downstream of `roots`, excluding the roots themselves.
fn descendants(graph: &Graph, roots: &[String]) -> Vec<String> {
    let mut seen: BTreeSet<String> = BTreeSet::new();
    let mut pending: Vec<String> = roots.to_vec();
    while let Some(current) = pending.pop() {
        for dependent in graph.dependents_of(&current) {
            if seen.insert(dependent.clone()) {
                pending.push(dependent);
            }
        }
    }
    graph
        .ids()
        .filter(|id| seen.contains(*id))
        .cloned()
        .collect()
}

/// Report every decision that began holding dependents back, and every one that
/// released them.
fn report_decisions(
    paths: &RunPaths,
    journal: &mut Journal,
    decisions: &BTreeMap<DecisionRef, Decision>,
    held: &mut BTreeMap<DecisionRef, Decision>,
) -> Result<()> {
    for (reference, decision) in decisions {
        if held.get(reference) == Some(decision) {
            continue;
        }
        journal.emit(
            journal::PipelineKind::DecisionPending,
            journal::labels(&paths.run, Some(&decision.reference.as_wire())),
            journal::payload(&[
                ("reference", json!(decision.reference.as_wire())),
                ("kind", json!(decision.kind)),
                ("unblocks", json!(decision.unblocks)),
            ]),
        )?;
    }
    let cleared: Vec<Decision> = held
        .iter()
        .filter(|(reference, _)| !decisions.contains_key(*reference))
        .map(|(_, decision)| decision.clone())
        .collect();
    for decision in cleared {
        journal.emit(
            journal::PipelineKind::DecisionCleared,
            journal::labels(&paths.run, Some(&decision.reference.as_wire())),
            journal::payload(&[
                ("reference", json!(decision.reference.as_wire())),
                ("kind", json!(decision.kind)),
                ("released", json!(decision.unblocks)),
            ]),
        )?;
    }
    *held = decisions.clone();
    Ok(())
}

/// The nodes no decision point will let start yet.
fn paused_by(decisions: &BTreeMap<DecisionRef, Decision>) -> BTreeSet<String> {
    decisions
        .values()
        .flat_map(|decision| decision.unblocks.iter().cloned())
        .collect()
}

/// Say once, of each node, that its dependencies have settled and it may go.
///
/// The fact the whole roundless contract turns on: a dependency settling is what
/// makes its dependents actionable, and this is where that is visible to a
/// reader who is not watching dispatches.
fn announce_ready(
    paths: &RunPaths,
    journal: &mut Journal,
    statuses: &BTreeMap<String, NodeStatus>,
    announced: &mut BTreeSet<String>,
) -> Result<()> {
    announced.retain(|id| statuses.get(id).copied() == Some(NodeStatus::Ready));
    let fresh: Vec<String> = statuses
        .iter()
        .filter(|(_, status)| **status == NodeStatus::Ready)
        .map(|(id, _)| id.clone())
        .filter(|id| !announced.contains(id))
        .collect();
    for id in fresh {
        journal.emit(
            journal::PipelineKind::NodeReady,
            journal::labels(&paths.run, Some(&id)),
            journal::payload(&[]),
        )?;
        announced.insert(id);
    }
    Ok(())
}

/// Where a relayed envelope says its member's turn can be reached.
///
/// `oneagentgraph` stamps its own run id and the member on every envelope one of
/// its members produces, and those two values are exactly what its `interrupt`
/// addresses a turn by. This crate has no second way to learn either — a graph
/// run is not this run, and which member a node's graph runs is the graph's
/// business — so the address is read off the stream and kept current: the latest
/// envelope wins, because that is the turn a note aimed at the node now would be
/// correcting.
fn addressed_by(envelope: &Envelope) -> Option<TurnAddress> {
    if envelope.source != crate::event::Source::Agentgraph {
        return None;
    }
    TurnAddress::of(
        envelope.labels.run_id.as_deref()?,
        envelope.labels.extra.get("member")?.as_str()?,
    )
}

/// Whether any node could still change state without an edit or an attestation.
///
/// `ready` and `running` are the obvious two. A **draft-complete** node is the
/// third: this loop is what watches for the release that lifts its draft, so
/// breaking here would settle the run with the temporary pin still in the change.
/// Everything else is settled or gated by something only the channel delivers.
fn any_node_can_still_move(statuses: &BTreeMap<String, NodeStatus>) -> bool {
    statuses.values().any(|status| {
        matches!(
            status,
            NodeStatus::Ready | NodeStatus::Running | NodeStatus::CompleteDraft
        )
    })
}

/// Drain the planner's durable command queue and answer every claimed envelope.
///
/// Both edits that change only *eligibility* — `attest` and `reparent` — take
/// effect on this same pass, because the derived statuses are recomputed from
/// the graph rather than stored.
///
/// The author's allowlist is enforced here as well as at submission: the queue
/// is durable, so an envelope reaching this loop may have been written by a
/// build or a caller that did not check, and the reconciler is the last place a
/// refusal still means something.
fn reconcile_edits(
    paths: &RunPaths,
    journal: &mut Journal,
    state: &mut RunState,
    channel: &ChannelState,
    launch: &LaunchRecord,
    in_flight: &mut BTreeMap<String, Dispatch>,
) -> Result<bool> {
    let mut changed = false;
    for envelope in channel.claim_commands()? {
        let author = envelope.author;
        let mut applied = true;
        let mut reason = None;
        for command in &envelope.commands {
            let compiled = crate::channel::allows(author, command).and_then(|()| {
                compile_and_deliver(paths, state, author, command, launch, in_flight)
            });
            match compiled {
                Ok(operations) => {
                    // Dropping or retrying a running node raises its
                    // cooperative cancellation signal: the dispatch stops and,
                    // for a lifecycle node, preserves what it committed.
                    for target in cancelled_by(command) {
                        if let Some(dispatch) = in_flight.get(&target) {
                            dispatch.cancel.cancel();
                        }
                    }
                    journal.emit(
                        journal::PipelineKind::EditCommitted,
                        journal::labels(&paths.run, None),
                        journal::payload(&[
                            ("author", json!(author)),
                            ("command", json!(command)),
                            ("operations", json!(operations)),
                        ]),
                    )?;
                    record_operation_facts(paths, journal, author, &operations)?;
                    // An edit the monitor made is the planner's to review: it
                    // was applied on the monitor's own judgement, so the planner
                    // learns of it without being asked to approve it first.
                    if author == crate::channel::Author::Monitor {
                        if let Some(surface) = monitor_edit(command) {
                            raise(paths, journal, surface)?;
                        }
                    }
                    *state = projection::fold(&journal::read(&paths.journal()));
                    changed = true;
                }
                Err(error) => {
                    applied = false;
                    reason = Some(error.to_string());
                    record_rejection(paths, journal, author, command, &error)?;
                    break;
                }
            }
        }
        channel.answer_commands(&CommandOutcome {
            id: envelope.id,
            applied,
            reason,
        })?;
    }
    Ok(changed)
}

/// Validate one command, hand a note to the node's conversation where its
/// `deliver` asks for that, and compile what actually happened.
///
/// The order matters both ways. Validation first, because a note must not be
/// offered to a live conversation on behalf of an edit the reconciler is about to
/// refuse; delivery before the compile that is recorded, because *how* the note
/// reached the node is part of the mutation — a note a turn took is not also owed
/// to the next dispatch.
fn compile_and_deliver(
    paths: &RunPaths,
    state: &RunState,
    author: crate::channel::Author,
    command: &Command,
    launch: &LaunchRecord,
    in_flight: &BTreeMap<String, Dispatch>,
) -> Result<Vec<edits::Operation>> {
    // The loop's own frontier, which is the ledger's plus what only this process
    // knows: which dispatches are still running. A node the journal records as
    // parked can still have one, and an edit judged without that is the edit
    // that returns a node to a workspace its own predecessor is holding.
    let frontier = Frontier {
        in_flight: in_flight
            .iter()
            .map(|(id, dispatch)| (id.clone(), dispatch.live()))
            .collect(),
        // The launch's own, read off the record this loop read strictly at the
        // start of the pass — never out of this process's environment, which is
        // a driver an `adopt` started somewhere else with a different one.
        node_validator: launch.node_validator().map(str::to_owned),
        ..state.frontier()
    };
    let mut candidate = state.graph.clone();
    let operations = edits::compile(&mut candidate, &frontier, author, command)?;
    let Command::Note {
        id,
        addressee,
        text,
        criterion,
        deliver,
        persist,
    } = command
    else {
        return Ok(operations);
    };
    // The note's own record is the delivery's, so the structural compile above
    // contributed none: it established that the ask is one this run can act on,
    // and this is the answer the conversation gave.
    deliver_manager_note(
        paths,
        &Offered {
            id,
            addressee: *addressee,
            text,
            criterion: criterion.as_ref(),
            reach: crate::note::Reach::of(id, *deliver, *persist)?,
            dispatchable: frontier.recorded.get(id) != Some(&NodeStatus::Done),
        },
        in_flight
            .get(id)
            .and_then(|dispatch| dispatch.control.clone())
            .as_ref(),
    )
}

/// Put one refused edit into the run's record: the rejection, and the surface that
/// makes sure nobody has to go looking for it.
///
/// Both writers of the graph call this — the reconciler, and `reply` when nothing
/// is driving the run and it becomes the single writer itself — because which of
/// them judged an edit is an accident of timing and a planner reading the record
/// afterwards should not be able to tell. A note that reached nobody is the case
/// this matters most for: the whole point of refusing one is that the
/// non-delivery is *said*, and a refusal recorded on one path only is silence on
/// the other.
///
/// # Errors
///
/// The reason the run's own journal or channel could not be written.
pub(crate) fn record_rejection(
    paths: &RunPaths,
    journal: &mut Journal,
    author: crate::channel::Author,
    command: &Command,
    error: &Error,
) -> Result<()> {
    journal.emit(
        journal::PipelineKind::EditRejected,
        journal::labels(&paths.run, None),
        journal::payload(&[
            ("author", json!(author)),
            ("command", json!(command)),
            ("reason", json!(error.to_string())),
        ]),
    )?;
    // Every rejection is also surfaced, so no accepted command is silently
    // dropped.
    raise(
        paths,
        journal,
        Surface {
            id: 0,
            kind: "edit-rejected".into(),
            message: format!("reconciler: rejected — {error}"),
            source: crate::channel::source::RECONCILER.into(),
            blocking: false,
            queued_at: sys::now_millis(),
            workstream: None,
        },
    )
}

/// One note as it is offered: the note itself, and where it may land.
///
/// A struct rather than five parameters because the note's two axes are read
/// together and mean nothing apart — see
/// [`Command::Note`](crate::channel::Command::Note), which is where what each of
/// them decides is declared, and [`crate::note::Reach`], which is the pair they
/// make once the envelope has refused the combination that lands nowhere.
pub(crate) struct Offered<'a> {
    /// The node whose dispatch the note is for.
    pub id: &'a str,
    /// Whose task it says it updates.
    pub addressee: crate::note::Addressee,
    /// What that party reads.
    pub text: &'a crate::note::NoteText,
    /// The criterion it binds in the conversation it reaches, when it binds one.
    pub criterion: Option<&'a crate::note::Criterion>,
    /// Where it may land: whether the running turn is attempted, and whether a
    /// note no turn took is composed into the node's next dispatch.
    pub reach: crate::note::Reach,
    /// Whether the node has a next dispatch at all. A node that has settled `done`
    /// does not, which is what turns a carry into the reach-nobody refusal.
    pub dispatchable: bool,
}

/// Hand one manager note to a node's conversation where `deliver` asks for that,
/// carry it to the node's next dispatch where no turn took it and `persist` asks
/// for that, and compile what became of it.
///
/// The delivery is `oneagentgraph`'s and the routing `onejudge`'s: the note goes to
/// whichever party of the two-party member is live, and the other party receives it
/// with that party's response. What this decides is only *which member* — the one
/// the node's dispatch is running, live or not.
///
/// **Not only the live one**, and that is the point of the second address below: a
/// note arriving after the node's dispatch has completed is still asked of the
/// member it was for, so the answer that comes back is the conversation's own —
/// naming how it ended — rather than this crate's guess that there was nothing to
/// ask. Addressing nothing at all is the one case this composes itself, and it says
/// which case it is.
///
/// What that answer becomes is [`persist`](Offered::persist)'s: a note the
/// conversation took is delivered and composes forward into nothing; a note it
/// could not take is carried to the node's next dispatch, or — with `persist` off,
/// or with no next dispatch to carry it to — refused under the one reach-nobody
/// rule, naming what left it nowhere to go.
///
/// # Errors
///
/// [`Error::Refused`] for a note that reached nobody, carrying the conversation's
/// own sentence where the conversation is what answered. The reconciler journals
/// that refusal and surfaces it, so a non-delivery is in the run's record and not
/// only in the caller's exit code.
pub(crate) fn deliver_manager_note(
    paths: &RunPaths,
    offered: &Offered<'_>,
    live: Option<&TurnAddress>,
) -> Result<Vec<edits::Operation>> {
    let Offered {
        id,
        addressee,
        text,
        criterion,
        reach,
        dispatchable,
    } = *offered;
    let note = crate::note::of(addressee, text, criterion)
        .map_err(|refused| Error::Refused(format!("note: node '{id}': {refused}")))?;
    let recorded = |reached| {
        Ok(vec![edits::Operation::NoteDelivered {
            node: id.to_string(),
            addressee,
            text: text.clone(),
            criterion: criterion.cloned(),
            reached,
        }])
    };
    // `next` declines the live attempt outright, so there is no turn to ask and
    // the note is the carried one by construction. The combination that carries
    // it nowhere was refused at the envelope.
    if !reach.attempts_a_live_turn() {
        return match dispatchable {
            true => recorded(crate::note::Reached::Carried),
            false => Err(nowhere_to_carry(id)),
        };
    }
    let attempted = match live.cloned().or_else(|| last_turn_address(paths, id)) {
        Some(address) => agentgraph::note(&address, &note).map_err(|why| why.to_string()),
        None => Err(
            "no dispatch of this node has reported a member yet, so there is no \
                     conversation to hand it to"
                .to_string(),
        ),
    };
    match attempted {
        // A turn of the running dispatch's conversation took it — including the
        // one that queues it for the next turn *of that conversation* to open,
        // which is the running dispatch and not a later one. So nothing is owed
        // forward: this is the whole of the delivery.
        Ok(accepted) => recorded(crate::note::Reached::from(&accepted)),
        // Nothing took it, which is the only case `persist` has an opinion about.
        Err(_) if reach.composes_forward() && dispatchable => {
            recorded(crate::note::Reached::Carried)
        }
        Err(why) if reach.composes_forward() => Err(crate::note::reaches_nobody(
            id,
            &format!(
                "{why}; and it has settled done, so no dispatch of it will take the note \
                 either"
            ),
        )),
        Err(why) => Err(crate::note::reaches_nobody(
            id,
            &format!("{why}; and `persist: false` composes it into no dispatch"),
        )),
    }
}

/// The reach-nobody refusal for a note with nowhere left to be carried to.
fn nowhere_to_carry(id: &str) -> Error {
    crate::note::reaches_nobody(
        id,
        "it has settled done, so no dispatch of it will ever take the note and \
         `deliver: next` asks for no live delivery",
    )
}

/// Where the last dispatch of `node` was addressed, read back out of the run's own
/// merged store.
///
/// The same two labels [`addressed_by`] reads off a live envelope, taken from the
/// record instead — which is what makes a note to a node whose dispatch has
/// *finished* reach the member that had it, and be refused by that member's own
/// account of how it ended.
fn last_turn_address(paths: &RunPaths, node: &str) -> Option<TurnAddress> {
    journal::read(&paths.journal())
        .into_iter()
        .rev()
        .filter(|envelope| envelope.labels.node.as_deref() == Some(node))
        .find_map(|envelope| addressed_by(&envelope))
}

/// Carry one arrival note into a node's running turn, falling through to that
/// node's next dispatch where there is none.
///
/// The lever a *release arrival* is delivered by, and the only caller left of the
/// sibling's `interrupt`: a manager's own note goes through the two-party note
/// seam instead, which is a different verb and reaches both parties.
///
/// `oneagentgraph interrupt`'s exit 3 — no controllable turn in flight — is the
/// answer this is built around, and it is a **fact** rather than a failure: it
/// is what the fall-through to the next dispatch is decided on. A delivery that
/// was attempted and *broke* is neither, and is refused: a caller told `deferred`
/// when the truth is that the lever failed has been told something that is not so.
fn deliver_note(
    journal: &mut Journal,
    id: &str,
    note: &str,
    in_flight: &BTreeMap<String, Dispatch>,
) -> Result<edits::Delivery> {
    let Some(address) = in_flight
        .get(id)
        .and_then(|dispatch| dispatch.control.clone())
    else {
        return Ok(edits::Delivery::Deferred);
    };
    let interrupt = agentgraph::interrupt(&address, note);
    // Whatever it answered, the sibling published an envelope saying the lever
    // was pulled and what came of it. It belongs in the merged store like any
    // other envelope this crate's processes produce — stamped with the node it
    // is about, which its producer could not know.
    for event in interrupt.events {
        let mut event = event;
        if event.labels.node.is_none() {
            event.labels.node = Some(id.to_string());
        }
        journal.relay(&event)?;
    }
    match interrupt.outcome {
        Interrupted::Delivered => Ok(edits::Delivery::Live),
        Interrupted::NoTurn(_) => Ok(edits::Delivery::Deferred),
        Interrupted::Failed(reason) => Err(Error::Refused(format!(
            "delivering the arrival note to node '{id}' failed: {reason}"
        ))),
    }
}

/// Tell every fast-adoption node whose awaited releases have all arrived, exactly
/// once — and by telling a **draft-complete** one, lift its draft.
///
/// Delivery is [`deliver_note`] — the running turn where the node has one, and
/// that node's next dispatch where it does not — and `release-adopted` is the
/// durable record that makes it deliver once across a driver's death.
///
/// A draft-complete node has no turn to reach, so that record is also what
/// returns it to the frontier — on the branch its own settlement pinned it to.
/// Moving the pin is that worker's; lifting the draft is its publication's,
/// because a publication carrying no reason is what `onevcs` lifts one on.
///
/// The note is not an edit: nobody submitted it and no author owns it, so it is
/// recorded under its own kind rather than as an `edit-committed` attributed to a
/// planner who never wrote it.
fn adopt_releases(
    paths: &RunPaths,
    journal: &mut Journal,
    state: &mut RunState,
    statuses: &BTreeMap<String, NodeStatus>,
    releases: &mut crate::release::Watch,
    in_flight: &BTreeMap<String, Dispatch>,
) -> Result<bool> {
    // Every node an arrival is owed to: the dispatches in flight, and the nodes
    // this run stopped short of merging. A node in neither has either not been
    // told anything to correct or has already settled on what it was told.
    let told: Vec<Node> = in_flight
        .values()
        .map(|dispatch| dispatch.node.clone())
        .chain(
            state
                .graph
                .iter()
                .filter(|node| statuses.get(&node.id) == Some(&NodeStatus::CompleteDraft))
                .cloned(),
        )
        .collect();
    let ready = releases.ready_to_adopt(&told);
    if ready.is_empty() {
        return Ok(false);
    }
    for (node, released) in ready {
        let note = crate::release::arrival_note(&released);
        // Whatever the lever answered, the node has been told: the delivery falls
        // through to the next dispatch where there is no controllable turn, and
        // a delivery that was *attempted and broke* is the one case that leaves
        // the note owed — so that one is not recorded and is tried again.
        let delivery = match deliver_note(journal, &node, &note, in_flight) {
            Ok(delivery) => delivery,
            Err(error) => {
                eprintln!(
                    "onepipeline: the release note for node '{node}' was not delivered: {error}"
                );
                continue;
            }
        };
        releases.adopted(&node);
        journal.emit(
            journal::PipelineKind::ReleaseAdopted,
            journal::labels(&paths.run, Some(&node)),
            journal::payload(&[
                ("node", json!(node)),
                (
                    "delivery",
                    json!(match delivery {
                        edits::Delivery::Live => "live",
                        edits::Delivery::Deferred => "next",
                    }),
                ),
                (
                    "versions",
                    json!(released
                        .iter()
                        .map(crate::release::Released::payload)
                        .collect::<Vec<_>>()),
                ),
            ]),
        )?;
    }
    *state = projection::fold(&journal::read(&paths.journal()));
    Ok(true)
}

/// The nodes whose in-flight dispatch a command stops.
fn cancelled_by(command: &Command) -> Vec<String> {
    match command {
        Command::Drop { id, .. } | Command::Retry { id, .. } | Command::Cancel { id, .. } => {
            vec![id.clone()]
        }
        _ => Vec::new(),
    }
}

/// Start every node whose dependencies have settled, bounded by `concurrency`.
///
/// The moment they settle: a node reaches this the same pass its last dependency
/// recorded `done`, so nothing waits on a boundary. What it does *not* start is
/// a node a decision point is holding — `paused` is that subtree, and every
/// other branch runs on regardless.
// llmlint: ignore-block[invalid_states_unrepresentable] the resolved graph stays a
// string because LaunchRecord is the durable internal schema and oneagentgraph's
// ConfigRef is transparent/string-valued. A second resolved-graph type across
// scheduling and threads would add no invariant beyond the launch check above.
#[allow(
    clippy::too_many_arguments,
    reason = "the reconcile loop's borrowed state, which cannot be bundled without \
              taking one mutable borrow where three independent ones are needed"
)]
fn start_ready(
    paths: &RunPaths,
    journal: &mut Journal,
    state: &mut RunState,
    statuses: &BTreeMap<String, NodeStatus>,
    rules: &ExecutorRules,
    launch: &LaunchRecord,
    tx: &Sender<Message>,
    in_flight: &mut BTreeMap<String, Dispatch>,
    paused: &BTreeSet<String>,
    releases: &crate::release::Watch,
) -> Result<bool> {
    let concurrency = state.graph.concurrency as usize;
    // Two things become actionable here. A `ready` node is dispatched, and a
    // human action that has just become ready is *recorded* as waiting — the
    // derived status is not enough, because `attest` validates against the
    // frontier the journal actually wrote.
    let actionable: Vec<Node> = state
        .graph
        .iter()
        .filter(|node| match statuses.get(&node.id) {
            Some(NodeStatus::Ready) => true,
            Some(NodeStatus::Waiting) => !state.recorded.contains_key(&node.id),
            _ => false,
        })
        .filter(|node| !in_flight.contains_key(&node.id))
        .filter(|node| !paused.contains(&node.id))
        .cloned()
        .collect();

    let mut settled_here = false;
    for node in actionable {
        if node.kind != NodeKind::Human && in_flight.len() >= concurrency {
            break;
        }
        // An `expects_no_diff` node settles deterministically, without
        // dispatching: the executor never infers this from task prose.
        if node.expects_no_diff {
            settle(
                paths,
                journal,
                &Settlement::plain(&node.id, NodeStatus::Done, Some(NO_CHANGES)),
            )?;
            settled_here = true;
            continue;
        }
        if node.kind == NodeKind::Human {
            // A ready human action is a settlement, not a dispatch: the harness
            // never guesses that an approval or a deployment happened.
            settle(
                paths,
                journal,
                &Settlement::plain(&node.id, NodeStatus::Waiting, None),
            )?;
            settled_here = true;
            continue;
        }

        let cancel = CancellationToken::new();
        journal.emit(
            journal::PipelineKind::NodeDispatched,
            journal::labels(&paths.run, Some(&node.id)),
            journal::payload(&[("persona", json!(node.persona)), ("attempt", json!(1))]),
        )?;
        // What the run can say about every dependency of this node that lands
        // outside its own repository. Empty for a node that has none — which is
        // every node a plan naming neither new field carries — and the dispatch
        // is then composed exactly as it always was.
        let references = releases.references(&node);
        spawn(
            paths,
            rules,
            launch,
            &node,
            &references,
            cancel.clone(),
            tx.clone(),
        )?;
        let now = Instant::now();
        in_flight.insert(
            node.id.clone(),
            Dispatch {
                node,
                cancel,
                started: now,
                last_progress: now,
                reported_quiet: false,
                control: None,
            },
        );
        settled_here = true;
    }
    if settled_here {
        *state = projection::fold(&journal::read(&paths.journal()));
    }
    Ok(settled_here)
}

/// Run one node's dispatch on a thread, reporting back to the single writer.
#[allow(
    clippy::too_many_arguments,
    reason = "one dispatch's whole context: the run, the rules, the launch, the node, \
              its cross-repository references, its cancellation, and where to report"
)]
fn spawn(
    paths: &RunPaths,
    rules: &ExecutorRules,
    launch: &LaunchRecord,
    node: &Node,
    references: &[crate::plan::CrossRepoReference],
    cancel: CancellationToken,
    tx: Sender<Message>,
) -> Result<()> {
    // The labels a `node_label` rule selects on. An executor is chosen once per
    // node, before its steps run, so a node's own labels are what exists here.
    let labels = dispatch_labels(&paths.run, &node.id, None, node.persona.as_deref());
    let executor_name = rules.select(node.executor.as_deref(), &labels, &|name| {
        rules
            .executors
            .iter()
            .find(|entry| entry.name == name)
            .map(|entry| crate::rules::executor_for(entry).capacity())
            .unwrap_or_default()
    })?;
    let entry = rules
        .executors
        .iter()
        .find(|entry| entry.name == executor_name)
        .ok_or_else(|| Error::Invalid(format!("executor '{executor_name}' is not declared")))?
        .clone();

    let run = paths.run.clone();
    let node = node.clone();
    let references = references.to_vec();
    let paths = paths.clone();
    // Cloned off the record the loop read **strictly**, rather than re-read from
    // disk where a dispatch needs it: `launch.json` is external input, and a
    // second, leniently-read copy of it would be a graph reference or a filter
    // this build could not honour arriving where nothing can refuse it.
    let launched = crate::lifecycle::Launch {
        node_graph: launch.node_graph.clone(),
        pr_author_graph: launch.pr_author_graph().map(str::to_owned),
        vcs_filter: launch.filters.vcs.clone(),
    };
    std::thread::Builder::new()
        .name(format!("dispatch-{}", node.id))
        .spawn(move || {
            let executor = crate::rules::executor_for(&entry);
            let settlement = if node.repo.is_some() {
                crate::lifecycle::execute(
                    executor.as_ref(),
                    &paths,
                    &launched,
                    &node,
                    &references,
                    &cancel,
                    &tx,
                )
            } else {
                execute_direct(
                    executor.as_ref(),
                    &run,
                    &launched.node_graph,
                    &node,
                    &references,
                    &cancel,
                    &tx,
                )
            };
            let _ = tx.send(Message::Settled(Box::new(settlement)));
        })
        .map_err(|e| Error::Invalid(format!("cannot start a dispatch thread: {e}")))?;
    Ok(())
}

/// Run one direct agent node: one dispatch in the selected project directory.
fn execute_direct(
    executor: &dyn Executor,
    run: &str,
    default_graph: &str,
    node: &Node,
    references: &[crate::plan::CrossRepoReference],
    cancel: &CancellationToken,
    tx: &Sender<Message>,
) -> Settlement {
    let graph = node_graph(node.agent_graph.as_ref(), default_graph);
    // The node's controls are narrowed *before* a dispatch is composed, and a
    // declaration no dispatch can run under settles the node instead of being
    // launched with. Validation refuses one at every submission, so reaching
    // this arm means the graph came from somewhere validation did not run — a
    // journal a stale build wrote, or one edited by hand — and the answer there
    // is the same as the plan's, in the node's own settlement.
    // llmlint: ignore-block[changed_behavior_has_e2e] no invocation a user can type
    // reaches this arm: `graph::validate` refuses the declaration at `start` and at
    // every live edit, so the only graph carrying one is
    // folded from a journal an *earlier build* wrote. This suite could reach that only
    // by writing that journal by hand, which would prove the fixture rather than the
    // code, and deleting the arm would reinstate the silent default this control
    // exists to remove. Held instead by the unit test below, which drives the real
    // `LocalExecutor`.
    let controls = match crate::controls::NodeControls::of_node(node) {
        Ok(controls) => controls,
        Err(why) => {
            return Settlement {
                detail: Some(why),
                ..Settlement::plain(&node.id, NodeStatus::Failed, Some(INVALID_NODE))
            }
        }
    }; // llmlint: ignore-end[changed_behavior_has_e2e]
    let request = || DispatchRequest {
        graph: graph.clone(),
        task: node.rendered_task_with(references),
        labels: dispatch_labels(run, &node.id, None, node.persona.as_deref()),
        controls,
        workspace: WorkspaceSpec::Path(project_dir()),
        cancel: cancel.clone(),
    };
    attempt(executor, &node.id, cancel, tx, &request).settlement
}

/// How far one attempt got.
///
/// Ordered, and deliberately one value rather than two flags: an attempt that
/// spoke without starting is not a state, and a retry decision made from two
/// independent bools has to keep proving that it never happens.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Reached {
    /// The dispatch could not be started at all. Nothing about this failure is
    /// the agent's, so asking again unchanged spends the next budget the same
    /// way.
    NotStarted,
    /// It ran and recorded nothing. This is the one case a retry exists for:
    /// the failure carries no work to lose.
    Silence,
    /// It recorded something. An attempt that answered has already answered,
    /// whatever its exit status.
    Speech,
}

/// One dispatch, drained: how it settled, and what it left behind.
pub(crate) struct Drained {
    /// How the node settled on this attempt.
    pub settlement: Settlement,
    /// How far the attempt got, which is what decides whether asking again
    /// could produce a different answer.
    pub reached: Reached,
    /// The `onevcs` session it left open, when its workspace was one.
    pub session: Option<onevcs::SessionToken>,
    /// The branch that session has checked out.
    pub branch: Option<String>,
}

/// Run a dispatch, asking again for one that produced *nothing*.
///
/// Only an attempt that produced no events is retried: an attempt that answered
/// has already answered, whatever its exit status, and asking again would spend
/// another budget on work that is already done. A provider that refuses before
/// the first turn is the case this exists for — the one where the failure
/// carries no work to lose.
pub(crate) fn attempt(
    executor: &dyn Executor,
    node: &str,
    cancel: &CancellationToken,
    tx: &Sender<Message>,
    request: &dyn Fn() -> DispatchRequest,
) -> Drained {
    let attempts = boundary_attempts();
    let mut backoff = Duration::from_secs(boundary_backoff_seconds());
    let mut last = Drained {
        settlement: failed(node, INFRASTRUCTURE_FAILURE),
        reached: Reached::NotStarted,
        session: None,
        branch: None,
    };

    for attempt in 1..=attempts.get() {
        let drained = match executor.dispatch(request()) {
            Ok(mut handle) => drain(handle.as_mut(), tx, node, cancel),
            Err(error) => Drained {
                settlement: Settlement {
                    detail: Some(error.to_string()),
                    // Named an infrastructure failure rather than a task the
                    // agent failed, because none of it is the agent's: the
                    // dispatch layer refused before any work began. It is
                    // still retried below, and this is the case retrying is
                    // most likely to recover — an executor that was
                    // momentarily unable to start anything.
                    ..failed(node, INFRASTRUCTURE_FAILURE)
                },
                reached: Reached::NotStarted,
                session: None,
                branch: None,
            },
        };
        if drained.settlement.status != NodeStatus::Failed
            || drained.reached == Reached::Speech
            || cancel.is_cancelled()
        {
            return drained;
        }
        last = drained;
        if attempt == attempts.get() {
            // The budget was spent without the agent producing anything.
            // Reported apart from an ordinary task failure because retrying
            // this one unchanged spends the next budget the same way — and
            // apart from a dispatch that never started, which failed for a
            // reason that has nothing to do with the agent.
            if last.reached != Reached::NotStarted {
                last.settlement = Settlement {
                    detail: last.settlement.detail.clone(),
                    ..failed(node, NO_AGENT_PROGRESS)
                };
            }
            break;
        }
        std::thread::sleep(backoff);
        backoff = (backoff * 2).min(BOUNDARY_BACKOFF_CEILING);
        // Announced only once the wait is over, so the record marks the moment
        // the node was actually asked again rather than the moment the last
        // attempt gave up.
        let _ = tx.send(Message::Redispatched(Box::new(Redispatch {
            node: node.to_string(),
            attempt: NonZeroU32::MIN.saturating_add(attempt),
            attempts,
            reason: last.settlement.detail.clone().unwrap_or_default(),
        })));
    }
    last
}

/// Relay a dispatch's events into the merged stream and settle on its outcome.
///
/// Reports whether the dispatch said anything at all, which is what decides
/// whether asking again could produce a different answer.
///
/// It is also where a cancellation becomes something that *stops* the dispatch.
/// Raising the token alone stops nothing — no agent process reads it — so a
/// cancelled dispatch is asked, through the lever `oneagentgraph` already
/// exposes, to commit what it has and end its turn, and is torn down if it has
/// not exited by the deadline.
pub(crate) fn drain(
    handle: &mut dyn DispatchHandle,
    tx: &Sender<Message>,
    node: &str,
    cancel: &CancellationToken,
) -> Drained {
    let grace = Duration::from_secs(cancel_grace_seconds());
    // The stream is read on a thread of its own so this loop keeps a clock of
    // its own. A dispatch that has gone quiet is exactly the one a supervisor
    // cancels, and a drain blocked on the next envelope would notice the
    // cancellation only if the dispatch spoke again — which is how a cancelled
    // node kept committing for forty-five minutes.
    let (relayed, arriving) = mpsc::channel();
    let events = handle.events();
    // Not waited on: the channel closing is what says the stream ended, and a
    // thread that could not be started closes it immediately — which is a
    // dispatch relayed as silent rather than a drain that hangs.
    let _ = std::thread::Builder::new()
        .name(format!("relay-{node}"))
        .spawn(move || {
            for envelope in events {
                if relayed.send(envelope).is_err() {
                    return;
                }
            }
        });

    let mut spoke = false;
    // Where this dispatch's turns can be reached, learned from the stream: the
    // graph run and member are `oneagentgraph`'s own labels and this crate has
    // no second way to know either. Every member that has named a turn is asked,
    // because which of them still has a live one is the sibling's answer rather
    // than something to infer here.
    let mut addresses: Vec<TurnAddress> = Vec::new();
    // What the producer said killed this dispatch, if it said so at all. The
    // **first** such envelope and not the last: a member that dies takes the run
    // down with it, so the deaths after the first are the teardown it caused.
    let mut death = Death::Unstated;
    // What the producer published about the turns its members ran, which is what
    // a death is reconciled against before it is acted on.
    let mut turns = TurnRecords::default();
    let mut asked_at: Option<Instant> = None;
    let mut killed = false;
    loop {
        match arriving.recv_timeout(TEARDOWN_TICK) {
            Ok(Ok(envelope)) => {
                spoke = true;
                if let Some(address) = addressed_by(&envelope) {
                    if !addresses.contains(&address) {
                        addresses.push(address);
                    }
                }
                turns.read(&envelope);
                if matches!(death, Death::Unstated) {
                    if let Some(published) = MemberDeath::of(&envelope) {
                        // The turn record arrives before the death it is about —
                        // a producer closes a turn and then reports the member,
                        // in that order, on its own `seq` — so what has been read
                        // by the time a death arrives is the record for the turn
                        // that death is about.
                        death = if published.from_provider
                            && member_of(&envelope)
                                .is_some_and(|member| turns.contradicts_a_death_of(member))
                        {
                            Death::Contradicted
                        } else {
                            Death::Published(published)
                        };
                    }
                }
                let _ = tx.send(Message::Event(Box::new(envelope)));
            }
            // A line this build cannot read, skipped exactly as it always was.
            Ok(Err(_)) => {}
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
        match asked_at {
            None if cancel.is_cancelled() => {
                // Cooperative: the dispatch is asked to stop and preserve its
                // work, which killing the process would not.
                handle.cancel(CancelMode::Cooperative);
                let said = interrupt_turns(tx, node, &addresses, grace);
                report(tx, node, CancelPhase::Interrupted, said);
                asked_at = Some(Instant::now());
            }
            // It was asked and it is still here. The work it has committed is
            // safe on its branch; whatever it has not is what the ask was for,
            // and the deadline is what stops a dispatch that ignored it.
            Some(asked) if !killed && asked.elapsed() >= grace => {
                killed = true;
                handle.cancel(CancelMode::Kill);
                report(
                    tx,
                    node,
                    CancelPhase::Killed,
                    format!(
                        "the dispatch had not exited {}s after it was asked to stop, so it \
                         was killed and its process tree reaped; anything its turn had not \
                         committed is gone",
                        grace.as_secs()
                    ),
                );
            }
            _ => {}
        }
    }

    let waited = handle.wait();
    // Where a session token stops being text and starts addressing a session.
    // `DispatchOutcome::session` is the contract's own `Option<String>` — the
    // seam is a wire shape and carries no sibling types — so this is the one
    // place a run's own token is taken into the type every reader of it uses.
    let (session, branch) = match &waited {
        Ok(outcome) => (
            outcome.session.clone().map(onevcs::SessionToken),
            outcome.branch.clone(),
        ),
        Err(_) => (None, None),
    };
    let settlement = match waited {
        Ok(outcome) if outcome.succeeded && !cancel.is_cancelled() => {
            Settlement::plain(node, NodeStatus::Done, None)
        }
        Ok(_) if cancel.is_cancelled() => Settlement {
            // How it stopped, on the settlement itself: the surfaces say it as
            // it happens, and this is what a reader of the settled node sees
            // afterwards. Absent where the cancellation arrived after the
            // dispatch had already ended, which asked nothing of anybody.
            detail: asked_at.map(|_| stopped_how(killed, grace)),
            ..Settlement::plain(node, NodeStatus::Cancelled, None)
        },
        Ok(outcome) => failed_task(node, &outcome, session.as_ref(), &death),
        Err(error) => Settlement {
            detail: Some(error.to_string()),
            ..failed(node, INFRASTRUCTURE_FAILURE)
        },
    };
    Drained {
        settlement,
        reached: if spoke {
            Reached::Speech
        } else {
            Reached::Silence
        },
        session,
        branch,
    }
}

/// Ask every turn this dispatch has named to stop, and say what each answered.
///
/// None of the three answers is a failure. A member on a harness with no
/// out-of-band control, a turn that was already over, and a redirection the
/// sibling would not take are all *facts* about the lever, and the deadline
/// applies either way — so each is recorded and the cancellation carries on.
fn interrupt_turns(
    tx: &Sender<Message>,
    node: &str,
    addresses: &[TurnAddress],
    grace: Duration,
) -> String {
    if addresses.is_empty() {
        return format!(
            "nothing of this dispatch has named a turn to interrupt, so there was nothing to \
             ask; it is killed in {}s if it has not exited by then",
            grace.as_secs()
        );
    }
    let mut answers = Vec::new();
    for address in addresses {
        let interrupt = agentgraph::interrupt(address, CANCEL_INPUT);
        // Whatever it answered, the sibling published an envelope saying the
        // lever was pulled and what came of it. It belongs in the merged store
        // like any other, stamped with the node it is about — which its producer
        // could not know.
        for mut event in interrupt.events {
            if event.labels.node.is_none() {
                event.labels.node = Some(node.to_string());
            }
            let _ = tx.send(Message::Event(Box::new(event)));
        }
        answers.push(format!(
            "{}: {}",
            address.member(),
            answered(&interrupt.outcome)
        ));
    }
    format!(
        "asked {} turn(s) to stop, commit, and end without starting new work — {}; the \
         dispatch is killed in {}s if it has not exited by then",
        addresses.len(),
        answers.join("; "),
        grace.as_secs()
    )
}

/// What one interrupt answered, as a planner reads it.
fn answered(outcome: &Interrupted) -> String {
    match outcome {
        Interrupted::Delivered => "the running turn took the redirection".to_string(),
        Interrupted::NoTurn(reason) => format!("no turn to redirect ({reason})"),
        Interrupted::Failed(reason) => format!("the lever failed ({reason})"),
    }
}

/// How a cancelled dispatch ended, on its own settlement.
fn stopped_how(killed: bool, grace: Duration) -> String {
    if killed {
        format!(
            "the dispatch was asked to stop and had not exited {}s later, so it was killed",
            grace.as_secs()
        )
    } else {
        "the dispatch stopped after its turn was asked to commit and end".to_string()
    }
}

/// Tell the run's single writer about one transition of a cancellation.
fn report(tx: &Sender<Message>, node: &str, phase: CancelPhase, detail: String) {
    let _ = tx.send(Message::Cancelling(Box::new(Cancelling {
        node: node.to_string(),
        phase,
        detail,
    })));
}

/// The surface one transition of a cancellation is raised as.
///
/// Non-blocking: the planner asked for this, so holding its dependents back to
/// report it would pause a run over a decision already made. It is raised
/// against the node, so it reaches whoever is reading that workstream.
fn cancelling_surface(step: &Cancelling) -> Surface {
    Surface {
        id: 0,
        kind: step.phase.kind().into(),
        message: format!("{}: {}", step.phase.kind(), bounded(&step.detail)),
        source: crate::channel::source::RECONCILER.into(),
        blocking: false,
        queued_at: sys::now_millis(),
        workstream: Some(step.node.clone()),
    }
}

/// The word a dispatch that ended for a reason that is not the agent's verdict
/// on its task settles under.
///
/// The line a reader will get wrong is against [`INFRASTRUCTURE_FAILURE`], which
/// is the dispatch layer refusing **before any work began** and is why [`attempt`]
/// retries that one: it carries no work to lose. This is the opposite — the
/// dispatch started and the agent worked — and it is not retried.
///
/// Not `dispatch-failed`, which [`crate::lifecycle::Undrafted::ending`] already
/// publishes for a drafting dispatch; the collision is caught by
/// [`tests::the_words_this_crate_publishes_are_one_vocabulary`] rather than
/// assumed away.
pub const DISPATCH_DIED: &str = "dispatch-died";

/// The same death, where what killed the dispatch was the **provider**.
///
/// A narrowing of [`DISPATCH_DIED`] rather than a second word for it, and the
/// narrowing is what a reader acts on: a node whose provider went is a node with
/// nothing wrong with its work, and the word it settled under used to send the
/// reader looking for what the work got wrong. The journal already said so — the
/// producer's own liveness rule is `provider-failure` — and this is that fact
/// reaching the settlement, the results, and the views instead of stopping at the
/// event.
///
/// A node that failed its own task still settles [`TASK_FAILED`], and a dispatch
/// that died to anything else — a heartbeat, a stall, a signal — still settles
/// [`DISPATCH_DIED`]. This word names provider deaths and nothing else.
pub const PROVIDER_FAILED: &str = "provider-failed";

/// A node whose declaration no dispatch could be composed from.
pub const INVALID_NODE: &str = "invalid-node";

/// A node whose work its base branch already carries — or that wrote none.
///
/// Deliberately the same word `crate::vcs::outcome_of` gives a publication with
/// nothing to publish: the two are one fact about the node, and a second spelling
/// would be a distinction no reader could act on. It is the one word this crate
/// publishes twice, and [`tests::the_words_this_crate_publishes_are_one_vocabulary`]
/// names it as such rather than letting the next sharing in unnoticed.
pub const NO_CHANGES: &str = "no-changes";

/// The dispatch layer refused **before any work began**.
///
/// The word [`DISPATCH_DIED`] is deliberately not: see the reasoning there.
pub const INFRASTRUCTURE_FAILURE: &str = "infrastructure-failure";

/// The dispatch budget was spent without the agent producing anything.
pub const NO_AGENT_PROGRESS: &str = "no-agent-progress";

/// The agent's own verdict on its task was that it failed.
pub const TASK_FAILED: &str = "task-failed";

/// The same, over a session that had already opened a change request.
pub const TASK_FAILED_CHANGE_OPEN: &str = "task-failed-change-open";

// llmlint: ignore-block[contracts_have_one_source_or_a_drift_gate] there is no source to
// derive these from and no gate to reconcile them against. The classification vocabulary
// belongs to `oneharness`, which is deliberately **not** a dependency of this crate —
// AGENTS.md fixes the direction at `onepipeline → {oneagentgraph, onevcs}` — so a typed
// source would mean taking one on for two punctuation marks. And there is no vocabulary
// here to go stale: no word of the producer's is compared against anything, every
// classification is carried exactly as it was spelled, and what these two hold is three
// English words for "the machinery" and the two delimiters prose sets a token apart with.
// A producer that changed either leaves the detail unclassified, which settles the node
// `task-failed` — the outcome it settled under before this existed — rather than reporting
// something untrue.
/// The words this crate takes as a detail talking about the **machinery** rather
/// than about the task.
///
/// The guard on the lift below, which needs one: a delimited token is no evidence
/// on its own, and reading `the gate failed (clippy)` as a dispatch that died
/// would report a node whose *work* is wrong as a node whose harness broke.
const MACHINERY: [&str; 3] = ["harness", "provider", "spawn"];

/// The delimiters the machinery sets a classification apart from its prose with:
/// `harness failed (rate_limit)`, and `codex [spawn-error]` for a chain naming
/// every candidate it stepped past.
///
/// The **shape**, not a vocabulary — what the token says is carried as the
/// producer spelled it, so a classification that layer adds arrives here without
/// this crate learning it.
const CLASSIFIED_IN: [(char, char); 2] = [('(', ')'), ('[', ']')];
// llmlint: ignore-end[contracts_have_one_source_or_a_drift_gate]

/// What a producer said killed one of its members, off the `member-died`
/// envelope it published while the dispatch was running.
///
/// The **stated** classification, as against the one [`dispatch_death_cause`]
/// reads out of a sentence: this is the producer saying which of its members
/// died and why, and that one is this crate reading standard error for want of
/// anything better. The reading stays as the degrade path for a producer that
/// publishes no such event.
struct MemberDeath {
    // llmlint: ignore[invalid_states_unrepresentable] a cause is the plain string every
    // classification in this crate is, for the reason `NodeResult::cause` records: the
    // word is the harness's and that vocabulary grows there, so what this crate does is
    // check the value for what it does with it — `is_a_classification`, on both edges it
    // crosses — and carry it. This is one of those edges, and the value it holds is the
    // one `Settlement::cause` and the journal payload carry unchanged.
    /// What killed the member, as its producer classified it: `oneagentgraph`'s
    /// own `Cause`, carried as the word it was spelled with.
    cause: String,
    /// Whether the liveness rule that fired was the **provider** one.
    ///
    /// A decision taken at the boundary rather than the rule carried through it:
    /// the only question anything here asks of that field is whether it is
    /// `oneagentgraph`'s own `Rule::ProviderFailure`, and a rule this build does
    /// not know is simply not that one. So nothing downstream holds another
    /// producer's unbounded string, and a rule renamed there is a compile error
    /// here rather than a word that silently stops matching.
    from_provider: bool,
}

impl MemberDeath {
    /// What a relayed envelope says killed a member, or `None` for an envelope
    /// that is not one saying so.
    ///
    /// The kind is the sibling's own spelling rather than a literal, so a rename
    /// there is a compile error here. The payload is **not** deserialized through
    /// that library's `MemberDied`, and deliberately: the producer is a program
    /// resolved on the `PATH` at dispatch time, so it may be a newer release than
    /// the one this build links, and that type is `deny_unknown_fields` — a field
    /// added there would turn every death it publishes into an unreadable payload
    /// and settle the node `task-failed` again, which is what this exists to
    /// stop. So the one value a settlement carries is read off the payload and
    /// bounded by the check every other classification crosses.
    // llmlint: ignore[changed_behavior_has_e2e] the two arms that answer `None` for an
    // envelope that *is* a death are not reachable from any producer in this tree: this
    // crate and `onevcs` publish no `member-died` at all, and `oneagentgraph` writes its
    // `cause` through a closed enum, so a payload carrying a sentence, a control
    // character, or no cause at all is a stream something else wrote. Reaching either end
    // to end would mean hand-writing that envelope, which would prove the fixture, and
    // dropping them would put another process's JSON on a rendered line unchecked. Held
    // by this module's unit test, which drives every shape past the real reading. The
    // arms a producer *does* reach are journeys:
    // `boundary::a_published_death_decides_the_settlement_ahead_of_the_sentence_the_dispatch_exits_on`
    // for a death that is read, and every `.died` journey for a producer that publishes
    // none.
    fn of(envelope: &Envelope) -> Option<Self> {
        if envelope.source != crate::event::Source::Agentgraph
            || envelope.kind.0 != oneagentgraph::event::EventKind::MemberDied.as_str()
        {
            return None;
        }
        let cause = envelope.payload.get("cause")?.as_str()?;
        is_a_classification(cause).then(|| Self {
            cause: cause.to_owned(),
            from_provider: envelope.payload.get("rule").and_then(Value::as_str)
                == Some(oneagentgraph::member::Rule::ProviderFailure.as_str()),
        })
    }
}

/// What a producer published about the turns one dispatch ran, as far as this
/// dispatch's own stream carries it.
///
/// The **record** a death is reconciled against. `oneagentgraph` closes a turn
/// only on a harness record it could settle on — a run that reported `ok` and
/// exited `0` — and the close carries what that one turn consumed, so a turn that
/// is open *and* closed on this stream is a turn whose own record said the work
/// finished and the provider was spent on.
///
/// Keyed by the member as well as the turn, because a graph runs several and a
/// turn number is only unique within one. A producer that stamps no member is one
/// member's stream as far as anything here can tell, and keys under the same
/// empty name on both sides — which is what keeps a single-sided producer's turn
/// record readable rather than silently unmatchable.
#[derive(Debug, Default)]
struct TurnRecords {
    /// The turn each member last **started**, kept whether or not it went on to
    /// close: a completion is recorded beside this rather than taken out of it,
    /// because the two together are the question — the turn a member was last on,
    /// and whether that same turn closed billed.
    last_started: BTreeMap<String, u64>,
    /// The members and turns whose close carried a **non-zero**
    /// [`USAGE_FIGURES`] figure, which is the whole of what "billed" means here:
    /// a close carrying no usage, or a usage whose every figure is nought, is
    /// not in this set.
    with_billed_usage: BTreeSet<(String, u64)>,
}

// llmlint: ignore-block[changed_behavior_has_e2e] the half a producer reaches *is* a
// journey: a close with no start leaves a member with no turn recorded, so the death stands,
// which is what `lifecycle::a_dispatch_whose_member_died_is_settled_from_the_classification_its_producer_published`
// and `boundary::a_published_death_decides_the_settlement_ahead_of_the_sentence_the_dispatch_exits_on`
// settle `provider-failed` over. The rest are the trust boundary — a turn number that is
// not a number, a member label that is not a member name, a usage carrying no figure —
// and every one of them is a shape no producer in this tree writes: `oneagentgraph` emits
// both kinds through its own closed types. Reaching one end to end means hand-writing that
// envelope, which proves the fixture; dropping the guards would let another process's JSON
// suppress a real death. Held by this module's own test, which drives every shape past the
// real reading.
impl TurnRecords {
    /// Fold one relayed envelope in, where it says something about a turn.
    fn read(&mut self, envelope: &Envelope) {
        if envelope.source != crate::event::Source::Agentgraph {
            return;
        }
        let Some(turn) = envelope.payload.get("turn").and_then(Value::as_u64) else {
            return;
        };
        let Some(member) = member_of(envelope).map(str::to_owned) else {
            return;
        };
        let kind = &envelope.kind.0;
        if kind == oneagentgraph::event::EventKind::TurnStarted.as_str() {
            self.last_started.insert(member, turn);
        } else if kind == oneagentgraph::event::EventKind::TurnCompleted.as_str()
            && has_a_usage_figure(envelope.payload.get("usage"))
        {
            self.with_billed_usage.insert((member, turn));
        }
    }

    /// Whether the turn this member was last on is one its own record says
    /// completed, and reported usage for.
    ///
    /// The turn it was **last on**, because a death names none: a `member-died`
    /// says which member went and why, so the turn it is about is the one that
    /// member had reached. A member that started none has no record to reconcile
    /// against, and a death naming one stands.
    fn contradicts_a_death_of(&self, member: &str) -> bool {
        self.last_started
            .get(member)
            .is_some_and(|turn| self.with_billed_usage.contains(&(member.to_owned(), *turn)))
    }
} // llmlint: ignore-end[changed_behavior_has_e2e]

/// The key a relayed envelope's records pair under: the member it was stamped
/// with, [`UNSTAMPED_MEMBER`] for a producer that stamped none, and `None` for a
/// label this build cannot read as a member.
///
/// The third answer is a refusal rather than a default. The label is another
/// process's JSON and this key is what a turn record and a death are correlated
/// on, so folding an unreadable one onto the unstamped key would let a stranger's
/// record contradict a real member's death — `projection::member_label` keeps the
/// same three apart, for the same reason on the rendering side.
fn member_of(envelope: &Envelope) -> Option<&str> {
    match envelope.labels.extra.get("member") {
        None => Some(UNSTAMPED_MEMBER),
        // The same token check every other relayed string in this module crosses,
        // for the same reason: this is another process's JSON, and a member name
        // is a graph identifier — `worker`, `check-in` — so a paragraph or a
        // control character is not one, whatever it is. Bounded here because the
        // value becomes a map key held for the life of the dispatch.
        Some(Value::String(member)) => is_a_classification(member).then_some(member.as_str()),
        Some(_) => None,
    }
}

/// The key a producer that stamped no member pairs its records under.
///
/// Deliberately a name no member can have — [`member_of`] refuses an empty
/// label — so a single-sided producer's records pair with each other and with
/// nothing else.
const UNSTAMPED_MEMBER: &str = "";

/// The figures on a turn's usage a non-zero one of which says the provider did
/// work for it.
///
/// Three of the five the sibling declares, and the two left out are the choice: a
/// prompt served from the provider's cache is what a turn is charged *less* for,
/// so a turn whose only non-zero figure is a cache read is not evidence the
/// provider ran one. The three here are independently optional on the wire — a
/// provider that counted tokens and reported no price still ran the turn — so any
/// of them settles it.
///
/// Field names rather than that library's `Usage`, for [`MemberDeath::of`]'s
/// reason: the producer is resolved on the `PATH`, so it may be a newer release
/// than this build links and that type is `deny_unknown_fields`. What keeps the
/// names true is
/// [`tests::the_usage_figures_this_crate_reads_are_the_ones_the_producer_writes`],
/// which asks the linked `Usage` what it serializes to and fails on a rename.
const USAGE_FIGURES: [&str; 3] = ["input_tokens", "output_tokens", "cost_usd"];

/// Whether a turn's usage carries a non-zero [`USAGE_FIGURES`] figure.
///
/// The literal question, because every stronger one would be a claim the wire
/// does not support: `cost_usd` is optional, so "was it charged" is unanswerable
/// for a provider that counts tokens and prices nothing, and "does it report
/// usage" would take a cache-only record — which is the one shape here that is
/// deliberately not evidence a turn ran.
fn has_a_usage_figure(usage: Option<&Value>) -> bool {
    let Some(usage) = usage else { return false };
    USAGE_FIGURES
        .into_iter()
        .filter_map(|figure| usage.get(figure))
        .any(|figure| figure.as_f64().is_some_and(|spent| spent > 0.0))
}

/// What the producer said killed a dispatch, once it has been reconciled against
/// the record of the turn it names.
///
/// Only the provider rule is reconciled — see divergence 49 in
/// [the divergence record](../../../docs/contract-divergences.md) — because only
/// it is a claim about a turn: a heartbeat, a stall and a signal are statements
/// about the member *after* whatever turn it completed, so a closed turn beside
/// one of those contradicts nothing.
enum Death {
    /// The producer published one, and the turn record does not contradict it.
    Published(MemberDeath),
    /// The producer published a `provider-failure` the record for that turn
    /// contradicts: the turn completed, reporting usage.
    Contradicted,
    /// The producer published none. The sentence the dispatch exited on is all
    /// there is, and [`dispatch_death_cause`] is the reading of it.
    Unstated,
}

/// The classification a dispatch death carries, lifted out of the failure's own
/// detail — or `None` where that detail is the agent's own verdict on its task.
///
/// The **last** delimited bare token in the detail, because a producer that
/// carries two puts the operation it was performing in front of the reason it
/// stopped, and a producer that names a candidate per identity ends on the one it
/// gave up at.
///
/// Read only where the detail names the machinery — see [`MACHINERY`] — because a
/// delimited token is no evidence of anything on its own.
///
/// The guard is where the reading stops, and it is a reading: a detail is the
/// whole of what this seam is given, so a detail written in the machinery's own
/// shape is the machinery as far as anything here can tell. Both sides of that
/// are driven end to end by `a_verdict_that_delimits_a_token_without_naming_the_/// machinery_stays_a_task_failure` and
/// `a_verdict_written_in_the_machinerys_own_shape_is_read_as_the_machinery`.
// llmlint: ignore[names_match_behavior] the name is the caller's question — `failed_task`
// asks what killed this dispatch and takes `None` for "the agent's own verdict" — and the
// answer is a reading of a sentence, because a sentence is all that arrives: nothing on
// this seam says whose stderr a detail came off. A name encoding the heuristic instead
// would put the mechanism in the caller's vocabulary and still not make it exact. Its cost
// is bounded by what the word does: `dispatch-died` carries the branch and the commit and
// is not re-dispatched, so the worst a misreading does is hand an operator finished work
// rather than ask for it again.
fn dispatch_death_cause(detail: &str) -> Option<String> {
    let lowered = detail.to_ascii_lowercase();
    if !MACHINERY.iter().any(|word| lowered.contains(word)) {
        return None;
    }
    CLASSIFIED_IN
        .into_iter()
        .filter_map(|(open, close)| delimited(detail, open, close))
        .max_by_key(|(at, _)| *at)
        .map(|(_, word)| word)
}

/// The most a classification can be and still be one.
///
/// A producer classifies in a token — `rate_limit`, `spawn-error` — and this value
/// is written onto a settlement, into a journal payload, and onto a rendered line.
/// Anything longer is prose that happened to be delimited, and carrying it would
/// put a paragraph where a reader looks for a word.
const CLASSIFICATION_LIMIT: usize = 64;

/// The last classification one pair of delimiters holds, and where it ended.
///
/// The position comes back with it so the caller can take whichever pair ended
/// last rather than whichever it looked at first.
fn delimited(detail: &str, open: char, close: char) -> Option<(usize, String)> {
    let mut found = None;
    let mut at = 0;
    let mut rest = detail;
    while let Some(start) = rest.find(open) {
        let after = &rest[start + open.len_utf8()..];
        let Some(end) = after.find(close) else { break };
        let inside = &after[..end];
        if is_a_classification(inside) {
            found = Some((at + start, inside.to_owned()));
        }
        at += start + open.len_utf8() + end + close.len_utf8();
        rest = &after[end + close.len_utf8()..];
    }
    found
}

/// Whether what a producer delimited is a classification this crate will carry.
///
/// The trust boundary. A dispatch's stderr is another process's output, read here
/// for one token and rendered wherever the node is; nothing about it is checked
/// before this. So it has to be a **token**: something, short, on one line, and
/// with no control character in it — which is also what keeps a parenthetical
/// clause a producer wrote in prose from being read as one. What it *says* is not
/// checked and must not be, because the words are the harness's and the set grows
/// there.
pub(crate) fn is_a_classification(word: &str) -> bool {
    !word.is_empty()
        && word.len() <= CLASSIFICATION_LIMIT
        && !word.chars().any(|c| c.is_whitespace() || c.is_control())
}

/// How a dispatch that did not succeed settles.
///
/// Three outcomes in one order, and the order is the point. A change request the
/// session opened wins outright — a reviewer is waiting on it whatever ended the
/// dispatch that left it, and `task-failed` over an open change sends a planner to
/// re-run work that is waiting to be read. Failing that, a death the producer
/// **stated** — or, for a producer that stated none, a detail that classifies
/// itself — settles [`DISPATCH_DIED`]; the branch is carried, never consulted, so
/// a dispatch that died holding finished work and one whose workspace disappeared
/// reach the same word.
///
/// The two sources of that classification are ranked and not merged, because one
/// of them is evidence and the other is a reading: a [`MemberDeath`] is the
/// producer saying which of its members died and why, on the stream this dispatch
/// published while it ran, and [`dispatch_death_cause`] is this crate reading a
/// sentence off standard error for want of anything better. So the stated one is
/// taken wherever there is one, and the reading remains for the producer that
/// says nothing.
///
/// A death the turn's own record **contradicts** takes neither. The record is
/// stronger than both — see [`Death`] — and the sentence a contradicted dispatch
/// exits on is the same producer saying the same thing less precisely, so
/// consulting it would put back exactly what the reconciliation took out. What
/// that node settles is the plain failure, carrying the commit its branch was
/// left at: nothing here can say the work passed its bar, and nobody should have
/// to dig through a journal to find out whether there is any.
///
/// Every unknown degrades to the plain failure this arm always produced.
fn failed_task(
    node: &str,
    outcome: &DispatchOutcome,
    session: Option<&onevcs::SessionToken>,
    death: &Death,
) -> Settlement {
    let detail = (!outcome.detail.is_empty()).then(|| outcome.detail.clone());
    if let Some(url) = session.and_then(crate::vcs::change_opened_in) {
        return Settlement {
            detail,
            change_url: Some(url),
            ..failed(node, TASK_FAILED_CHANGE_OPEN)
        };
    }
    let head = || session.and_then(crate::vcs::branch_head_in);
    let (cause, word) = match death {
        Death::Contradicted => {
            return Settlement {
                detail,
                head: head(),
                ..failed(node, TASK_FAILED)
            }
        }
        Death::Published(published) => (
            Some(published.cause.clone()),
            if published.from_provider {
                PROVIDER_FAILED
            } else {
                DISPATCH_DIED
            },
        ),
        Death::Unstated => (dispatch_death_cause(&outcome.detail), DISPATCH_DIED),
    };
    let Some(cause) = cause else {
        return Settlement {
            detail,
            ..failed(node, TASK_FAILED)
        };
    };
    Settlement {
        detail,
        cause: Some(cause),
        head: head(),
        ..failed(node, word)
    }
}

/// How long a cancelled dispatch has to stop itself before it is torn down.
///
/// An unusable value falls back to the default rather than disabling the
/// deadline it configures — or, worse, making it zero, which would turn every
/// cooperative cancel into an immediate kill.
fn cancel_grace_seconds() -> u64 {
    std::env::var(CANCEL_GRACE_ENV)
        .ok()
        .and_then(|value| value.parse().ok())
        .filter(|seconds| *seconds > 0)
        .unwrap_or(DEFAULT_CANCEL_GRACE_SECONDS)
}

/// How many times a dispatch that produced nothing is re-asked.
///
/// A [`NonZeroU32`] for the reason [`publication_attempts`] gives, and the parse
/// *is* the `> 0` filter this used to apply afterwards.
/// [`DEFAULT_BOUNDARY_ATTEMPTS`] stays the published `u32` it has always been, so
/// the conversion happens here and once.
fn boundary_attempts() -> NonZeroU32 {
    std::env::var(BOUNDARY_ATTEMPTS_ENV)
        .ok()
        .and_then(|value| value.parse().ok())
        .or_else(|| NonZeroU32::new(DEFAULT_BOUNDARY_ATTEMPTS))
        .unwrap_or(NonZeroU32::MIN)
}

/// How many times a lifecycle node whose publication keeps failing is dispatched.
///
/// An unusable value falls back to the default rather than disabling the loop it
/// configures — and `0` most of all, which read literally would settle a node
/// having never dispatched it. That is what the [`NonZeroU32`] parse is: the
/// trust boundary this value crosses, so nothing downstream carries a budget it
/// has to re-check.
pub(crate) fn publication_attempts() -> NonZeroU32 {
    std::env::var(PUBLICATION_ATTEMPTS_ENV)
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(DEFAULT_PUBLICATION_ATTEMPTS)
}

/// How many times the merge path behind a push that reached the remote is read.
///
/// A [`NonZeroU32`] for the reason [`publication_attempts`] gives, and the parse
/// *is* the `> 0` filter: a zero here would settle a node on a read nobody made.
pub(crate) fn merge_path_reads() -> NonZeroU32 {
    std::env::var(MERGE_PATH_READS_ENV)
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(DEFAULT_MERGE_PATH_READS)
}

/// The first backoff between those reads. It doubles, to [`BOUNDARY_BACKOFF_CEILING`].
///
/// Bounded at both ends, because this is a value a *run* waits out. A value meant
/// as milliseconds would hold a node open for as long as the number says while a
/// host that answered in a second sat there answering, so it is held to the
/// ceiling on the way in as well as on the way up — the same one every backoff in
/// this crate doubles to, so nothing here waits longer than anything else does.
/// And a stray zero is not a shorter wait but no wait at all: a run that read a
/// host that had gone dark as fast as the process could ask it, so the
/// [`NonZeroU64`] parse falls it back to the default for the reason
/// [`merge_path_reads`] gives about its own zero.
pub(crate) fn merge_path_backoff() -> Duration {
    Duration::from_secs(
        std::env::var(MERGE_PATH_BACKOFF_ENV)
            .ok()
            .and_then(|value| value.parse::<NonZeroU64>().ok())
            .map_or(DEFAULT_MERGE_PATH_BACKOFF_SECONDS, NonZeroU64::get),
    )
    // llmlint: ignore[changed_behavior_has_e2e] the ceiling's only effect is to make a
    // wait *shorter*, so observing it end to end means a journey that waits two minutes
    // per read to prove it did not wait longer — minutes of the offline tier to watch a
    // clock. The fallback half, which is what an operator actually mistypes, is driven by
    // `an_unusable_read_budget_falls_back_rather_than_disabling_the_recovery`; the clamp is
    // held by the unit test below.
    .min(BOUNDARY_BACKOFF_CEILING)
}

/// The next backoff after one, doubled to the ceiling every wait here shares.
pub(crate) fn doubled(backoff: Duration) -> Duration {
    // llmlint: ignore[changed_behavior_has_e2e] the same ceiling as the one on the way in,
    // and the same reason there is no journey: its only effect is to make a wait *shorter*,
    // so a journey observing it is a journey that waits two minutes per read to prove it
    // did not wait longer. What a user can reach — the reads themselves, and the settlement
    // a spent budget writes — is driven end to end in `tests/e2e/lifecycle.rs`; the arithmetic
    // is held by `the_wait_between_merge_path_reads_grows_to_the_ceiling_and_stops_there`.
    (backoff * 2).min(BOUNDARY_BACKOFF_CEILING)
}

fn boundary_backoff_seconds() -> u64 {
    std::env::var(BOUNDARY_BACKOFF_ENV)
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(DEFAULT_BOUNDARY_BACKOFF_SECONDS)
}

fn failed(node: &str, outcome: &str) -> Settlement {
    Settlement::plain(node, NodeStatus::Failed, Some(outcome))
}

/// A reason bounded to what an envelope payload may carry.
pub(crate) fn bounded(reason: &str) -> String {
    reason
        .chars()
        .take(crate::event::MAX_PAYLOAD_TEXT_BYTES / 4)
        .collect()
}

/// The labels a dispatch is stamped with. The reserved keys, and nothing else.
pub(crate) fn dispatch_labels(
    run: &str,
    node: &str,
    step: Option<&str>,
    persona: Option<&str>,
) -> Labels {
    Labels {
        run_id: Some(run.to_string()),
        // Never stamped: execution is continuous, so there is no round to name.
        round: None,
        node: Some(node.to_string()),
        step: step.map(str::to_string),
        persona: persona.map(str::to_string),
        extra: serde_json::Map::new(),
    }
}

/// The node-scope agent graph a dispatch runs under.
pub(crate) fn node_graph(
    override_ref: Option<&oneagentgraph::config::ConfigRef>,
    default_graph: &str,
) -> oneagentgraph::config::ConfigRef {
    override_ref
        .cloned()
        .unwrap_or_else(|| oneagentgraph::config::ConfigRef(default_graph.to_string()))
}

pub(crate) fn configured_node_graph() -> String {
    std::env::var(NODE_GRAPH_ENV)
        .ok()
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| DEFAULT_NODE_GRAPH.to_string())
}
// llmlint: ignore-end[invalid_states_unrepresentable]

/// What [`NODE_VALIDATOR_ENV`] says, when it says anything at all.
///
/// *Set* rather than *usable*: a variable that is there answers this rung, and
/// whether what it holds names a command is settled once, by the caller, for
/// all three rungs alike. So set-and-blank means "this launch names none" and
/// stops the search rather than falling through to a config file that names
/// one — which is what a host exporting it empty to turn the hook off is saying.
///
/// # Errors
///
/// [`Error::Invalid`] for a value this build cannot read as text. That is a
/// rung that is *there* and names something unusable, and it is external input
/// like any other: refused at the boundary rather than discarded, which would
/// silently hand the run whichever validator the config file names.
// llmlint: ignore-block[invalid_states_unrepresentable] the value is a `String` because
// that is what an environment holds and what `LaunchRecord`'s schema declares; the one
// invariant a newtype could carry is applied by the caller, for all three rungs at once.
pub(crate) fn configured_node_validator() -> Result<Option<String>> {
    match std::env::var(NODE_VALIDATOR_ENV) {
        Ok(value) => Ok(Some(value)),
        Err(std::env::VarError::NotPresent) => Ok(None),
        Err(std::env::VarError::NotUnicode(_)) => Err(Error::Invalid(format!(
            "{NODE_VALIDATOR_ENV} holds something this build cannot read as text, so the \
             command it names cannot be resolved — set it to the command, or unset it to \
             declare that this launch has none"
        ))),
    }
}
// llmlint: ignore-end[invalid_states_unrepresentable]

/// What [`ENVELOPE_REVIEWER_ENV`] says, when it says anything at all.
///
/// Read exactly as [`configured_node_validator`] reads its own rung, and for
/// the same reasons: *set* rather than *usable*, so set-and-blank means "this
/// launch names none" and stops the search rather than falling through to a
/// config file that names one, and a value this build cannot read as text is
/// refused at the boundary rather than discarded.
///
/// # Errors
///
/// [`Error::Invalid`] for a value this build cannot read as text.
// llmlint: ignore-block[invalid_states_unrepresentable] the value is a `String` because
// that is what an environment holds and what `LaunchRecord`'s schema declares; the one
// invariant a newtype could carry is applied by the caller, for all three rungs at once.
pub(crate) fn configured_envelope_reviewer() -> Result<Option<String>> {
    match std::env::var(ENVELOPE_REVIEWER_ENV) {
        Ok(value) => Ok(Some(value)),
        Err(std::env::VarError::NotPresent) => Ok(None),
        Err(std::env::VarError::NotUnicode(_)) => Err(Error::Invalid(format!(
            "{ENVELOPE_REVIEWER_ENV} holds something this build cannot read as text, so the \
             command it names cannot be resolved — set it to the command, or unset it to \
             declare that this launch has none"
        ))),
    }
}
// llmlint: ignore-end[invalid_states_unrepresentable]

fn project_dir() -> std::path::PathBuf {
    std::env::var_os(PROJECT_DIR_ENV)
        .map(std::path::PathBuf::from)
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or_else(|| std::path::PathBuf::from("."))
}

fn stall_after_seconds() -> u64 {
    std::env::var(STALL_AFTER_ENV)
        .ok()
        .and_then(|value| value.parse().ok())
        .filter(|seconds| *seconds > 0)
        .unwrap_or(DEFAULT_STALL_AFTER_SECONDS)
}

fn executor_rules() -> Result<ExecutorRules> {
    match std::env::var_os(EXECUTOR_RULES_ENV) {
        Some(path) if !path.is_empty() => ExecutorRules::load(std::path::Path::new(&path)),
        _ => Ok(ExecutorRules::shipped_default()),
    }
}

/// Put the facts one command's operations state — but its `edit-committed` does
/// not surface — into the run's own record, each under its own kind.
///
/// Four of the compiled operations are facts about the *run* rather than
/// mutations of its graph, and a reader looking for any of them should not have
/// to open an operation list to find it. Shared by the two writers of the graph
/// — the reconcile loop, and a `reply` that found nothing driving the run and
/// became the single writer itself — because which of them applied an edit is an
/// accident of timing, and a fact recorded on one path only is silence on the
/// other.
///
/// # Errors
///
/// The reason the run's own journal or channel could not be written.
pub(crate) fn record_operation_facts(
    paths: &RunPaths,
    journal: &mut Journal,
    author: crate::channel::Author,
    operations: &[edits::Operation],
) -> Result<()> {
    for operation in operations {
        match operation {
            edits::Operation::CompletionRequested { reason } => journal.emit(
                journal::PipelineKind::CompletionRequested,
                journal::labels(&paths.run, None),
                journal::payload(&[("reason", json!(reason))]),
            )?,
            edits::Operation::HumanAttested { node } => journal.emit(
                journal::PipelineKind::HumanAttested,
                journal::labels(&paths.run, Some(node)),
                journal::payload(&[("ref", json!(node))]),
            )?,
            edits::Operation::FindingRaised {
                node,
                message,
                blocking,
            } => raise(
                paths,
                journal,
                finding_surface(author, node.clone(), message, *blocking),
            )?,
            // A node an operator settled from evidence settles like any other:
            // under this crate's own `node-settled`, so every reader of a run —
            // the views, the write-back, the telemetry, a consumer folding the
            // store — sees one settlement shape rather than one it has to know
            // to look for in an operation list. What tells the two apart is the
            // outcome word, and the evidence is the detail.
            edits::Operation::SettledFromEvidence {
                node,
                outcome,
                evidence,
            } => journal.emit(
                journal::PipelineKind::NodeSettled,
                journal::labels(&paths.run, Some(node)),
                journal::settled_payload(
                    outcome.as_str(),
                    Some(journal::SETTLED_FROM_EVIDENCE),
                    Some(evidence),
                ),
            )?,
            _ => {}
        }
    }
    Ok(())
}

/// Record one node's settlement.
fn settle(paths: &RunPaths, journal: &mut Journal, settlement: &Settlement) -> Result<()> {
    let mut payload = journal::settled_payload(
        settlement.status.as_str(),
        settlement.outcome.as_deref(),
        settlement.detail.as_deref(),
    );
    if let Some(branch) = &settlement.branch {
        payload.insert("branch".into(), json!(branch));
    }
    if let Some(url) = &settlement.change_url {
        payload.insert("change_url".into(), json!(url));
    }
    if let Some(cause) = &settlement.cause {
        payload.insert(journal::SETTLED_CAUSE.into(), json!(cause));
    }
    if let Some(head) = &settlement.head {
        payload.insert(journal::SETTLED_HEAD.into(), json!(head));
    }
    if let Some(landing) = settlement.landing {
        payload.insert(journal::SETTLED_LANDING.into(), json!(landing.as_str()));
    }
    if !settlement.completed_steps.is_empty() {
        payload.insert("completed_steps".into(), json!(settlement.completed_steps));
    }
    journal.emit(
        journal::PipelineKind::NodeSettled,
        journal::labels(&paths.run, Some(&settlement.node)),
        payload,
    )
}

/// The surface that tells the planner what the monitor did on its own judgement.
///
/// Non-blocking: the edit was applied, so holding anything back to report it
/// would pause the run over a decision that has already been made. Raised by
/// whichever side applied it — the loop, or a `reply` that found nothing
/// driving the run — because which one that was is not the planner's concern.
///
/// `None` for a `finding`, which is the one op that has *already* said its piece
/// to the planner: it compiles to the surface the planner reads, so reporting it
/// a second time as an edit the monitor made would put two entries on the queue
/// for one thing said once — the multiplication this op exists to end.
pub(crate) fn monitor_edit(command: &Command) -> Option<Surface> {
    if matches!(command, Command::Finding { .. }) {
        return None;
    }
    Some(Surface {
        id: 0,
        kind: "monitor-edit".into(),
        message: format!(
            "monitor applied an edit: {}",
            bounded(&serde_json::to_string(command).unwrap_or_default())
        ),
        source: crate::channel::source::MONITOR.into(),
        blocking: false,
        queued_at: sys::now_millis(),
        workstream: crate::channel::target_of(command),
    })
}

/// The surface one accepted `finding` op raises.
///
/// Its source is the envelope's author, so the journal keeps a watcher's finding
/// and a worker's proposal apart — the same reason [`source`] separates a
/// pacemaker update from advice.
///
/// [`source`]: crate::channel::source
pub(crate) fn finding_surface(
    author: crate::channel::Author,
    node: Option<String>,
    message: &str,
    blocking: bool,
) -> Surface {
    Surface {
        id: 0,
        kind: crate::channel::SurfaceKind::Finding.as_str().into(),
        message: message.to_string(),
        source: match author {
            crate::channel::Author::Monitor => crate::channel::source::MONITOR,
            crate::channel::Author::Planner => crate::channel::source::PROPOSAL,
        }
        .into(),
        blocking,
        queued_at: sys::now_millis(),
        workstream: node,
    }
}

/// What the run's own record carries one comparison as.
///
/// The three answers are one field and not the presence or absence of a record:
/// a reader asking "was this criterion checked, and what came back" gets the
/// same shaped answer for a match, a mismatch, and a file the branch would not
/// give up, and never has to read one of them off silence.
fn criterion_payload(checked: &CriterionChecked) -> serde_json::Map<String, serde_json::Value> {
    let mut payload = journal::payload(&[
        ("criterion", json!(bounded(checked.check.criterion()))),
        ("file", json!(bounded(checked.check.file()))),
        ("expected", json!(bounded(checked.check.literal()))),
        ("answer", json!(checked.answer.as_str())),
    ]);
    match &checked.answer {
        crate::criteria::Answer::Match => {}
        crate::criteria::Answer::Mismatch { holds } => {
            payload.insert("holds".into(), json!(bounded(holds)));
        }
        crate::criteria::Answer::Unread { reason } => {
            payload.insert("reason".into(), json!(bounded(reason)));
        }
    }
    payload
}

/// The finding one mismatch raises.
///
/// Non-blocking, and stated as evidence rather than as a verdict: the node has
/// settled and holding its dependents back over a reading nobody has ruled on
/// would make this check the thing it exists to prevent — a demand nobody wrote
/// failing correct work. It names all four things a manager needs to decide
/// without opening the branch: which criterion, which file, what the criterion
/// said, and what the file holds.
/// What the branch holds is passed rather than read back off the answer, so
/// there is no arm here for the two answers that raise nothing: this is reached
/// for a mismatch alone, and a function that could be handed anything else would
/// need a case nobody can ever exercise.
fn criterion_finding(checked: &CriterionChecked, holds: &str) -> Surface {
    let holds = bounded(holds);
    Surface {
        id: 0,
        kind: crate::channel::SurfaceKind::Finding.as_str().into(),
        message: format!(
            "node '{node}' settled against a criterion its branch contradicts.\n\
             criterion: {criterion}\n\
             file: {file}\n\
             expected: {expected}\n\
             the file holds: {holds}\n\
             The node settled on its own work as it always would have and nothing was \
             failed on this: it is a reading of the branch, for you to rule on.",
            node = checked.node.as_str(),
            criterion = bounded(checked.check.criterion()),
            file = bounded(checked.check.file()),
            expected = bounded(checked.check.literal()),
        ),
        source: crate::channel::source::PROPOSAL.into(),
        blocking: false,
        queued_at: sys::now_millis(),
        workstream: Some(checked.node.as_str().to_owned()),
    }
}

/// Raise every projection that failed since this was last asked.
///
/// What it raises is **non-blocking**: a board that is behind holds no dependents
/// back, because the projection is best effort by contract and a store that
/// refused a write is not a node settlement and not a scheduling decision. What
/// it *is* is a board that no longer says what happened, which nothing else tells
/// anybody — the worker's own report is one line on the driver's standard error,
/// and a detached run writes that to a log nobody opens.
///
/// A [`raise`] that *fails* is propagated, exactly as every other one in this
/// loop is: that is the run's own ledger refusing a write, which is not this
/// worker's news arriving badly but the record this driver holds the lock on
/// being unwritable.
fn report_unprojected(
    paths: &RunPaths,
    journal: &mut Journal,
    writeback: &crate::writeback::Writeback,
) -> Result<()> {
    for failure in writeback.take_unprojected() {
        raise(paths, journal, unprojected_surface(&failure))?;
    }
    Ok(())
}

/// One line of whatever a producer said, with its own runs of whitespace closed
/// up.
fn one_line(said: &str) -> String {
    said.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The finding one failed projection raises.
fn unprojected_surface(failure: &crate::writeback::Unprojected) -> Surface {
    let items = failure.items.join(", ");
    Surface {
        id: 0,
        kind: crate::channel::SurfaceKind::Finding.as_str().into(),
        message: format!(
            "the onetaskgraph project '{project}' did not take this run's projection.\n\
             items: {items}\n\
             reason: {reason}\n\
             The run itself is unaffected — nothing was settled, scheduled or failed on \
             this — but the project is behind what the run recorded until it is fixed.",
            project = bounded(failure.project.as_str()),
            items = bounded(&items),
            // On one line, because the line above it is what a reader scans: a
            // sibling's refusal is several lines of its own and one of them is
            // the `next:` it ends with, which read as this surface's own advice.
            reason = bounded(&one_line(&failure.reason)),
        ),
        source: crate::channel::source::PROPOSAL.into(),
        blocking: false,
        queued_at: sys::now_millis(),
        workstream: None,
    }
}

/// Surface something to the planner, recording that it was *sent*.
pub(crate) fn raise(paths: &RunPaths, journal: &mut Journal, surface: Surface) -> Result<()> {
    let queued = ChannelState::new(paths).push(surface)?;
    journal.emit(
        journal::PipelineKind::PlannerSurfaceQueued,
        journal::labels(&paths.run, queued.workstream.as_deref()),
        journal::payload(&[
            ("kind", json!(queued.kind)),
            ("message", json!(queued.message)),
            ("source", json!(queued.source)),
            ("blocking", json!(queued.blocking)),
        ]),
    )
}

/// Report an in-flight dispatch that has recorded nothing past the threshold.
///
/// Non-blocking, because a stall is evidence rather than a verdict: the planner
/// decides whether to cancel the node, retry it, or let it run, and a blocking
/// surface would hold its dependents back to ask.
fn watch_for_quiet(
    paths: &RunPaths,
    journal: &mut Journal,
    stall_after: Duration,
    in_flight: &mut BTreeMap<String, Dispatch>,
) -> Result<()> {
    let quiet: Vec<(String, u64, bool, String)> = in_flight
        .iter()
        .filter(|(_, dispatch)| !dispatch.reported_quiet)
        .filter(|(_, dispatch)| dispatch.last_progress.elapsed() > stall_after)
        .map(|(id, dispatch)| {
            (
                id.clone(),
                dispatch.last_progress.elapsed().as_secs(),
                dispatch.last_progress == dispatch.started,
                dispatch.node.persona.clone().unwrap_or_else(|| "-".into()),
            )
        })
        .collect();

    for (node, quiet_for, never_spoke, persona) in quiet {
        if let Some(dispatch) = in_flight.get_mut(&node) {
            dispatch.reported_quiet = true;
        }
        let last = if never_spoke {
            "nothing recorded since it was dispatched".to_string()
        } else {
            format!("last activity {quiet_for}s ago")
        };
        journal.emit(
            journal::PipelineKind::QuietWorker,
            journal::labels(&paths.run, Some(&node)),
            journal::payload(&[
                ("quiet_for_seconds", json!(quiet_for)),
                ("threshold_seconds", json!(stall_after.as_secs())),
                ("persona", json!(persona)),
            ]),
        )?;
        raise(
            paths,
            journal,
            Surface {
                id: 0,
                kind: "quiet-worker".into(),
                message: format!(
                    "quiet-worker: no activity for {quiet_for}s (threshold {}s); {last}. \
                     The dispatch has not failed — decide whether to cancel it, retry it, \
                     or let it run.",
                    stall_after.as_secs()
                ),
                source: crate::channel::source::PROPOSAL.into(),
                blocking: false,
                queued_at: sys::now_millis(),
                workstream: Some(node.clone()),
            },
        )?;
    }
    Ok(())
}

/// Write the run's result, as the loop closes out.
///
/// One document at the run's root, rewritten each time a driver closes out: the
/// frontier is continuous, so what the ledger records is where the whole graph
/// has got to rather than what one round did.
fn record_result(paths: &RunPaths, state: &RunState, settled: GraphState) -> Result<RunResult> {
    let statuses = state.statuses();
    let landings = landings_after_asking_again(state);
    let mut nodes: Vec<NodeResult> = state
        .graph
        .iter()
        .map(|node| {
            let status = statuses
                .get(&node.id)
                .copied()
                .unwrap_or(NodeStatus::Pending);
            NodeResult {
                id: node.id.clone(),
                status,
                outcome: state.outcomes.get(&node.id).cloned(),
                landing: landings.get(&node.id).copied(),
                action: (status == NodeStatus::Waiting)
                    .then(|| node.task.clone())
                    .flatten(),
                unblocks: if status == NodeStatus::Waiting {
                    graph::unblocks(&state.graph, &node.id)
                } else {
                    Vec::new()
                },
                blocked_by: if status == NodeStatus::Blocked {
                    gating_humans(state, &statuses, &node.id)
                } else {
                    Vec::new()
                },
                // What the dispatch reported, falling back to what the plan
                // pinned: an unpinned lifecycle node's branch is named by the
                // sibling that cut it, so the plan cannot be the only source.
                branch: state
                    .branches
                    .get(&node.id)
                    .cloned()
                    .or_else(|| node.branch.clone()),
                change_url: state.change_urls.get(&node.id).cloned(),
                cause: state.causes.get(&node.id).cloned(),
                head: state.heads.get(&node.id).cloned(),
                // Every node the graph still holds is one nothing superseded:
                // the supersession removes the node it replaced in the same
                // edit, so the two lists cannot overlap.
                superseded_by: None,
            }
        })
        .collect();
    nodes.extend(superseded_results(state, &landings));

    let result = RunResult {
        run_id: paths.run.clone(),
        state: settled,
        nodes,
    };
    ledger::write_json(&paths.result(), &result)?;
    Ok(result)
}

/// Every node's landing after the run has asked again about the ones that had
/// not landed.
///
/// Named for the asking rather than for the answer: a change
/// [`crate::vcs::proved_landed`] shows nothing about keeps the claim its
/// settlement made, so the map is not uniformly fresh and a name promising "now"
/// would say otherwise of it.
///
/// A settlement's `landing` is an observation of a moment, and the run neither
/// blocks nor polls for a merge somebody else owns — so a change merged while the
/// run was still going was reported at the end as work that had reached nobody.
/// This is the last thing the run will ever say about the node, and the reader
/// acts on it.
///
/// **Asked only of the changes that had not landed**, and only where the run
/// recorded the branch to ask about, so a run whose every change merged asks
/// nothing.
fn landings_after_asking_again(state: &RunState) -> BTreeMap<String, Landing> {
    let mut landings = state.landings.clone();
    let unlanded: Vec<String> = landings
        .iter()
        .filter(|(_, landing)| **landing == Landing::Unlanded)
        .map(|(node, _)| node.clone())
        .collect();
    for node in unlanded {
        let Some(branch) = state.branches.get(&node) else {
            continue;
        };
        if crate::vcs::proved_landed(branch) {
            landings.insert(node, Landing::Landed);
        }
    }
    landings
}

/// The nodes a `retry` replaced, as the run's own result records them.
///
/// **After the graph's own nodes, because they are no longer in it** — see
/// [`crate::projection::RunState::superseded`] for what a document built from
/// the graph alone therefore said about them. Everything each node left behind
/// rides along unchanged; the status is `cancelled`, which is what the retry's
/// own stop has always recorded, and
/// [`superseded_by`](NodeResult::superseded_by) is what separates it from a
/// `drop`, which leaves the same word.
fn superseded_results(state: &RunState, landings: &BTreeMap<String, Landing>) -> Vec<NodeResult> {
    state
        .superseded
        .iter()
        .map(|(id, replacement)| NodeResult {
            id: id.clone(),
            status: NodeStatus::Cancelled,
            outcome: state.outcomes.get(id).cloned(),
            landing: landings.get(id).copied(),
            action: None,
            unblocks: Vec::new(),
            blocked_by: Vec::new(),
            branch: state.branches.get(id).cloned(),
            change_url: state.change_urls.get(id).cloned(),
            cause: state.causes.get(id).cloned(),
            head: state.heads.get(id).cloned(),
            superseded_by: Some(replacement.clone()),
        })
        .collect()
}

/// The ready human references a blocked node is transitively gated by.
fn gating_humans(
    state: &RunState,
    statuses: &BTreeMap<String, NodeStatus>,
    id: &str,
) -> Vec<String> {
    let mut gates = BTreeSet::new();
    let mut seen = BTreeSet::new();
    let mut pending = vec![id.to_string()];
    while let Some(current) = pending.pop() {
        if !seen.insert(current.clone()) {
            continue;
        }
        let Some(node) = state.graph.get(&current) else {
            continue;
        };
        for dep in &node.deps {
            match statuses.get(dep) {
                Some(NodeStatus::Waiting) => {
                    gates.insert(dep.clone());
                }
                Some(NodeStatus::Blocked) => pending.push(dep.clone()),
                _ => {}
            }
        }
    }
    gates.into_iter().collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plan::{Plan, PLAN_SCHEMA_VERSION};
    use crate::projection::Recorded;

    /// The two things this change publishes outside the crate are spelled the
    /// same in the README, in the divergence record, and here.
    ///
    /// The gate the publication budget's own has, for the same reason: an
    /// environment variable is set from outside this crate by a name nothing
    /// compiles, and a settlement word is read out of a run's result by one. Both
    /// are stated in three documents, each further from the constant than the
    /// last, and the README is the copy an operator actually meets. Each is
    /// asserted from the constant, so a rename fails here rather than leaving
    /// three documents describing a build that no longer exists.
    ///
    /// The **divergence record** rather than the contract, deliberately: neither
    /// is in `docs/contract.md`, both are proposals to the planner who owns it,
    /// and that is exactly the status the two entries state. A gate against the
    /// contract would be a gate against a document that must not carry them yet.
    #[test]
    fn the_scratch_variable_and_the_provider_word_are_spelled_one_way_in_every_document() {
        let read = |relative: &str| {
            std::fs::read_to_string(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(relative))
                .unwrap_or_else(|error| panic!("{relative} ships: {error}"))
        };
        let readme = read("README.md");
        let divergences = read("docs/contract-divergences.md");
        let contract = read("docs/contract.md");

        for named in [
            crate::executor::NODE_SCRATCH_DIR_ENV,
            PROVIDER_FAILED,
            // The word it narrows, which is what makes the narrowing legible: a
            // README naming only the new one would read as a rename.
            DISPATCH_DIED,
        ] {
            let quoted = format!("`{named}`");
            assert!(
                readme.contains(&quoted),
                "the README does not name {quoted}"
            );
            assert!(
                divergences.contains(&quoted),
                "docs/contract-divergences.md does not name {quoted}"
            );
        }
        // And the two the contract does not carry are still the two it does not
        // carry, so the README's "open divergence" is true when a reader reads it.
        for open in [crate::executor::NODE_SCRATCH_DIR_ENV, PROVIDER_FAILED] {
            assert!(
                !contract.contains(open),
                "docs/contract.md now names {open:?}, so the README must stop calling it an \
                 open divergence and the entry must be marked ruled on"
            );
        }
        assert!(
            readme.contains("open divergence 48") && readme.contains("open divergence 49"),
            "the README does not say which entries these two are waiting on"
        );
        for entry in ["## 48. ", "## 49. "] {
            assert!(
                divergences.contains(entry),
                "docs/contract-divergences.md has no entry {entry:?}"
            );
        }
    }

    /// The bound on re-dispatching a node whose publication keeps failing is
    /// named in the contract that publishes it, under the spelling an operator
    /// sets.
    ///
    /// A knob is a promise: it is set from outside this crate, by a name nothing
    /// compiles, so the name and the documents that carry it need a gate the way
    /// the closed set of kinds has one. The default travels with it, because a
    /// budget whose size a document states wrongly is worse than one it does not
    /// state at all — and so does what spending it settles, which is the other
    /// half an operator plans around.
    #[test]
    fn the_publication_budget_is_the_one_the_contract_and_the_readme_publish() {
        let contract = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("docs/contract.md"),
        )
        .expect("the contract ships");
        assert!(
            contract.contains(&format!("`{PUBLICATION_ATTEMPTS_ENV}`")),
            "docs/contract.md does not name the {PUBLICATION_ATTEMPTS_ENV} bound"
        );
        assert_eq!(DEFAULT_PUBLICATION_ATTEMPTS.get(), 3);
        assert!(
            contract.contains("and three by default"),
            "docs/contract.md does not state the default this build ships"
        );
        assert_eq!(publication_attempts(), DEFAULT_PUBLICATION_ATTEMPTS);

        // The README states the same two facts, in the prose an operator meets
        // the knob in. Nothing compiles that either, and it is the copy furthest
        // from this constant.
        let readme = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("README.md"),
        )
        .expect("the README ships");
        let prose = readme.split_whitespace().collect::<Vec<_>>().join(" ");
        assert!(
            prose.contains(&format!("`{PUBLICATION_ATTEMPTS_ENV}`, three by default")),
            "the README does not state the {PUBLICATION_ATTEMPTS_ENV} bound and its default"
        );
        assert!(
            prose.contains(&format!(
                "settles `{}` under the last failure's word",
                NodeStatus::Failed.as_str()
            )),
            "the README does not say what spending the budget settles the node as"
        );
    }

    /// The wait between those reads falls back when the environment is unusable
    /// and is held to the ceiling every backoff in this crate shares.
    ///
    /// The value a *run* waits out, and the one knob here whose misuse costs time
    /// rather than an answer: a stray `0` reads as no wait at all, and a value
    /// meant as milliseconds would hold a node open for eleven days.
    #[test]
    fn the_wait_between_merge_path_reads_falls_back_and_is_held_to_the_ceiling() {
        assert_eq!(
            merge_path_backoff(),
            Duration::from_secs(DEFAULT_MERGE_PATH_BACKOFF_SECONDS)
        );
        for unusable in ["", "not a number", "-1", "5.5", "0"] {
            std::env::set_var(MERGE_PATH_BACKOFF_ENV, unusable);
            assert_eq!(
                merge_path_backoff(),
                Duration::from_secs(DEFAULT_MERGE_PATH_BACKOFF_SECONDS),
                "{unusable:?} was read as a wait rather than falling back"
            );
        }
        std::env::set_var(MERGE_PATH_BACKOFF_ENV, "1000000");
        assert_eq!(
            merge_path_backoff(),
            BOUNDARY_BACKOFF_CEILING,
            "a value nobody meant holds a node open for as long as it says"
        );
        // Below the ceiling the value is the operator's, which is the whole point
        // of the knob.
        std::env::set_var(MERGE_PATH_BACKOFF_ENV, "1");
        assert_eq!(merge_path_backoff(), Duration::from_secs(1));
        std::env::remove_var(MERGE_PATH_BACKOFF_ENV);
    }

    /// And it grows by doubling until it reaches that ceiling, where it stays.
    ///
    /// The other half of the bound: the wait on the way in is clamped once, and
    /// this is what happens to it over a budget's worth of reads. Unbounded
    /// doubling from a value an operator set would overflow the ceiling in three
    /// reads and hold a node open for the rest.
    #[test]
    fn the_wait_between_merge_path_reads_grows_to_the_ceiling_and_stops_there() {
        assert_eq!(doubled(Duration::from_secs(5)), Duration::from_secs(10));
        assert_eq!(
            doubled(BOUNDARY_BACKOFF_CEILING / 2),
            BOUNDARY_BACKOFF_CEILING
        );
        assert_eq!(
            doubled(BOUNDARY_BACKOFF_CEILING),
            BOUNDARY_BACKOFF_CEILING,
            "the wait grew past the ceiling every backoff in this crate shares"
        );
    }

    /// The bound on re-reading a merge path that went dark is the one the
    /// contract and the README publish, under the spelling an operator sets.
    ///
    /// A knob is a promise, for the reason the publication budget's own gate
    /// gives: it is set from outside this crate, by a name nothing compiles. The
    /// default travels with it, and so does the one thing an operator plans
    /// around — that spending it settles the node rather than sending a worker
    /// back to a tree nothing rejected.
    #[test]
    fn the_merge_path_read_budget_is_the_one_the_contract_and_the_readme_publish() {
        let contract = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("docs/contract.md"),
        )
        .expect("the contract ships");
        assert!(
            contract.contains(&format!("`{MERGE_PATH_READS_ENV}`")),
            "docs/contract.md does not name the {MERGE_PATH_READS_ENV} bound"
        );
        assert_eq!(DEFAULT_MERGE_PATH_READS.get(), 3);
        assert_eq!(merge_path_reads(), DEFAULT_MERGE_PATH_READS);
        assert!(
            contract.contains(&format!(
                "`{MERGE_PATH_READS_ENV}`, three by default and the whole budget"
            )),
            "docs/contract.md does not state the default this build ships"
        );

        let readme = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("README.md"),
        )
        .expect("the README ships");
        let prose = readme.split_whitespace().collect::<Vec<_>>().join(" ");
        assert!(
            prose.contains(&format!("`{MERGE_PATH_READS_ENV}`, three by default")),
            "the README does not state the {MERGE_PATH_READS_ENV} bound and its default"
        );
        assert!(
            prose.contains("reads that never get one settle it `failed`"),
            "the README does not say what spending the read budget settles the node as"
        );
    }

    /// Every word this crate publishes for how something ended is spelled once.
    ///
    /// Three vocabularies reach an operator through the same views and the same
    /// documents — a node's settlement outcome, a publication's own ending, and
    /// the ending of a drafting dispatch that produced no body — and a word in two
    /// of them means two things to a reader who has no way to tell which. That is
    /// not hypothetical: `dispatch-failed` was already taken by a drafting
    /// dispatch when a settlement for a dispatch that died was being named, which
    /// is why the word is [`DISPATCH_DIED`] and not that.
    ///
    /// The one deliberate sharing is named rather than excluded by a rule, so the
    /// next one has to be argued for here instead of arriving unnoticed.
    #[test]
    fn the_words_this_crate_publishes_are_one_vocabulary() {
        use crate::lifecycle::Undrafted;
        use crate::vcs::outcome_of;
        use onevcs::PublishOutcome;

        let publications: std::collections::BTreeSet<&str> = [
            PublishOutcome::Merged(onevcs::Sha("abc".into())),
            PublishOutcome::ChangeOpen(url()),
            PublishOutcome::ChangeDraft(url()),
            PublishOutcome::Queued(url()),
            PublishOutcome::NothingToPublish,
        ]
        .iter()
        .map(outcome_of)
        .chain(EVERY_PUBLICATION_FAILURE.iter().map(|kind| {
            outcome_of(&PublishOutcome::Failed {
                kind: *kind,
                reason: String::new(),
                retained: None,
            })
        }))
        .collect();
        let draftings: std::collections::BTreeSet<&str> = [
            Undrafted::Dispatch(String::new()),
            Undrafted::SchemaRefused,
            Undrafted::Bodyless,
        ]
        .iter()
        .map(Undrafted::ending)
        .collect();

        // Deduplicated inside each vocabulary before the three are compared, so
        // what this measures is a word meaning two things rather than one word
        // reached twice the same way: `publication-failed` is the **residual**,
        // and three kinds settling on it is the whole point of a residual.
        //
        // `no-changes` is one fact reached two ways — a node whose steps all
        // declared no diff, and a publication whose base already carried the
        // branch — and both settle a node under the one word on purpose. It is
        // the only word allowed in two of the three lists, and taking it out here
        // is what makes every other collision fail.
        let mut seen: std::collections::BTreeMap<&str, usize> = std::collections::BTreeMap::new();
        let settlements: std::collections::BTreeSet<&str> =
            SETTLEMENT_OUTCOMES.iter().copied().collect();
        assert_eq!(
            settlements.len(),
            SETTLEMENT_OUTCOMES.len(),
            "one settlement outcome is spelled twice in the list itself"
        );
        for word in settlements
            .iter()
            .copied()
            .chain(publications.iter().copied())
            .chain(draftings.iter().copied())
        {
            *seen.entry(word).or_default() += 1;
        }
        assert_eq!(
            seen.remove(NO_CHANGES),
            Some(2),
            "`{NO_CHANGES}` is documented as the one word two vocabularies share"
        );
        let collided: Vec<&str> = seen
            .iter()
            .filter(|(_, times)| **times > 1)
            .map(|(word, _)| *word)
            .collect();
        assert!(
            collided.is_empty(),
            "these words mean two things to a reader who cannot tell which: {collided:?}"
        );
        // And the new word is in the vocabulary at all, so a rename that emptied
        // the list would not pass this by saying nothing.
        assert!(SETTLEMENT_OUTCOMES.contains(&DISPATCH_DIED));
        assert!(SETTLEMENT_OUTCOMES.contains(&PROVIDER_FAILED));
        assert!(draftings.contains(&"dispatch-failed"));
    }

    /// Every word this crate settles a node under that a publication does not
    /// bring with it.
    ///
    /// A list beside the constants rather than a derivation, for the reason
    /// `crate::vcs`'s own `EVERY_PRESERVING` is one: nothing enumerates a set of
    /// `const`s. It does not stand alone either — every settlement in this crate
    /// names one of those constants, so a **rename** fails to compile — and what
    /// this carries is the other half: a word *added* is a deliberate edit here,
    /// and the gate below is what stops it colliding with a vocabulary an
    /// operator reads through the same views.
    const SETTLEMENT_OUTCOMES: [&str; 8] = [
        INVALID_NODE,
        NO_CHANGES,
        INFRASTRUCTURE_FAILURE,
        NO_AGENT_PROGRESS,
        TASK_FAILED,
        TASK_FAILED_CHANGE_OPEN,
        DISPATCH_DIED,
        PROVIDER_FAILED,
    ];

    /// Every `FailureKind` the sibling distinguishes, for the vocabulary gate.
    ///
    /// Written out because the sibling's enum offers no enumeration of itself —
    /// the same list `crate::vcs`'s own tests keep, and for the same reason:
    /// `vcs::failure_of` matches arm by arm, so a variant added there fails
    /// *that* to compile and this list is what carries it into the gate.
    const EVERY_PUBLICATION_FAILURE: &[onevcs::FailureKind] = &[
        onevcs::FailureKind::Gate,
        onevcs::FailureKind::Invalid,
        onevcs::FailureKind::SyncConflict,
        onevcs::FailureKind::NotImplemented,
        onevcs::FailureKind::ChecksFailed,
        onevcs::FailureKind::ChecksUnsettled,
        onevcs::FailureKind::PushRejected,
        onevcs::FailureKind::PushedUnverified,
    ];

    /// A change request URL, for the publication outcomes that carry one.
    fn url() -> onevcs::Url {
        "https://example.invalid/pull/7".parse().expect("a URL")
    }

    /// What a dispatch death's detail is classified as, and what is left alone.
    ///
    /// The two shapes the machinery writes, and — the half that matters more —
    /// the details that are the agent's own verdict and must keep settling as
    /// one. A classifier that read a parenthesis anywhere would turn a node whose
    /// *work* is wrong into a node whose harness broke, and send whoever read it
    /// to restore a subscription that was never the problem.
    #[test]
    fn a_dispatch_death_is_classified_out_of_the_detail_and_a_task_failure_is_not() {
        for (detail, cause) in [
            (
                "oneagentgraph: member 'worker' failed: provider error (respond): harness                  failed (rate_limit)",
                "rate_limit",
            ),
            ("harness failed (auth)", "auth"),
            ("provider error (quota)", "quota"),
            // A chain that stepped past every candidate brackets each reason
            // beside the identity it belongs to, and the one it gave up at is
            // last.
            (
                "no candidate ran the turn: claude-code [auth], codex [spawn-error]",
                "spawn-error",
            ),
            // Both delimiters in one sentence: what the reader wants is whichever
            // came last, not whichever kind this crate happened to look at first.
            (
                "provider error (respond): no candidate ran the turn: codex [quota]",
                "quota",
            ),
            (
                "the harness chain [claude-code, codex] ended: harness failed (overloaded)",
                "overloaded",
            ),
        ] {
            assert_eq!(
                dispatch_death_cause(detail).as_deref(),
                Some(cause),
                "{detail:?} was not classified as the machinery stopping"
            );
        }
        for verdict in [
            "the node failed its gate",
            "the judge refused the report form",
            // A parenthesis in an agent's own verdict. Nothing here names the
            // machinery, so nothing here is a dispatch that died.
            "the gate failed (clippy)",
            "",
        ] {
            assert_eq!(
                dispatch_death_cause(verdict),
                None,
                "{verdict:?} was read as a dispatch that died rather than a task that failed"
            );
        }
    }

    /// A death is read off the producer's own `member-died` and off nothing else.
    ///
    /// The end-to-end half is
    /// `lifecycle::a_dispatch_whose_member_died_is_settled_from_the_classification_its_producer_published`,
    /// which drives a producer that publishes one. What is held here is the
    /// **boundary**: a relayed envelope is another process's JSON, and every way
    /// of it not being a classification this crate will carry is a value no
    /// producer in this tree emits — `oneagentgraph` writes its `cause` through a
    /// closed enum, so a payload carrying a sentence, a control character, or no
    /// cause at all can only come from something else on the stream.
    #[test]
    fn a_death_is_read_from_the_producers_own_event_and_not_from_anything_beside_it() {
        let died = |source, kind: &str, cause| Envelope {
            v: crate::event::ENVELOPE_VERSION,
            ts: "2026-08-29T00:00:00.000Z".into(),
            stream: "oneagentgraph-1".into(),
            seq: 0,
            source,
            kind: crate::event::EventKind(kind.into()),
            phase: None,
            labels: Labels::default(),
            payload: match cause {
                Some(cause) => serde_json::json!({"rule": "provider-failure", "cause": cause})
                    .as_object()
                    .cloned()
                    .expect("a payload is an object"),
                None => serde_json::Map::new(),
            },
            artifacts: Vec::new(),
        };
        use crate::event::Source;
        let member_died = oneagentgraph::event::EventKind::MemberDied.as_str();

        let read = MemberDeath::of(&died(
            Source::Agentgraph,
            member_died,
            Some(serde_json::json!("quota")),
        ))
        .expect("a death this build can take a classification from");
        assert_eq!(read.cause, "quota");
        // The rule the payload names, decided here rather than carried: the
        // fixtures above are written under the provider rule, which is the one
        // this crate asks about.
        assert!(read.from_provider);
        let mut supervised = died(Source::Agentgraph, member_died, Some(json!("timeout")));
        supervised.payload.insert(
            "rule".into(),
            json!(oneagentgraph::member::Rule::Heartbeat.as_str()),
        );
        assert_eq!(
            MemberDeath::of(&supervised).map(|death| death.from_provider),
            Some(false),
            "a death under another liveness rule was read as the provider's"
        );
        for beside in [
            // This crate's own kinds and `onevcs`'s are relayed onto the same
            // store; only the producer that supervises members publishes a death.
            died(Source::Pipeline, member_died, Some(json!("quota"))),
            died(Source::Vcs, member_died, Some(json!("quota"))),
            // A kind that is not a death, however it is labelled.
            died(Source::Agentgraph, "member-settled", Some(json!("quota"))),
            // A death this build cannot take a classification from. Each of these
            // settles the node the way it settled before any of this existed.
            died(Source::Agentgraph, member_died, None),
            died(Source::Agentgraph, member_died, Some(json!(""))),
            died(Source::Agentgraph, member_died, Some(json!("rate limited"))),
            died(Source::Agentgraph, member_died, Some(json!("quota\n"))),
            died(Source::Agentgraph, member_died, Some(json!(3))),
            died(
                Source::Agentgraph,
                member_died,
                Some(json!("q".repeat(CLASSIFICATION_LIMIT + 1))),
            ),
        ] {
            assert!(
                MemberDeath::of(&beside).is_none(),
                "{beside:?} was read as a member this producer said had died"
            );
        }
    }

    /// The usage figures this crate reads off a turn record are the ones the
    /// linked producer writes.
    ///
    /// The drift gate for [`USAGE_FIGURES`], which is the one place this crate
    /// names a sibling's payload fields rather than deserializing them. The
    /// spelling comes from that library's own type here, so a field renamed
    /// upstream fails this rather than quietly making every turn look unbilled —
    /// which would put back exactly the settlement the reconciliation removes.
    #[test]
    fn the_usage_figures_this_crate_reads_are_the_ones_the_producer_writes() {
        let written = serde_json::to_value(oneagentgraph::event::Usage {
            input_tokens: Some(1),
            output_tokens: Some(1),
            cache_read_tokens: Some(1),
            cache_write_tokens: Some(1),
            cost_usd: Some(1.0),
        })
        .expect("the sibling's usage serializes");
        for figure in USAGE_FIGURES {
            assert!(
                written.get(figure).is_some(),
                "the producer no longer writes {figure:?}: {written}"
            );
        }
        // And the two this crate deliberately does not read are still the two it
        // was deciding about, rather than names that have since become something
        // else.
        for cached in ["cache_read_tokens", "cache_write_tokens"] {
            assert!(written.get(cached).is_some(), "{written}");
            assert!(!USAGE_FIGURES.contains(&cached));
        }
    }

    /// A death is reconciled against the record of the turn it names, and only a
    /// `provider-failure` is.
    ///
    /// The end-to-end halves are
    /// `lifecycle::a_provider_death_the_turns_own_record_contradicts_is_not_settled_as_one`,
    /// whose producer publishes both, and
    /// `lifecycle::a_task_its_agent_failed_and_a_dispatch_its_provider_killed_settle_under_different_words`,
    /// which holds the three words apart. What is held here is the **reading**:
    /// which records pair, which do not, and which figures count. Every
    /// shape below is one a producer can put on that stream, and the pairing is
    /// what decides whether about $24.72 of finished work is thrown away.
    #[test]
    fn a_turn_record_contradicts_only_the_death_of_the_member_and_turn_it_belongs_to() {
        let of = |kind: &str, member: Option<&str>, payload: serde_json::Value| {
            let mut envelope = Envelope {
                v: crate::event::ENVELOPE_VERSION,
                ts: "2026-08-29T00:00:00.000Z".into(),
                stream: "oneagentgraph-1".into(),
                seq: 0,
                source: crate::event::Source::Agentgraph,
                kind: crate::event::EventKind(kind.into()),
                phase: None,
                labels: Labels::default(),
                payload: payload
                    .as_object()
                    .cloned()
                    .expect("a payload is an object"),
                artifacts: Vec::new(),
            };
            if let Some(member) = member {
                envelope.labels.extra.insert("member".into(), json!(member));
            }
            envelope
        };
        let started = oneagentgraph::event::EventKind::TurnStarted.as_str();
        let completed = oneagentgraph::event::EventKind::TurnCompleted.as_str();
        let billed = json!({"turn": 1, "usage": {"output_tokens": 12}});

        // The pair the incident produced: one member, one turn, opened and closed
        // on a record that was billed.
        let mut records = TurnRecords::default();
        records.read(&of(started, Some("worker"), json!({"turn": 1})));
        records.read(&of(completed, Some("worker"), billed.clone()));
        assert!(records.contradicts_a_death_of("worker"));
        // A member whose *next* turn is the one it had open has no record for it,
        // which is what a real provider death looks like: the turn never closed.
        records.read(&of(started, Some("worker"), json!({"turn": 2})));
        assert!(
            !records.contradicts_a_death_of("worker"),
            "a record of an earlier turn was read as the record of the turn that died"
        );
        // And another member's record says nothing about this one.
        assert!(!records.contradicts_a_death_of("reviewer"));

        // A close with no opening is not a turn record this crate can attribute:
        // a death names no turn, so the turn it is about is the one its member had
        // open, and a member with none has nothing to reconcile against.
        let mut orphan = TurnRecords::default();
        orphan.read(&of(completed, Some("worker"), billed.clone()));
        assert!(!orphan.contradicts_a_death_of("worker"));

        // A turn that closed on nothing billed is not the record either: what
        // contradicts a death is a turn the provider was paid for.
        let mut unpaid = TurnRecords::default();
        unpaid.read(&of(started, Some("worker"), json!({"turn": 1})));
        for nothing in [
            json!({"turn": 1}),
            json!({"turn": 1, "usage": {}}),
            json!({"turn": 1, "usage": {"cost_usd": 0.0}}),
            json!({"turn": 1, "usage": {"output_tokens": "many"}}),
        ] {
            unpaid.read(&of(completed, Some("worker"), nothing));
        }
        assert!(!unpaid.contradicts_a_death_of("worker"));
        unpaid.read(&of(completed, Some("worker"), billed.clone()));
        assert!(unpaid.contradicts_a_death_of("worker"));

        // A producer that stamps no member is one member's stream as far as
        // anything here can tell, and both halves key under the same name.
        let mut unstamped = TurnRecords::default();
        unstamped.read(&of(started, None, json!({"turn": 1})));
        unstamped.read(&of(completed, None, billed.clone()));
        assert!(unstamped.contradicts_a_death_of(UNSTAMPED_MEMBER));

        // A label present and unreadable is refused rather than folded onto the
        // unstamped key: a stranger's record would otherwise contradict a real
        // member's death. Every way of not being a member name, including the two
        // that are strings.
        for label in [
            json!(7),
            json!(""),
            json!("a name with spaces in it"),
            json!("worker\n"),
            json!("m".repeat(CLASSIFICATION_LIMIT + 1)),
        ] {
            let mut unreadable = TurnRecords::default();
            let mut opened = of(started, None, json!({"turn": 1}));
            opened.labels.extra.insert("member".into(), label.clone());
            unreadable.read(&opened);
            let mut closed = of(completed, None, billed.clone());
            closed.labels.extra.insert("member".into(), label);
            unreadable.read(&closed);
            assert!(!unreadable.contradicts_a_death_of(UNSTAMPED_MEMBER));
            assert!(!unreadable.contradicts_a_death_of("worker"));
        }

        // Everything that says nothing about a turn is folded as nothing: another
        // producer's stream, and a kind with no turn on it.
        let mut beside = TurnRecords::default();
        let mut relayed = of(started, Some("worker"), json!({"turn": 1}));
        relayed.source = crate::event::Source::Vcs;
        beside.read(&relayed);
        beside.read(&of(started, Some("worker"), json!({})));
        beside.read(&of("member-heartbeat", Some("worker"), json!({"turn": 1})));
        beside.read(&of(completed, Some("worker"), billed));
        assert!(
            !beside.contradicts_a_death_of("worker"),
            "a record from another producer opened a turn for this one"
        );
    }

    /// A death the producer published settles under the word its liveness rule
    /// names, and a death the record contradicts settles under neither.
    ///
    /// The mapping, held beside the journeys that drive it: three inputs on one
    /// seam, and the whole point of the word is that they do not collapse.
    #[test]
    fn each_death_settles_under_the_word_its_own_rule_and_record_leave_it() {
        let failed = DispatchOutcome {
            succeeded: false,
            detail: "oneagentgraph: member 'worker' failed: the turn exited 0 without a report"
                .into(),
            ..DispatchOutcome::default()
        };
        let word = |death: &Death| {
            failed_task("build", &failed, None, death)
                .outcome
                .expect("a failed dispatch settles under a word")
        };
        assert_eq!(
            word(&Death::Published(MemberDeath {
                cause: "quota".into(),
                from_provider: true,
            })),
            PROVIDER_FAILED
        );
        assert_eq!(
            word(&Death::Published(MemberDeath {
                cause: "timeout".into(),
                from_provider: false,
            })),
            DISPATCH_DIED
        );
        // The record won: neither the event nor the sentence it exited on gets a
        // say, and no classification rides a settlement the record contradicts.
        let contradicted = failed_task("build", &failed, None, &Death::Contradicted);
        assert_eq!(contradicted.outcome.as_deref(), Some(TASK_FAILED));
        assert_eq!(contradicted.cause, None);
        // And a producer that published nothing settles exactly as it always did.
        assert_eq!(word(&Death::Unstated), TASK_FAILED);
    }

    /// The checked-in shape of a schema-5 run result.
    const RUN_RESULT_GOLDEN: &str = include_str!("../tests/golden/run-result-v5.json");

    use serde_json::Value;

    /// One node that settled `done` with nothing else to say about it.
    fn settled(id: &str) -> NodeResult {
        NodeResult {
            id: id.into(),
            status: NodeStatus::Done,
            outcome: None,
            landing: None,
            action: None,
            unblocks: Vec::new(),
            blocked_by: Vec::new(),
            branch: None,
            change_url: None,
            cause: None,
            head: None,
            superseded_by: None,
        }
    }

    /// The document the golden pins, built through the types.
    ///
    /// Five nodes because each pins a case the wire has and the others do not.
    /// The landing has three — a change observed on its base, one that had not
    /// reached it, and a node with no change of its own, which carries no
    /// `landing` key at all — and a golden carrying one of them would pin a third
    /// of that change. The fourth is a dispatch that died: the one node carrying a
    /// `cause` and a `head`, which is what schema `4` added. The fifth is a node a
    /// `retry` superseded, which is what schema `5` added — the one node here that
    /// is not in the run's graph at all, and the one carrying `superseded_by`,
    /// which every other node omits.
    fn run_result_golden() -> RunResult {
        RunResult {
            run_id: "golden".into(),
            state: GraphState::Complete,
            nodes: vec![
                NodeResult {
                    id: "merged".into(),
                    status: NodeStatus::Done,
                    outcome: Some("merged".into()),
                    landing: Some(Landing::Landed),
                    branch: Some("onepipeline/merged".into()),
                    ..settled("merged")
                },
                NodeResult {
                    id: "opened".into(),
                    status: NodeStatus::Done,
                    outcome: Some("change-open".into()),
                    landing: Some(Landing::Unlanded),
                    branch: Some("onepipeline/opened".into()),
                    change_url: Some("https://example.invalid/pull/7".into()),
                    ..settled("opened")
                },
                NodeResult {
                    id: "died".into(),
                    status: NodeStatus::Failed,
                    outcome: Some(DISPATCH_DIED.into()),
                    branch: Some("onepipeline/died".into()),
                    cause: Some("rate_limit".into()),
                    head: Some("0123456789abcdef0123456789abcdef01234567".into()),
                    ..settled("died")
                },
                settled("built"),
                NodeResult {
                    id: "replaced".into(),
                    status: NodeStatus::Cancelled,
                    outcome: Some(TASK_FAILED.into()),
                    branch: Some("onepipeline/replaced".into()),
                    superseded_by: Some("replaced-2".into()),
                    ..settled("replaced")
                },
            ],
        }
    }

    /// The shape a run result is written as, pinned to the checked-in golden.
    #[test]
    fn a_schema_5_run_result_is_the_shape_the_golden_pins() {
        let rendered = serde_json::to_string_pretty(&run_result_golden()).expect("it serialises");
        assert_eq!(
            rendered.trim(),
            RUN_RESULT_GOLDEN.trim(),
            "the run result changed shape. If that was deliberate, bump \
             RUN_RESULT_SCHEMA_VERSION and update tests/golden/run-result-v5.json together"
        );
    }

    /// Both landings survive the wire, and a node with none carries no key.
    ///
    /// The omission is the half that has to be checked at the wire rather than
    /// through the types: `None` and `landed` are different values in Rust
    /// whatever the serializer does, but a `landing: null` key on a node that
    /// published nothing would have every consumer branching on a field that is
    /// always present and usually meaningless.
    #[test]
    fn a_schema_5_run_result_round_trips_and_omits_what_it_does_not_have() {
        let value = run_result_golden();
        let read: RunResult =
            serde_json::from_str(RUN_RESULT_GOLDEN).expect("the golden reads back into the types");
        assert_eq!(read, value);
        let again: RunResult =
            serde_json::from_str(&serde_json::to_string(&value).expect("it serialises"))
                .expect("it reads back");
        assert_eq!(again, value);

        let document: Value =
            serde_json::from_str(&serde_json::to_string(&value).expect("it serialises"))
                .expect("it is JSON");
        assert_eq!(document["nodes"][0]["landing"], json!("landed"));
        assert_eq!(document["nodes"][1]["landing"], json!("unlanded"));
        assert!(
            document["nodes"][2].get("landing").is_none(),
            "a node with no change to land carries a landing key anyway: {}",
            document["nodes"][2]
        );
        // And the same for the key schema `5` added: it is on the one node a
        // retry replaced and on none of the four the graph still holds, so a
        // consumer branches on its presence rather than on a field that is there
        // for every node and meaningless for most.
        assert_eq!(document["nodes"][4]["superseded_by"], json!("replaced-2"));
        for at in 0..4 {
            assert!(
                document["nodes"][at].get("superseded_by").is_none(),
                "a node nothing superseded carries a superseded_by key anyway: {}",
                document["nodes"][at]
            );
        }
    }

    /// The version is a decision, not an accident: it moves when the shape does,
    /// and the golden is named for the one it pins.
    #[test]
    fn the_run_result_schema_version_and_the_golden_name_the_same_number() {
        assert_eq!(RUN_RESULT_SCHEMA_VERSION, 5);
        let document: Value = serde_json::from_str(RUN_RESULT_GOLDEN).expect("the golden is JSON");
        assert_eq!(document["schema_version"], RUN_RESULT_SCHEMA_VERSION);
        assert!(
            document.get("round").is_none(),
            "the run's own result document names a round: {document}"
        );
        let written: Value = serde_json::from_str(
            &serde_json::to_string(&run_result_golden()).expect("it serialises"),
        )
        .expect("it is JSON");
        assert_eq!(written["schema_version"], RUN_RESULT_SCHEMA_VERSION);
    }

    /// Which version this build will read, and which it refuses by name.
    ///
    /// One number, unlike the additive bump that first recorded a landing: `2`
    /// and `1` were a *round's* result, and this document has no round to put
    /// theirs in. Read leniently either would be normalised into a run result
    /// that looks like every other, which is the shape of the defect the version
    /// exists to make visible — and a version *ahead* of this build cannot be
    /// read honestly at all, because it may state something there is no field
    /// for.
    #[test]
    fn only_this_build_s_run_result_version_reads_and_every_other_is_refused_by_name() {
        let document = serde_json::to_value(run_result_golden()).expect("it serialises");
        let edit = |document: &Value, each: &dyn Fn(&mut serde_json::Map<String, Value>)| {
            let mut copy = document.clone();
            each(copy.as_object_mut().expect("it is an object"));
            copy
        };

        // Above is a build that knows more than this one. `4` is this document
        // before it carried the nodes a retry superseded, `3` before it carried a
        // cause, `2` and `1` the per-round document that shape replaced, and `0` a
        // number this crate has never written, so each came from somewhere that is
        // not this contract.
        for outside in [RUN_RESULT_SCHEMA_VERSION + 1, 4, 3, 2, 1, 0] {
            let claimed = edit(&document, &|object| {
                object.insert("schema_version".into(), json!(outside));
            });
            let refused = serde_json::from_value::<RunResult>(claimed)
                .expect_err("a result this build never wrote was read as one it did");
            let refusal = refused.to_string();
            assert!(
                refusal.contains(&outside.to_string())
                    && refusal.contains(&RUN_RESULT_SCHEMA_VERSION.to_string()),
                "the refusal of {outside} names neither version: {refusal}"
            );
        }

        // A document with no key at all is the unversioned `1`, and is refused as
        // the missing field it is rather than defaulted into this shape.
        let unversioned = edit(&document, &|object| {
            object.remove("schema_version");
        });
        let refusal = serde_json::from_value::<RunResult>(unversioned)
            .expect_err("an unversioned result was read as this build's version")
            .to_string();
        assert!(
            refusal.contains("schema_version"),
            "the refusal of an unversioned result does not name the field: {refusal}"
        );
    }

    fn agent(id: &str, deps: &[&str]) -> Node {
        Node {
            id: id.into(),
            persona: Some("engineer".into()),
            task: Some("## What\ndo it".into()),
            deps: deps.iter().map(|d| (*d).to_string()).collect(),
            ..Node::default()
        }
    }

    /// A node whose budget no dispatch can run under settles instead of being
    /// launched with.
    ///
    /// Validation refuses one at every submission, so a node reaching here with
    /// a zero came from a graph validation never saw — a journal a stale build
    /// wrote, which `Graph::from_plan` folds without re-checking. The executor
    /// is the real [`LocalExecutor`], deliberately: if the refusal regressed, it
    /// would go looking for `oneagentgraph` rather than quietly running the node
    /// under the graph's own ceiling.
    #[test]
    fn a_node_whose_budget_no_dispatch_can_run_under_settles_rather_than_launching() {
        let (tx, rx) = std::sync::mpsc::channel();
        let node = Node {
            max_turns: Some(0),
            ..agent("build", &[])
        };
        let settlement = execute_direct(
            &crate::executor::LocalExecutor,
            "demo",
            "graphs/node-scope.yaml",
            &node,
            &[],
            &CancellationToken::new(),
            &tx,
        );
        assert_eq!(settlement.status, NodeStatus::Failed);
        assert_eq!(settlement.outcome.as_deref(), Some("invalid-node"));
        let detail = settlement.detail.expect("the settlement says why");
        assert!(detail.contains("no turn at all"), "{detail}");
        assert!(
            rx.try_iter().count() == 0,
            "a node that was never dispatched reported turns"
        );
    }

    fn state_of(nodes: Vec<Node>, recorded: &[(&str, NodeStatus)]) -> RunState {
        let plan = Plan {
            schema_version: PLAN_SCHEMA_VERSION,
            goal: None,
            name: Some("demo".into()),
            concurrency: 4,
            tasks: nodes,
        };
        RunState {
            graph: Graph::from_plan(&plan),
            plan: Some(plan),
            recorded: recorded
                .iter()
                .map(|(id, status)| ((*id).to_string(), Recorded::At(*status)))
                .collect(),
            ..RunState::default()
        }
    }

    /// An envelope only addresses a turn when it carries both halves of the
    /// address the sibling's `interrupt` takes, and only when the sibling is who
    /// produced it: this crate's own events name a run that verb has never heard
    /// of.
    #[test]
    fn only_a_siblings_envelope_naming_a_run_and_a_member_addresses_a_turn() {
        let envelope = |source: crate::event::Source, run: Option<&str>, member: Option<&str>| {
            let mut labels = Labels {
                run_id: run.map(str::to_string),
                node: Some("build".into()),
                ..Labels::default()
            };
            if let Some(member) = member {
                labels.extra.insert("member".into(), member.into());
            }
            Envelope {
                v: crate::event::ENVELOPE_VERSION,
                ts: "2026-08-12T00:00:00.000Z".into(),
                stream: "oneagentgraph-1".into(),
                seq: 0,
                source,
                kind: crate::event::EventKind("turn-started".into()),
                phase: None,
                labels,
                payload: serde_json::Map::new(),
                artifacts: Vec::new(),
            }
        };
        use crate::event::Source;

        assert_eq!(
            addressed_by(&envelope(
                Source::Agentgraph,
                Some("node-scope-1786304152340-19"),
                Some("worker")
            )),
            TurnAddress::of("node-scope-1786304152340-19", "worker")
        );
        for unaddressable in [
            // This crate's own event, whose `run_id` is this run rather than a
            // graph run: sent to `interrupt` it would name a run that does not
            // exist, or worse, another one that does.
            envelope(Source::Pipeline, Some("demo"), Some("worker")),
            // A sibling's envelope missing either half of the address.
            envelope(Source::Agentgraph, Some("graph-1"), None),
            envelope(Source::Agentgraph, None, Some("worker")),
            envelope(Source::Agentgraph, Some("graph-1"), Some("   ")),
            envelope(Source::Agentgraph, Some(""), Some("worker")),
            // A member name that would name a path outside the run: the
            // sibling's own predicate refuses it, so this crate never sends it.
            envelope(Source::Agentgraph, Some("graph-1"), Some("../elsewhere")),
        ] {
            assert_eq!(
                addressed_by(&unaddressable),
                None,
                "{unaddressable:?} was read as an address"
            );
        }
    }

    #[test]
    fn a_blocked_node_names_the_ready_human_gating_it_transitively() {
        let human = Node {
            id: "approve".into(),
            kind: NodeKind::Human,
            task: Some("approve it".into()),
            ..Node::default()
        };
        let state = state_of(
            vec![
                human,
                agent("ship", &["approve"]),
                agent("after", &["ship"]),
            ],
            &[("approve", NodeStatus::Waiting)],
        );
        let statuses = state.statuses();
        assert_eq!(statuses["after"], NodeStatus::Blocked);
        assert_eq!(
            gating_humans(&state, &statuses, "after"),
            vec!["approve".to_string()]
        );
    }

    /// A decision point holds exactly the subtree that depends on it, and
    /// nothing else: an independent branch is not in `unblocks`, so nothing
    /// pauses it.
    #[test]
    fn a_waiting_human_holds_its_own_subtree_and_no_other_branch() {
        let human = Node {
            id: "approve".into(),
            kind: NodeKind::Human,
            task: Some("approve it".into()),
            deps: vec!["seed".into()],
            ..Node::default()
        };
        let state = state_of(
            vec![
                agent("seed", &[]),
                human,
                agent("ship", &["approve"]),
                agent("after", &["ship"]),
                agent("probe", &[]),
                agent("report", &["probe"]),
            ],
            &[("seed", NodeStatus::Done), ("approve", NodeStatus::Waiting)],
        );
        let statuses = state.statuses();
        let decisions = decisions_now(
            &state,
            &statuses,
            &ChannelState::new(&RunPaths::under(
                std::path::Path::new("/nonexistent"),
                "demo",
            )),
        );
        let held = decisions
            .get(&DecisionRef::Attestation("approve".into()))
            .expect("the human action holds");
        assert_eq!(held.kind, "attestation");
        assert_eq!(
            held.unblocks,
            vec!["ship".to_string(), "after".to_string()],
            "the decision held more than its own subtree"
        );
        let paused = paused_by(&decisions);
        assert!(
            !paused.contains("probe"),
            "an independent branch was paused"
        );
        assert!(
            !paused.contains("report"),
            "an independent branch was paused"
        );
    }

    /// Cleared, a decision releases exactly what it held — and says so once.
    /// The two references a decision can carry are the two things that clear
    /// one, and each spells itself: a surface reference can only be a surface's
    /// own id, and a node reference can only be a node's.
    #[test]
    fn a_decision_reference_spells_which_of_the_two_things_clears_it() {
        assert_eq!(
            DecisionRef::Attestation("approve".into()).as_wire(),
            "approve"
        );
        assert_eq!(DecisionRef::Surface(7).as_wire(), "surface:7");
    }

    /// A reference read back off a journal record is the one that was written.
    #[test]
    fn a_decision_reference_reads_back_as_the_thing_that_wrote_it() {
        for reference in [
            DecisionRef::Attestation("approve".into()),
            DecisionRef::Surface(7),
        ] {
            assert_eq!(DecisionRef::of_wire(&reference.as_wire()), reference);
        }
        // A node whose name merely starts like the other spelling is still a
        // node: the id would have to carry the separator, which validation
        // refuses.
        assert_eq!(
            DecisionRef::of_wire("surface-check"),
            DecisionRef::Attestation("surface-check".into())
        );
    }

    #[test]
    fn a_decision_is_reported_when_it_begins_holding_and_again_when_it_releases() {
        let root = std::env::temp_dir().join(format!("onepipeline-decisions-{}", sys::pid()));
        let _ = std::fs::remove_dir_all(&root);
        let paths = RunPaths::under(&root, "demo");
        paths.create().expect("the run directory");
        let mut journal = Journal::open(&paths);

        let decision = Decision {
            reference: DecisionRef::Attestation("approve".into()),
            kind: "attestation".into(),
            unblocks: vec!["ship".into()],
        };
        let mut held = BTreeMap::new();
        let pending: BTreeMap<DecisionRef, Decision> = [(
            DecisionRef::Attestation("approve".to_string()),
            decision.clone(),
        )]
        .into();

        report_decisions(&paths, &mut journal, &pending, &mut held).expect("reported");
        // Reported once: a decision that has not changed is not re-announced on
        // every pass of a loop that wakes forty times a second.
        report_decisions(&paths, &mut journal, &pending, &mut held).expect("reported");
        report_decisions(&paths, &mut journal, &BTreeMap::new(), &mut held).expect("reported");

        let kinds: Vec<String> = journal::read(&paths.journal())
            .iter()
            .map(|event| event.kind.0.clone())
            .collect();
        assert_eq!(
            kinds,
            vec![
                journal::PipelineKind::DecisionPending.as_str().to_string(),
                journal::PipelineKind::DecisionCleared.as_str().to_string(),
            ]
        );
        let cleared = &journal::read(&paths.journal())[1];
        assert_eq!(cleared.payload["released"], json!(["ship"]));
        assert_eq!(cleared.labels.node.as_deref(), Some("approve"));
        assert_eq!(cleared.labels.round, None, "a round was stamped");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_node_is_announced_ready_once_and_again_when_it_becomes_ready_again() {
        let root = std::env::temp_dir().join(format!("onepipeline-ready-{}", sys::pid()));
        let _ = std::fs::remove_dir_all(&root);
        let paths = RunPaths::under(&root, "demo");
        paths.create().expect("the run directory");
        let mut journal = Journal::open(&paths);
        let mut announced = BTreeSet::new();

        let ready: BTreeMap<String, NodeStatus> = [("build".to_string(), NodeStatus::Ready)].into();
        let running: BTreeMap<String, NodeStatus> =
            [("build".to_string(), NodeStatus::Running)].into();
        announce_ready(&paths, &mut journal, &ready, &mut announced).expect("announced");
        announce_ready(&paths, &mut journal, &ready, &mut announced).expect("announced");
        announce_ready(&paths, &mut journal, &running, &mut announced).expect("announced");
        announce_ready(&paths, &mut journal, &ready, &mut announced).expect("announced");

        assert_eq!(
            journal::read(&paths.journal()).len(),
            2,
            "a node was announced ready more than once per time it became ready"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// The four reasons and their fields are the ones the divergence record
    /// proposes, and a consumer in another repository reads.
    ///
    /// They are private vocabulary, so `tests/contract.rs` — which drives the
    /// public surface — cannot reach them, and the entry proposing them is the
    /// only place they are written down. Both directions: a field added here
    /// without a line there fails, and so does one the entry names that this
    /// enum no longer carries.
    #[test]
    fn the_hold_reasons_are_the_ones_the_divergence_record_names() {
        let record = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("docs/contract-divergences.md"),
        )
        .expect("the divergence record ships");
        let entry = record
            .split("\n## ")
            .find(|entry| entry.starts_with("55."))
            .expect("the record still carries entry 55");
        let block: Value = entry
            .split("```json")
            .nth(1)
            .and_then(|rest| rest.split("```").next())
            .and_then(|block| serde_json::from_str(block).ok())
            .expect("entry 55 carries the json block this test drives");

        // One of each, so every variant's own payload is read rather than a list
        // of names kept beside them.
        let reasons = [
            HoldReason::Dependencies {
                blocking: vec!["build".into()],
            },
            HoldReason::Concurrency {
                ahead: vec!["build".into()],
                limit: 1,
            },
            HoldReason::Decision {
                reference: DecisionRef::Surface(7),
            },
            HoldReason::Release {
                awaiting: vec!["build".into()],
            },
        ];
        let mine: Vec<String> = reasons
            .iter()
            .map(|reason| {
                reason.payload()["kind"]
                    .as_str()
                    .expect("a kind")
                    .to_string()
            })
            .collect();
        let named: Vec<String> = serde_json::from_value(block["reason_kinds"].clone())
            .expect("entry 55 names its kinds");
        assert_eq!(mine, named);

        for reason in &reasons {
            let payload = reason.payload();
            let kind = payload["kind"].as_str().expect("a kind");
            let fields: Vec<String> = serde_json::from_value(block["fields"][kind].clone())
                .unwrap_or_else(|e| panic!("entry 55 names {kind}'s fields: {e}"));
            let carried: Vec<String> = payload
                .as_object()
                .expect("an object")
                .keys()
                .filter(|key| *key != "kind")
                .cloned()
                .collect();
            assert_eq!(carried, fields, "{kind}");
        }
        // And the two payload keys the record itself is read by.
        assert_eq!(block["held_payload"], json!("reasons"));
        assert_eq!(block["unheld_payload"], json!("released"));
        assert_eq!(
            serde_json::from_value::<Vec<String>>(block["event_kinds"].clone())
                .expect("entry 55 names its kinds"),
            vec![
                journal::PipelineKind::NodeHeld.as_str(),
                journal::PipelineKind::NodeUnheld.as_str()
            ]
        );
    }

    /// Two reasons at once are two entries of one record, and losing one of them
    /// leaves a record carrying only the other.
    #[test]
    fn a_hold_is_written_when_it_begins_when_it_changes_and_when_it_clears() {
        let root = std::env::temp_dir().join(format!("onepipeline-holds-{}", sys::pid()));
        let _ = std::fs::remove_dir_all(&root);
        let paths = RunPaths::under(&root, "demo");
        paths.create().expect("the run directory");
        let mut journal = Journal::open(&paths);
        let mut reported = BTreeMap::new();

        let both: BTreeMap<String, Vec<HoldReason>> = [(
            "ship".to_string(),
            vec![
                HoldReason::Dependencies {
                    blocking: vec!["build".into()],
                },
                HoldReason::Concurrency {
                    ahead: vec!["build".into()],
                    limit: 1,
                },
            ],
        )]
        .into();
        let one: BTreeMap<String, Vec<HoldReason>> = [(
            "ship".to_string(),
            vec![HoldReason::Concurrency {
                ahead: vec!["build".into()],
                limit: 1,
            }],
        )]
        .into();

        report_holds(&paths, &mut journal, &both, &mut reported).expect("reported");
        // Said once. A hold that has not changed says nothing on the passes an
        // arriving envelope or a settled sibling wakes the loop for.
        report_holds(&paths, &mut journal, &both, &mut reported).expect("reported");
        report_holds(&paths, &mut journal, &one, &mut reported).expect("reported");
        report_holds(&paths, &mut journal, &one, &mut reported).expect("reported");
        report_holds(&paths, &mut journal, &BTreeMap::new(), &mut reported).expect("reported");
        // And nothing at all for a node that was never held.
        report_holds(&paths, &mut journal, &BTreeMap::new(), &mut reported).expect("reported");

        let written = journal::read(&paths.journal());
        let kinds: Vec<String> = written.iter().map(|event| event.kind.0.clone()).collect();
        assert_eq!(
            kinds,
            vec![
                journal::PipelineKind::NodeHeld.as_str(),
                journal::PipelineKind::NodeHeld.as_str(),
                journal::PipelineKind::NodeUnheld.as_str(),
            ]
        );
        assert!(written
            .iter()
            .all(|event| event.labels.node.as_deref() == Some("ship")));
        assert_eq!(
            written[0].payload["reasons"],
            json!([
                { "kind": "dependencies", "blocking": ["build"] },
                { "kind": "concurrency", "ahead": ["build"], "limit": 1 },
            ]),
            "a node held two ways carries one entry per reason in one record"
        );
        assert_eq!(
            written[1].payload["reasons"],
            json!([{ "kind": "concurrency", "ahead": ["build"], "limit": 1 }]),
            "ceasing to be held one way leaves only the reason that remains"
        );
        assert_eq!(
            written[2].payload["released"],
            json!([{ "kind": "concurrency", "ahead": ["build"], "limit": 1 }])
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// What the loop is holding and why, over the shapes a reader has to be able
    /// to tell apart.
    #[test]
    fn a_node_no_stated_reason_holds_carries_nothing_and_the_rest_name_theirs() {
        let mut state = state_of(
            vec![
                agent("build", &[]),
                agent("ship", &["build"]),
                Node {
                    kind: NodeKind::Human,
                    ..agent("approve", &[])
                },
                agent("after", &["approve"]),
                agent("spare", &[]),
            ],
            &[("approve", NodeStatus::Waiting)],
        );
        state.graph.concurrency = 1;
        let statuses = state.statuses();
        // One dispatch in flight, which is the run's whole concurrency.
        let in_flight: BTreeMap<String, Dispatch> = [(
            "build".to_string(),
            Dispatch {
                node: agent("build", &[]),
                cancel: CancellationToken::new(),
                started: Instant::now(),
                last_progress: Instant::now(),
                reported_quiet: false,
                control: None,
            },
        )]
        .into();
        let decisions: BTreeMap<DecisionRef, Decision> = [(
            DecisionRef::Attestation("approve".into()),
            Decision {
                reference: DecisionRef::Attestation("approve".into()),
                kind: "attestation".into(),
                unblocks: vec!["after".into()],
            },
        )]
        .into();
        let awaiting: BTreeMap<String, Vec<String>> =
            [("spare".to_string(), vec!["build".to_string()])].into();

        let holds = holds_now(&state, &statuses, &in_flight, &decisions, &awaiting);

        // The node in flight is what the loop *is* running, and the human action
        // is waiting on a person rather than on this loop. Neither is held.
        assert_eq!(holds.get("build"), None);
        assert_eq!(holds.get("approve"), None);
        // Behind the one dispatch this run's concurrency allows, and behind the
        // dependency that dispatch is.
        assert_eq!(
            holds.get("ship"),
            Some(&vec![HoldReason::Dependencies {
                blocking: vec!["build".into()]
            }]),
            "a node whose dependency is running is held by the dependency"
        );
        // Two reasons at once: the attestation holds the subtree, and the
        // dependency it is has not settled `done`.
        assert_eq!(
            holds.get("after"),
            Some(&vec![
                HoldReason::Dependencies {
                    blocking: vec!["approve".into()]
                },
                HoldReason::Decision {
                    reference: DecisionRef::Attestation("approve".into())
                },
            ])
        );
        // Ready, nothing depends on it, and the run has no slot for it — plus
        // the release it adopts has not happened.
        assert_eq!(
            holds.get("spare"),
            Some(&vec![
                HoldReason::Concurrency {
                    ahead: vec!["build".into()],
                    limit: 1,
                },
                HoldReason::Release {
                    awaiting: vec!["build".into()]
                },
            ])
        );
    }

    /// A dependency in another run is named as the graph names it, and one a
    /// `drop` detached holds nothing.
    #[test]
    fn a_cross_dag_dependency_that_has_not_settled_is_named_as_the_plan_wrote_it() {
        let mut state = state_of(vec![agent("ship", &["run:other#build", "gone"])], &[]);
        let statuses = state.statuses();
        let holds = holds_now(
            &state,
            &statuses,
            &BTreeMap::new(),
            &BTreeMap::new(),
            &BTreeMap::new(),
        );
        assert_eq!(
            holds.get("ship"),
            Some(&vec![HoldReason::Dependencies {
                blocking: vec!["run:other#build".into()]
            }]),
            "a dependency the graph no longer holds was detached and holds nothing"
        );

        // And once that upstream settles, the loop is not holding it at all: it
        // is dispatchable and merely awaiting the pass that starts it.
        state
            .cross_dag
            .insert("run:other#build".into(), NodeStatus::Done);
        let statuses = state.statuses();
        assert_eq!(
            holds_now(
                &state,
                &statuses,
                &BTreeMap::new(),
                &BTreeMap::new(),
                &BTreeMap::new()
            )
            .get("ship"),
            None
        );
    }

    /// The loop's own pacing: paced work is due at once and then on its interval,
    /// and a wait with nothing to wait for is unbounded rather than a spin.
    #[test]
    fn paced_work_is_due_once_and_then_on_its_interval() {
        assert!(due(None, Duration::from_secs(1)), "the first pass does it");
        assert_eq!(until_due(None, Duration::from_secs(1)), Duration::ZERO);
        let now = Instant::now();
        assert!(!due(Some(now), Duration::from_secs(60)));
        assert!(until_due(Some(now), Duration::from_secs(60)) > Duration::from_secs(50));
        assert!(
            due(Some(now), Duration::ZERO),
            "a zero interval is always due"
        );

        // Nothing in flight is no deadline of the loop's own, so it waits on the
        // channel alone rather than on a timer it invented.
        assert_eq!(
            next_quiet(&BTreeMap::new(), Duration::from_secs(2_400)),
            Duration::MAX
        );
        let in_flight: BTreeMap<String, Dispatch> = [(
            "build".to_string(),
            Dispatch {
                node: agent("build", &[]),
                cancel: CancellationToken::new(),
                started: now,
                last_progress: now,
                reported_quiet: false,
                control: None,
            },
        )]
        .into();
        let next = next_quiet(&in_flight, Duration::from_secs(2_400));
        assert!(
            next > Duration::from_secs(2_000) && next <= Duration::from_secs(2_400),
            "{next:?}"
        );
    }

    #[test]
    fn the_stall_threshold_falls_back_when_the_environment_is_unusable() {
        // Read through the same helper the loop uses, so an unusable value
        // cannot silently disable the watch it configures.
        assert!(stall_after_seconds() > 0);
    }

    #[test]
    fn a_node_names_its_own_agent_graph_or_takes_the_shipped_default() {
        let pinned = oneagentgraph::config::ConfigRef("./custom.yaml".into());
        assert_eq!(node_graph(Some(&pinned), "default"), pinned);
        assert_eq!(node_graph(None, "default").0, "default");
    }

    #[test]
    fn only_the_commands_that_stop_a_dispatch_name_a_node_to_cancel() {
        assert_eq!(
            cancelled_by(&Command::Cancel {
                id: "a".into(),
                reason: None
            }),
            vec!["a".to_string()]
        );
        assert_eq!(
            cancelled_by(&Command::Drop {
                id: "a".into(),
                dependents: crate::channel::Dependents::Detach
            }),
            vec!["a".to_string()]
        );
        assert!(cancelled_by(&Command::Complete { reason: "r".into() }).is_empty());
    }

    #[test]
    fn dispatch_labels_carry_only_the_reserved_keys_and_never_a_round() {
        let labels = dispatch_labels("demo", "build", Some("implement"), Some("engineer"));
        assert_eq!(labels.run_id.as_deref(), Some("demo"));
        assert_eq!(labels.node.as_deref(), Some("build"));
        assert_eq!(labels.step.as_deref(), Some("implement"));
        assert_eq!(labels.round, None, "a dispatch was stamped with a round");
        assert!(labels.extra.is_empty());
    }
}
