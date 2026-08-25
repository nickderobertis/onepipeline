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
use std::num::NonZeroU32;
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::agentgraph::{self, Interrupted, TurnAddress};
use crate::channel::{ChannelState, Command, CommandOutcome, Deliver, Surface};
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

/// How often the reconcile loop wakes to drain edits and re-derive the frontier.
const POLL: Duration = Duration::from_millis(25);

/// The schema version a run result is written as.
///
/// `result.json` is a machine-read artifact this crate writes and **never reads
/// back**, so the number is a statement to its consumers rather than to a reader
/// here.
///
/// `4` is this document: `3` plus every node's [`cause`](NodeResult::cause) and
/// [`head`](NodeResult::head), the two a settlement carries when a dispatch ended
/// for a reason that is not the agent's verdict on its task. Both are omitted
/// when they are empty, so a `4` node states nothing extra to a consumer whose
/// nodes carry neither — but the number moves anyway, because the document now
/// states something a `3` reader has no field for. `3` was one result per run,
/// carrying no round and every node's [`landing`](NodeResult::landing). `2` and
/// `1` were the per-round `round-NN/result.json` — `1` unversioned and saying
/// only that a node had settled, `2` where a landing was first recorded — and
/// both named a round that continuous execution does not have.
pub const RUN_RESULT_SCHEMA_VERSION: u32 = 4;

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
    /// own documentation. See [`dispatch_death_cause`].
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
    /// The dispatch settled.
    Settled(Box<Settlement>),
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

    loop {
        reconcile_edits(paths, journal, state, &channel, &mut in_flight)?;

        // Another run's ledger is the only thing that can answer a cross-DAG
        // edge, and it is written by a process this one does not control — so
        // the answer is re-read on every reconcile pass rather than taken once.
        // This is also where an upstream that moved past what a consumer
        // recorded is noticed.
        state.cross_dag = upstreams.resolve(&state.graph, paths, journal)?;

        let statuses = state.statuses();
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
        releases.relay_releases(paths, journal, state, launch.filters.vcs.as_ref())?;
        // One hold, beside the decision points rather than in place of them: a
        // node a person is holding and a node a release is holding are both nodes
        // this pass does not start, and neither shortens the other's wait.
        paused.extend(held_for_release);
        adopt_releases(paths, journal, state, &mut releases, &in_flight)?;

        // Start what became actionable *before* asking whether the run is over.
        // A ready human action derives as `waiting`, which is a settled status —
        // so a check that ran first would call the graph terminal and leave that
        // settlement unrecorded, with nothing for a later `attest` to validate
        // against.
        start_ready(
            paths,
            journal,
            state,
            &rules,
            launch,
            &tx,
            &mut in_flight,
            &paused,
            &releases,
        )?;

        if in_flight.is_empty() {
            // Nothing is running and nothing became ready, so no further
            // message can arrive: the graph is as converged as it will get.
            let statuses = state.statuses();
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

        match rx.recv_timeout(POLL) {
            Ok(Message::Event(envelope)) => {
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
            Ok(Message::Redispatched(again)) => journal.emit(
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
            Ok(Message::Cancelling(step)) => raise(paths, journal, cancelling_surface(&step))?,
            // Emitted rather than relayed: it is this crate's own kind, so it
            // belongs in this crate's own stream, numbered by the writer that
            // owns it.
            Ok(Message::BodyNotDrafted(undrafted)) => journal.emit(
                journal::PipelineKind::BodyNotDrafted,
                journal::labels(&paths.run, Some(&undrafted.node)),
                journal::payload(&[
                    ("ending", json!(undrafted.ending.ending())),
                    ("detail", json!(undrafted.ending.why())),
                ]),
            )?,
            Ok(Message::Settled(settlement)) => {
                in_flight.remove(&settlement.node);
                settle(paths, journal, &settlement)?;
                *state = projection::fold(&journal::read(&paths.journal()));
                // A node that settled may have readied its dependents, and a
                // node that is ready again — a requeue, a retry — is announced
                // again.
                announced_ready
                    .retain(|id| state.statuses().get(id).copied() == Some(NodeStatus::Ready));
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }

        watch_for_quiet(paths, journal, stall_after, &mut in_flight)?;
    }

    Ok(graph::state_of(&state.statuses()))
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
/// A node that is `ready` has not started yet and one that is `running` has not
/// finished; either way the loop has something to wait for. Everything else is
/// settled or gated by something only the channel delivers.
fn any_node_can_still_move(statuses: &BTreeMap<String, NodeStatus>) -> bool {
    statuses
        .values()
        .any(|status| matches!(status, NodeStatus::Ready | NodeStatus::Running))
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
    in_flight: &mut BTreeMap<String, Dispatch>,
) -> Result<()> {
    for envelope in channel.claim_commands()? {
        let author = envelope.author;
        let mut applied = true;
        let mut reason = None;
        for command in &envelope.commands {
            let compiled = crate::channel::allows(author, command)
                .and_then(|()| compile_and_deliver(journal, state, command, in_flight));
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
                    // Two of the compiled operations are facts about the run
                    // rather than mutations of its graph, and a reader looking
                    // for either should not have to read the operation list.
                    // Each gets its own kind here too.
                    for operation in &operations {
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
                            _ => {}
                        }
                    }
                    // An edit the monitor made is the planner's to review: it
                    // was applied on the monitor's own judgement, so the planner
                    // learns of it without being asked to approve it first.
                    if author == crate::channel::Author::Monitor {
                        if let Some(surface) = monitor_edit(command) {
                            raise(paths, journal, surface)?;
                        }
                    }
                    *state = projection::fold(&journal::read(&paths.journal()));
                }
                Err(error) => {
                    applied = false;
                    reason = Some(error.to_string());
                    journal.emit(
                        journal::PipelineKind::EditRejected,
                        journal::labels(&paths.run, None),
                        journal::payload(&[
                            ("author", json!(author)),
                            ("command", json!(command)),
                            ("reason", json!(error.to_string())),
                        ]),
                    )?;
                    // Every rejection is also surfaced, so no accepted command
                    // is silently dropped.
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
                    )?;
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
    Ok(())
}

/// Validate one command, carry a `context` note into the running turn where its
/// mode asks for that, and compile what actually happened.
///
/// The order matters both ways. Validation first, because a note must not be
/// pushed into a live turn on behalf of an edit the reconciler is about to
/// refuse; delivery before the compile that is recorded, because *how* the note
/// reached the node is part of the mutation — a note the turn took is not also
/// owed to the next dispatch.
fn compile_and_deliver(
    journal: &mut Journal,
    state: &RunState,
    command: &Command,
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
        ..state.frontier()
    };
    let mut candidate = state.graph.clone();
    let operations = edits::compile(&mut candidate, &frontier, command)?;
    let Command::Context { id, note, deliver } = command else {
        return Ok(operations);
    };
    let delivery = deliver_note(journal, *deliver, id, note, in_flight)?;
    if delivery == edits::Delivery::Deferred {
        return Ok(operations);
    }
    let mut candidate = state.graph.clone();
    edits::compile_with(&mut candidate, &frontier, command, delivery)
}

/// Carry one note into a node's running turn, as far as its mode asks.
///
/// `oneagentgraph interrupt`'s exit 3 — no controllable turn in flight — is the
/// answer this is built around, and it is a **fact** rather than a failure: it
/// is what `auto` falls through on and what `live` refuses on. A delivery that
/// was attempted and *broke* is neither, and is refused under both modes: a
/// planner told `deferred` when the truth is that the lever failed has been told
/// something that is not so.
fn deliver_note(
    journal: &mut Journal,
    deliver: Deliver,
    id: &str,
    note: &str,
    in_flight: &BTreeMap<String, Dispatch>,
) -> Result<edits::Delivery> {
    if deliver == Deliver::Next {
        return Ok(edits::Delivery::Deferred);
    }
    let Some(address) = in_flight
        .get(id)
        .and_then(|dispatch| dispatch.control.clone())
    else {
        return not_live(
            deliver,
            id,
            "it has no turn this run can address: nothing of its dispatch has \
             reported a member yet, or it has no dispatch at all",
        );
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
        Interrupted::NoTurn(reason) => not_live(deliver, id, &reason),
        Interrupted::Failed(reason) => Err(Error::Refused(format!(
            "context: delivering the note to node '{id}' failed: {reason}"
        ))),
    }
}

/// What a mode makes of a note that could not go into a running turn.
fn not_live(deliver: Deliver, id: &str, reason: &str) -> Result<edits::Delivery> {
    match deliver {
        Deliver::Live => Err(Error::Refused(format!(
            "context: node '{id}' has no controllable turn in flight, so the note \
             cannot be delivered live: {reason}"
        ))),
        // `auto` and `next` both mean the note rides the next dispatch, which is
        // exactly what a `context` edit has always done.
        Deliver::Auto | Deliver::Next => Ok(edits::Delivery::Deferred),
    }
}

/// Tell every still-running fast-adoption node whose awaited releases have all
/// arrived, exactly once.
///
/// Delivery is the `context` mechanism a planner's own note uses, at `auto`: into
/// the node's running turn where it has a controllable one, and onto its next
/// dispatch where it does not. `release-adopted` is the durable record of both —
/// which is what makes the note deliver once across a driver's death, and what
/// the fold reattaches a deferred note from.
///
/// The note is not an edit: nobody submitted it and no author owns it, so it is
/// recorded under its own kind rather than as an `edit-committed` attributed to a
/// planner who never wrote it.
fn adopt_releases(
    paths: &RunPaths,
    journal: &mut Journal,
    state: &mut RunState,
    releases: &mut crate::release::Watch,
    in_flight: &BTreeMap<String, Dispatch>,
) -> Result<()> {
    let running: Vec<Node> = in_flight
        .values()
        .map(|dispatch| dispatch.node.clone())
        .collect();
    let ready = releases.ready_to_adopt(&running);
    if ready.is_empty() {
        return Ok(());
    }
    for (node, released) in ready {
        let note = crate::release::arrival_note(&released);
        // Whatever the lever answered, the node has been told: `auto` falls
        // through to the next dispatch where there is no controllable turn, and
        // a delivery that was *attempted and broke* is the one case that leaves
        // the note owed — so that one is not recorded and is tried again.
        let delivery = match deliver_note(journal, Deliver::Auto, &node, &note, in_flight) {
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
    Ok(())
}

/// The nodes whose in-flight dispatch a command stops.
fn cancelled_by(command: &Command) -> Vec<String> {
    match command {
        Command::Drop { id, .. } | Command::Retry { id, .. } | Command::Cancel { id } => {
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
    rules: &ExecutorRules,
    launch: &LaunchRecord,
    tx: &Sender<Message>,
    in_flight: &mut BTreeMap<String, Dispatch>,
    paused: &BTreeSet<String>,
    releases: &crate::release::Watch,
) -> Result<()> {
    let statuses = state.statuses();
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
    Ok(())
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
    let mut asked_at: Option<Instant> = None;
    let mut killed = false;
    loop {
        match arriving.recv_timeout(POLL) {
            Ok(Ok(envelope)) => {
                spoke = true;
                if let Some(address) = addressed_by(&envelope) {
                    if !addresses.contains(&address) {
                        addresses.push(address);
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
        Ok(outcome) => failed_task(node, &outcome, session.as_ref()),
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
/// re-run work that is waiting to be read. Failing that, a detail that classifies
/// itself settles [`DISPATCH_DIED`]; the branch is carried, never consulted, so a
/// dispatch that died holding finished work and one whose workspace disappeared
/// reach the same word.
///
/// Every unknown degrades to the plain failure this arm always produced.
fn failed_task(
    node: &str,
    outcome: &DispatchOutcome,
    session: Option<&onevcs::SessionToken>,
) -> Settlement {
    let detail = (!outcome.detail.is_empty()).then(|| outcome.detail.clone());
    if let Some(url) = session.and_then(crate::vcs::change_opened_in) {
        return Settlement {
            detail,
            change_url: Some(url),
            ..failed(node, TASK_FAILED_CHANGE_OPEN)
        };
    }
    let Some(cause) = dispatch_death_cause(&outcome.detail) else {
        return Settlement {
            detail,
            ..failed(node, TASK_FAILED)
        };
    };
    Settlement {
        detail,
        cause: Some(cause),
        head: session.and_then(crate::vcs::branch_head_in),
        ..failed(node, DISPATCH_DIED)
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
/// Held to that ceiling on the way in as well as on the way up, because this is a
/// value a *run* waits out: an operator's stray zero, or a value meant as
/// milliseconds, would otherwise hold a node open for as long as the number says
/// while a host that answered in a second sat there answering. The ceiling is the
/// same one every backoff in this crate doubles to, so nothing here waits longer
/// than anything else does.
pub(crate) fn merge_path_backoff() -> Duration {
    Duration::from_secs(
        std::env::var(MERGE_PATH_BACKOFF_ENV)
            .ok()
            .and_then(|value| value.parse().ok())
            .unwrap_or(DEFAULT_MERGE_PATH_BACKOFF_SECONDS),
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
    let nodes = state
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
                landing: state.landings.get(&node.id).copied(),
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
            }
        })
        .collect();

    let result = RunResult {
        run_id: paths.run.clone(),
        state: settled,
        nodes,
    };
    ledger::write_json(&paths.result(), &result)?;
    Ok(result)
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
        for unusable in ["", "not a number", "-1", "5.5"] {
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
    const SETTLEMENT_OUTCOMES: [&str; 7] = [
        INVALID_NODE,
        NO_CHANGES,
        INFRASTRUCTURE_FAILURE,
        NO_AGENT_PROGRESS,
        TASK_FAILED,
        TASK_FAILED_CHANGE_OPEN,
        DISPATCH_DIED,
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

    /// The checked-in shape of a schema-4 run result.
    const RUN_RESULT_GOLDEN: &str = include_str!("../tests/golden/run-result-v4.json");

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
        }
    }

    /// The document the golden pins, built through the types.
    ///
    /// Four nodes because each pins a case the wire has and the others do not.
    /// The landing has three — a change observed on its base, one that had not
    /// reached it, and a node with no change of its own, which carries no
    /// `landing` key at all — and a golden carrying one of them would pin a third
    /// of that change. The fourth is a dispatch that died: the one node carrying a
    /// `cause` and a `head`, which is what schema `4` added and what every other
    /// node here omits.
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
            ],
        }
    }

    /// The shape a run result is written as, pinned to the checked-in golden.
    #[test]
    fn a_schema_4_run_result_is_the_shape_the_golden_pins() {
        let rendered = serde_json::to_string_pretty(&run_result_golden()).expect("it serialises");
        assert_eq!(
            rendered.trim(),
            RUN_RESULT_GOLDEN.trim(),
            "the run result changed shape. If that was deliberate, bump \
             RUN_RESULT_SCHEMA_VERSION and update tests/golden/run-result-v4.json together"
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
    fn a_schema_4_run_result_round_trips_and_omits_what_it_does_not_have() {
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
    }

    /// The version is a decision, not an accident: it moves when the shape does,
    /// and the golden is named for the one it pins.
    #[test]
    fn the_run_result_schema_version_and_the_golden_name_the_same_number() {
        assert_eq!(RUN_RESULT_SCHEMA_VERSION, 4);
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

        // Above is a build that knows more than this one. `3` is this document
        // before it carried a cause, `2` and `1` the per-round document that shape
        // replaced, and `0` a number this crate has never written, so each came
        // from somewhere that is not this contract.
        for outside in [RUN_RESULT_SCHEMA_VERSION + 1, 3, 2, 1, 0] {
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
            cancelled_by(&Command::Cancel { id: "a".into() }),
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
