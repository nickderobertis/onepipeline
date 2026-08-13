//! The round engine: the single writer that converges a round's frontier, and
//! the transition that derives the next round from the graph this one executed.
//!
//! Execution is a long-lived reconcile loop rather than a stop-the-world barrier.
//! It compares the round's live desired graph with the node state projected from
//! the journal, starts the reachable frontier, and reacts to each completion
//! until the graph is terminal — draining the planner's durable command queue on
//! every pass, so a live edit takes effect without waiting for the round to
//! settle.
//!
//! Everything here runs under the run's ownership lock. The engine verbs are the
//! only writers of the graph, the journal, and the round ledger; a second writer
//! would interleave with this loop and corrupt the ledger.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::agentgraph::{self, Interrupted, TurnAddress};
use crate::channel::{ChannelState, Command, CommandOutcome, Deliver, Surface};
use crate::edits;
use crate::error::{Error, Result};
use crate::event::{Envelope, Labels};
use crate::executor::{
    CancelMode, CancellationToken, DispatchHandle, DispatchRequest, Executor, WorkspaceSpec,
};
use crate::graph::{self, Graph, GraphState, NodeStatus};
use crate::journal::{self, Journal};
use crate::ledger::{self, LaunchRecord, OwnershipLock, RunPaths};
use crate::plan::{Node, NodeKind, Plan};
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

/// The environment variable overriding how long a dispatch may record nothing
/// before the round surfaces a quiet-worker proposal.
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

/// How often the reconcile loop wakes to drain edits and re-derive the frontier.
const POLL: Duration = Duration::from_millis(25);

/// One round's recorded result.
///
/// `ok` is on the wire but not on the type: it is `state == complete` and
/// nothing else, so storing it would let a result claim a failed round
/// succeeded. It is derived on the way out and re-derived on the way in, which
/// is also what makes a hand-edited result file impossible to disagree with
/// itself.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(into = "RoundResultWire", from = "RoundResultWire")]
pub struct RoundResult {
    /// The run.
    pub run_id: String,
    /// The round within it.
    pub round: u64,
    /// How the graph settled.
    pub state: GraphState,
    /// Every node's settled status, in the order the plan wrote them.
    pub nodes: Vec<NodeResult>,
}

impl RoundResult {
    /// True only for `complete`, as the recorded result renders it.
    pub fn ok(&self) -> bool {
        self.state == GraphState::Complete
    }
}

/// The shape a round result is written and read as.
// llmlint: ignore-block[invalid_states_unrepresentable] `ok` beside `state` is the wire's
// shape, not a state this crate can hold. The type is private, its only constructor is the
// `From<RoundResult>` below — which computes `ok` from `state` — and the `From` back drops
// the field, so a file claiming `state: failed, ok: true` is normalised rather than
// believed. Removing `ok` from the wire is a different change: `round-NN/result.json` is a
// machine-read artifact whose consumers filter on it, so it would need a schema version and
// a golden. Raise that with the planner who owns the contract.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RoundResultWire {
    run_id: String,
    round: u64,
    state: GraphState,
    ok: bool,
    nodes: Vec<NodeResult>,
}

// llmlint: ignore-end[invalid_states_unrepresentable]

impl From<RoundResult> for RoundResultWire {
    fn from(result: RoundResult) -> Self {
        Self {
            ok: result.ok(),
            run_id: result.run_id,
            round: result.round,
            state: result.state,
            nodes: result.nodes,
        }
    }
}

impl From<RoundResultWire> for RoundResult {
    fn from(wire: RoundResultWire) -> Self {
        Self {
            run_id: wire.run_id,
            round: wire.round,
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
}

/// How one node settled, as its dispatch reports it.
///
// llmlint: ignore-block[invalid_states_unrepresentable] `outcome`, `branch`, and
// `change_url` are optional strings because they are exactly what goes into the journal
// payload, and the journal is read by builds other than this one. An outcome enum here
// would make a record written by a newer build unreadable by an older one, which is the
// failure the schema-skipping rule elsewhere in this crate exists to prevent. `status` is
// the part that *is* narrowed, because scheduling depends on it.
#[derive(Debug, Clone, PartialEq)]
pub struct Settlement {
    /// The node.
    pub node: String,
    /// Its terminal status.
    pub status: NodeStatus,
    /// The named outcome, when it had one.
    pub outcome: Option<String>,
    /// The failure's own words.
    pub detail: Option<String>,
    /// The branch a lifecycle node left behind.
    pub branch: Option<String>,
    /// Where a human reads the change it published.
    pub change_url: Option<String>,
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
            detail: None,
            branch: None,
            change_url: None,
            completed_steps: Vec::new(),
        }
    }
}

/// What a dispatch thread sends back to the single writer.
pub(crate) enum Message {
    /// One envelope, relayed from wherever the dispatch ran.
    Event(Box<Envelope>),
    /// A dispatch that produced nothing is being asked again.
    Retried(Box<BoundaryRetry>),
    /// The dispatch settled.
    Settled(Box<Settlement>),
}

/// Everything a dispatch needs, resolved before it leaves the writer's thread.
struct Dispatch {
    node: Node,
    cancel: CancellationToken,
    started: Instant,
    /// When this dispatch last recorded anything, for the quiet-worker watch.
    last_activity: Instant,
    /// Whether it has already been reported quiet in this quiet stretch. A
    /// worker that wakes up, works, and goes quiet again is reported again; one
    /// that simply stays quiet is not repeated.
    reported_quiet: bool,
    /// Where this dispatch's in-flight turn is addressed, once its stream has
    /// said. `None` until then, which is the same answer as a turn there is no
    /// lever for: a `context` note has nothing to be delivered into.
    control: Option<TurnAddress>,
}

/// Execute the run's open round, opening one if none is.
///
/// Returns the round's settled state, whose exit code the binary carries: 0 for
/// `complete`, 1 for `waiting` or `failed`.
pub fn round_run(paths: &RunPaths) -> Result<GraphState> {
    let lock = OwnershipLock::acquire(paths, "round run")?;
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

    if !state.round_open {
        let round = state.round + 1;
        // A round's launch record is written once and never rewritten: the
        // transition folds the journal instead, so this stays the record of
        // what the round *started* with.
        let plan = plan_of(&state);
        ledger::write_json(&paths.round_plan(round), &plan)?;
        journal.emit(
            journal::PipelineKind::RoundStarted,
            journal::labels(&paths.run, Some(round), None),
            journal::payload(&[("plan", json!(plan))]),
        )?;
        state = projection::fold(&journal::read(&paths.journal()));
    }

    let round = state.round;
    let outcome = converge(paths, &mut journal, &mut state, round, &launch)?;
    let result = record_result(paths, &mut journal, &state, round, outcome)?;
    println!(
        "{}",
        serde_json::to_string(&result).map_err(|e| Error::Invalid(format!("result: {e}")))?
    );
    lock.release();
    Ok(outcome)
}

fn plan_of(state: &RunState) -> Plan {
    let template = state.plan.clone().unwrap_or(Plan {
        schema_version: crate::plan::PLAN_SCHEMA_VERSION,
        goal: None,
        name: None,
        concurrency: state.graph.concurrency,
        tasks: Vec::new(),
    });
    state.graph.to_plan(&template)
}

/// The reconcile loop: converge the actual frontier toward the desired graph.
fn converge(
    paths: &RunPaths,
    journal: &mut Journal,
    state: &mut RunState,
    round: u64,
    launch: &LaunchRecord,
) -> Result<GraphState> {
    let channel = ChannelState::new(paths);
    let rules = executor_rules()?;
    let (tx, rx): (Sender<Message>, Receiver<Message>) = mpsc::channel();
    let mut in_flight: BTreeMap<String, Dispatch> = BTreeMap::new();
    let started = Instant::now();
    let budget = Duration::from_secs(launch.round_budget);
    let stall_after = Duration::from_secs(stall_after_seconds());
    let mut budget_spent = false;
    let mut upstreams = crate::crossdag::Observer::of_run(paths, state);

    loop {
        reconcile_edits(paths, journal, state, &channel, &mut in_flight)?;

        // Another run's ledger is the only thing that can answer a cross-DAG
        // edge, and it is written by a process this one does not control — so
        // the answer is re-read on every pass rather than taken once. This is
        // also where an upstream that moved past what a consumer recorded is
        // noticed, which cannot happen at any single moment in the round.
        state.cross_dag = upstreams.resolve(&state.graph, paths, round, journal)?;

        if !budget_spent && started.elapsed() > budget {
            budget_spent = true;
            // Cooperatively cancel in flight work and surface a blocking
            // proposal, so a wedged dispatch layer cannot leave the planner
            // channel silent.
            for dispatch in in_flight.values() {
                dispatch.cancel.cancel();
            }
            journal.emit(
                journal::PipelineKind::RoundBudgetExceeded,
                journal::labels(&paths.run, Some(round), None),
                journal::payload(&[("budget_seconds", json!(launch.round_budget))]),
            )?;
            raise(
                paths,
                journal,
                round,
                Surface {
                    id: 0,
                    kind: "round-budget".into(),
                    message: format!(
                        "round {round} exceeded its {}s budget; in-flight work was cancelled \
                         cooperatively. Decide whether to retry, park, or raise the budget.",
                        launch.round_budget
                    ),
                    source: crate::channel::source::PROPOSAL.into(),
                    blocking: true,
                    round,
                    queued_at: sys::now_millis(),
                    workstream: None,
                },
            )?;
        }

        // Start what became actionable *before* asking whether the round is
        // over. A ready human action derives as `waiting`, which is a settled
        // status — so a check that ran first would call the round terminal and
        // leave that settlement unrecorded, with nothing for a later `attest`
        // to validate against.
        if !budget_spent {
            start_ready(
                paths,
                journal,
                state,
                round,
                &rules,
                &launch.node_graph,
                &tx,
                &mut in_flight,
            )?;
        }

        if in_flight.is_empty() {
            // Nothing is running and nothing became ready, so no further
            // message can arrive: the graph is as converged as it will get.
            let statuses = state.statuses();
            if graph::is_terminal(&statuses) || budget_spent {
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
                        dispatch.last_activity = Instant::now();
                        dispatch.reported_quiet = false;
                        if let Some(address) = addressed_by(&envelope) {
                            dispatch.control = Some(address);
                        }
                    }
                }
                journal.relay(&envelope)?;
            }
            // Every retry reaches the journal, so a retry that saved a run is
            // visible in the run's own record rather than only in a log.
            Ok(Message::Retried(retry)) => journal.emit(
                journal::PipelineKind::BoundaryRetried,
                journal::labels(&paths.run, Some(round), Some(&retry.node)),
                journal::payload(&[
                    ("role", json!(retry.role.as_str())),
                    ("attempt", json!(retry.attempt)),
                    ("attempts", json!(retry.attempts)),
                    ("backoff_seconds", json!(retry.backoff_seconds)),
                    ("reason", json!(bounded(&retry.reason))),
                ]),
            )?,
            Ok(Message::Settled(settlement)) => {
                in_flight.remove(&settlement.node);
                settle(paths, journal, round, &settlement)?;
                *state = projection::fold(&journal::read(&paths.journal()));
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }

        watch_for_quiet(paths, journal, round, stall_after, &mut in_flight)?;
    }

    Ok(graph::state_of(&state.statuses()))
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
fn reconcile_edits(
    paths: &RunPaths,
    journal: &mut Journal,
    state: &mut RunState,
    channel: &ChannelState,
    in_flight: &mut BTreeMap<String, Dispatch>,
) -> Result<()> {
    for envelope in channel.claim_commands()? {
        let mut applied = true;
        let mut reason = None;
        for command in &envelope.commands {
            match compile_and_deliver(journal, state, command, in_flight) {
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
                        journal::labels(&paths.run, Some(state.round), None),
                        journal::payload(&[
                            ("command", json!(command)),
                            ("operations", json!(operations)),
                        ]),
                    )?;
                    // Two of the compiled operations are facts about the run
                    // rather than mutations of its graph, and a reader looking
                    // for either should not have to know whether a round was
                    // live when it arrived. Each gets its own kind here too.
                    for operation in &operations {
                        match operation {
                            edits::Operation::CompletionRequested { reason } => journal.emit(
                                journal::PipelineKind::CompletionRequested,
                                journal::labels(&paths.run, Some(state.round), None),
                                journal::payload(&[("reason", json!(reason))]),
                            )?,
                            edits::Operation::HumanAttested { node } => journal.emit(
                                journal::PipelineKind::HumanAttested,
                                journal::labels(&paths.run, Some(state.round), Some(node)),
                                journal::payload(&[("ref", json!(node))]),
                            )?,
                            _ => {}
                        }
                    }
                    *state = projection::fold(&journal::read(&paths.journal()));
                }
                Err(error) => {
                    applied = false;
                    reason = Some(error.to_string());
                    journal.emit(
                        journal::PipelineKind::EditRejected,
                        journal::labels(&paths.run, Some(state.round), None),
                        journal::payload(&[
                            ("command", json!(command)),
                            ("reason", json!(error.to_string())),
                        ]),
                    )?;
                    // Every rejection is also surfaced, so no accepted command
                    // is silently dropped.
                    raise(
                        paths,
                        journal,
                        state.round,
                        Surface {
                            id: 0,
                            kind: "edit-rejected".into(),
                            message: format!("reconciler: rejected — {error}"),
                            source: crate::channel::source::RECONCILER.into(),
                            blocking: false,
                            round: state.round,
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
    let frontier = state.frontier();
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
    round: u64,
    rules: &ExecutorRules,
    node_graph: &str,
    tx: &Sender<Message>,
    in_flight: &mut BTreeMap<String, Dispatch>,
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
                round,
                &Settlement::plain(&node.id, NodeStatus::Done, Some("no-changes")),
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
                round,
                &Settlement::plain(&node.id, NodeStatus::Waiting, None),
            )?;
            settled_here = true;
            continue;
        }

        let cancel = CancellationToken::new();
        journal.emit(
            journal::PipelineKind::NodeDispatched,
            journal::labels(&paths.run, Some(round), Some(&node.id)),
            journal::payload(&[("persona", json!(node.persona))]),
        )?;
        spawn(
            paths,
            round,
            rules,
            node_graph,
            &node,
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
                last_activity: now,
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
fn spawn(
    paths: &RunPaths,
    round: u64,
    rules: &ExecutorRules,
    node_graph: &str,
    node: &Node,
    cancel: CancellationToken,
    tx: Sender<Message>,
) -> Result<()> {
    // The labels a `node_label` rule selects on. An executor is chosen once per
    // node, before its steps run, so a node's own labels are what exists here.
    let labels = dispatch_labels(&paths.run, round, &node.id, None, node.persona.as_deref());
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
    let node_graph = node_graph.to_string();
    std::thread::Builder::new()
        .name(format!("dispatch-{}", node.id))
        .spawn(move || {
            let executor = crate::rules::executor_for(&entry);
            let settlement = if node.repo.is_some() {
                crate::lifecycle::execute(
                    executor.as_ref(),
                    &run,
                    round,
                    &node_graph,
                    &node,
                    &cancel,
                    &tx,
                )
            } else {
                execute_direct(
                    executor.as_ref(),
                    &run,
                    round,
                    &node_graph,
                    &node,
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
    round: u64,
    default_graph: &str,
    node: &Node,
    cancel: &CancellationToken,
    tx: &Sender<Message>,
) -> Settlement {
    let graph = node_graph(node.agent_graph.as_ref(), default_graph);
    let request = || DispatchRequest {
        graph: graph.clone(),
        task: node.rendered_task(),
        labels: dispatch_labels(run, round, &node.id, None, node.persona.as_deref()),
        workspace: WorkspaceSpec::Path(project_dir()),
        cancel: cancel.clone(),
    };
    attempt(executor, &node.id, Role::Worker, cancel, tx, &request).settlement
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
    pub session: Option<String>,
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
    role: Role,
    cancel: &CancellationToken,
    tx: &Sender<Message>,
    request: &dyn Fn() -> DispatchRequest,
) -> Drained {
    let attempts = boundary_attempts();
    let mut backoff = Duration::from_secs(boundary_backoff_seconds());
    let mut last = Drained {
        settlement: failed(node, "infrastructure-failure"),
        reached: Reached::NotStarted,
        session: None,
        branch: None,
    };

    for attempt in 1..=attempts {
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
                    ..failed(node, "infrastructure-failure")
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
        if attempt == attempts {
            // The budget was spent without the agent producing anything.
            // Reported apart from an ordinary task failure because retrying
            // this one unchanged spends the next budget the same way — and
            // apart from a dispatch that never started, which failed for a
            // reason that has nothing to do with the agent.
            if last.reached != Reached::NotStarted {
                last.settlement = Settlement {
                    detail: last.settlement.detail.clone(),
                    ..failed(node, "no-agent-progress")
                };
            }
            break;
        }
        let _ = tx.send(Message::Retried(Box::new(BoundaryRetry {
            node: node.to_string(),
            role,
            attempt,
            attempts,
            backoff_seconds: backoff.as_secs(),
            reason: last.settlement.detail.clone().unwrap_or_default(),
        })));
        std::thread::sleep(backoff);
        backoff = (backoff * 2).min(BOUNDARY_BACKOFF_CEILING);
    }
    last
}

/// Which side of a node's dispatch was retried.
///
/// One variant, because the boundary retry guards exactly one side today: a
/// dispatch that produced nothing. The `pr-author` draft is off the publication
/// path and falls back deterministically rather than being asked again, and a
/// judge side is the sibling's to retry. The enum is here rather than a string
/// so the journal's word has one source and a second side is additive.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Role {
    /// The node's own work.
    Worker,
}

impl Role {
    /// The word the journal records this role as.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Worker => "worker",
        }
    }
}

/// One retried attempt, as the journal records it.
pub(crate) struct BoundaryRetry {
    /// The node whose dispatch was asked again.
    pub node: String,
    /// Which side was retried.
    pub role: Role,
    /// Which attempt this follows.
    pub attempt: u32,
    /// How many attempts the budget allows.
    pub attempts: u32,
    /// How long the next attempt waits.
    pub backoff_seconds: u64,
    /// A bounded reason, as the failing attempt reported it.
    pub reason: String,
}

/// Relay a dispatch's events into the merged stream and settle on its outcome.
///
/// Reports whether the dispatch said anything at all, which is what decides
/// whether asking again could produce a different answer.
pub(crate) fn drain(
    handle: &mut dyn DispatchHandle,
    tx: &Sender<Message>,
    node: &str,
    cancel: &CancellationToken,
) -> Drained {
    let mut cancelled = false;
    let mut spoke = false;
    for envelope in handle.events() {
        if let Ok(envelope) = envelope {
            spoke = true;
            let _ = tx.send(Message::Event(Box::new(envelope)));
        }
        if !cancelled && cancel.is_cancelled() {
            cancelled = true;
            // Cooperative: the dispatch is asked to stop and preserve its work,
            // which killing the process would not.
            handle.cancel(CancelMode::Cooperative);
        }
    }
    let waited = handle.wait();
    let (session, branch) = match &waited {
        Ok(outcome) => (outcome.session.clone(), outcome.branch.clone()),
        Err(_) => (None, None),
    };
    let settlement = match waited {
        Ok(outcome) if outcome.succeeded && !cancel.is_cancelled() => {
            Settlement::plain(node, NodeStatus::Done, None)
        }
        Ok(_) if cancel.is_cancelled() => Settlement::plain(node, NodeStatus::Cancelled, None),
        Ok(outcome) => Settlement {
            detail: (!outcome.detail.is_empty()).then_some(outcome.detail),
            ..failed(node, "task-failed")
        },
        Err(error) => Settlement {
            detail: Some(error.to_string()),
            ..failed(node, "infrastructure-failure")
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

/// How many attempts a dispatch that produced nothing gets.
///
/// An unusable value falls back to the default rather than disabling the
/// recovery it configures.
fn boundary_attempts() -> u32 {
    std::env::var(BOUNDARY_ATTEMPTS_ENV)
        .ok()
        .and_then(|value| value.parse().ok())
        .filter(|attempts| *attempts > 0)
        .unwrap_or(DEFAULT_BOUNDARY_ATTEMPTS)
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
fn bounded(reason: &str) -> String {
    reason
        .chars()
        .take(crate::event::MAX_PAYLOAD_TEXT_BYTES / 4)
        .collect()
}

/// The labels a dispatch is stamped with. The reserved keys, and nothing else.
pub(crate) fn dispatch_labels(
    run: &str,
    round: u64,
    node: &str,
    step: Option<&str>,
    persona: Option<&str>,
) -> Labels {
    Labels {
        run_id: Some(run.to_string()),
        round: Some(round),
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
fn settle(
    paths: &RunPaths,
    journal: &mut Journal,
    round: u64,
    settlement: &Settlement,
) -> Result<()> {
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
    if !settlement.completed_steps.is_empty() {
        payload.insert("completed_steps".into(), json!(settlement.completed_steps));
    }
    journal.emit(
        journal::PipelineKind::NodeSettled,
        journal::labels(&paths.run, Some(round), Some(&settlement.node)),
        payload,
    )
}

/// Surface something to the planner, recording that it was *sent*.
fn raise(paths: &RunPaths, journal: &mut Journal, round: u64, surface: Surface) -> Result<()> {
    let queued = ChannelState::new(paths).push(surface)?;
    journal.emit(
        journal::PipelineKind::PlannerSurfaceQueued,
        journal::labels(&paths.run, Some(round), queued.workstream.as_deref()),
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
/// surface would stop the round's other workers to ask.
fn watch_for_quiet(
    paths: &RunPaths,
    journal: &mut Journal,
    round: u64,
    stall_after: Duration,
    in_flight: &mut BTreeMap<String, Dispatch>,
) -> Result<()> {
    let quiet: Vec<(String, u64, bool, String)> = in_flight
        .iter()
        .filter(|(_, dispatch)| !dispatch.reported_quiet)
        .filter(|(_, dispatch)| dispatch.last_activity.elapsed() > stall_after)
        .map(|(id, dispatch)| {
            (
                id.clone(),
                dispatch.last_activity.elapsed().as_secs(),
                dispatch.last_activity == dispatch.started,
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
            journal::labels(&paths.run, Some(round), Some(&node)),
            journal::payload(&[
                ("quiet_for_seconds", json!(quiet_for)),
                ("threshold_seconds", json!(stall_after.as_secs())),
                ("persona", json!(persona)),
            ]),
        )?;
        raise(
            paths,
            journal,
            round,
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
                round,
                queued_at: sys::now_millis(),
                workstream: Some(node.clone()),
            },
        )?;
    }
    Ok(())
}

/// Write the round's result and close it.
fn record_result(
    paths: &RunPaths,
    journal: &mut Journal,
    state: &RunState,
    round: u64,
    settled: GraphState,
) -> Result<RoundResult> {
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
            }
        })
        .collect();

    let result = RoundResult {
        run_id: paths.run.clone(),
        round,
        state: settled,
        nodes,
    };
    ledger::write_json(&paths.round_result(round), &result)?;
    journal.emit(
        journal::PipelineKind::RoundFinished,
        journal::labels(&paths.run, Some(round), None),
        journal::payload(&[("state", json!(result.state)), ("ok", json!(result.ok()))]),
    )?;
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

/// Transition to the next round.
///
/// The next round is derived from the graph the last round **executed**, folded
/// from its own journal — not from its launch record, which every live edit the
/// reconciler committed is absent from.
pub fn round_next(paths: &RunPaths) -> Result<Option<u64>> {
    let lock = OwnershipLock::acquire(paths, "round next")?;
    let mut journal = Journal::open(paths);
    let state = projection::fold(&journal::read(&paths.journal()));
    if state.round_open {
        return Err(Error::Refused(format!(
            "run '{}' is still executing round {}",
            paths.run, state.round
        )));
    }

    // A record this build cannot read might have been an authoritative graph
    // mutation, so a transition that meets one reports rather than folding a
    // graph it knows is incomplete.
    let strict = state.strict && !journal::has_unreadable_lines(&paths.journal());
    let source = if strict {
        state.clone()
    } else {
        // A journal that cannot be folded strictly falls back to the launch
        // record, and says so, rather than deriving from a graph it knows is
        // incomplete.
        eprintln!(
            "onepipeline: run '{}' has a journal record this build cannot read; \
             deriving round {} from the launch record instead of the executed graph.",
            paths.run,
            state.round + 1
        );
        let plan: Plan = ledger::read_json(&paths.round_plan(state.round))?;
        RunState {
            graph: Graph::from_plan(&plan),
            plan: Some(plan),
            ..state.clone()
        }
    };

    let next = derive_next(&source);
    if next.is_empty() {
        println!(
            "{}",
            json!({"run_id": paths.run, "state": "complete", "next_round": null})
        );
        lock.release();
        return Ok(None);
    }

    // A round nothing could start is not a round. Every remaining node is
    // gated by something only a person or a planner can clear — a waiting
    // human, a parked node, an unresolved upstream — so opening one would
    // dispatch nothing, settle identically, and do it again forever. The run
    // waits instead, which is the state `results` and `status` already report.
    // Resolved the same way the round would, and for the same reason: a next
    // round whose only startable work is gated by an upstream that has *already*
    // arrived would otherwise be judged empty, and the run would park on work it
    // could start immediately. Reading only — the transition records the round it
    // opens, and an edge's own evidence belongs to the round that acts on it.
    let upstreams = crate::crossdag::resolve_quietly(
        &paths
            .dir
            .parent()
            .map_or_else(ledger::runs_root, std::path::Path::to_path_buf),
        &next,
    );
    let ready = crate::graph::derive(&next, &BTreeMap::new(), &|dependency| {
        upstreams.get(dependency).copied()
    });
    if !ready.values().any(|status| *status == NodeStatus::Ready) {
        println!(
            "{}",
            json!({
                "run_id": paths.run,
                "state": graph::state_of(&ready).as_str(),
                "next_round": null,
            })
        );
        lock.release();
        return Ok(None);
    }

    let round = state.round + 1;
    let plan = next.to_plan(&plan_of(&source));
    graph::validate(&plan)?;
    ledger::write_json(&paths.round_plan(round), &plan)?;
    journal.emit(
        journal::PipelineKind::RoundStarted,
        journal::labels(&paths.run, Some(round), None),
        journal::payload(&[("plan", json!(plan))]),
    )?;
    println!(
        "{}",
        json!({"run_id": paths.run, "state": "continuing", "next_round": round})
    );
    lock.release();
    Ok(Some(round))
}

/// The graph the next round executes.
pub(crate) fn derive_next(state: &RunState) -> Graph {
    let statuses = state.statuses();
    let carried: BTreeSet<String> = state
        .graph
        .ids()
        // A `done` node is never rescheduled. Everything else carries,
        // including a parked node: its flag is what stops the next round
        // dispatching it, and its preserved checkpoint is what a later
        // `requeue` has to pick up rather than cutting a fresh branch beside it.
        .filter(|id| statuses.get(*id).copied() != Some(NodeStatus::Done))
        // A node the reconciler superseded stays in the executed graph,
        // cancelled, so the transition removes it exactly as a `drop` would.
        // Named by the retry that replaced it, not read off its `cancelled`
        // status: a `cancel` parks a node *and* stops its dispatch, so a node
        // parked mid-flight settles `cancelled` too, and reading the status
        // deleted the very node `requeue` exists to bring back — along with the
        // gate it was holding over its dependents, which then ran.
        .filter(|id| !state.superseded.contains(*id))
        .cloned()
        .collect();

    let mut next = Graph::with_concurrency(state.graph.concurrency);
    for node in state.graph.iter() {
        if !carried.contains(&node.id) {
            continue;
        }
        let mut node = node.clone();
        // A satisfied dependency id falls out. A cross-DAG reference is not a
        // satisfied dependency id and is never removed by that rule: it names
        // no node of this graph, so it was never in the round to be satisfied.
        node.deps = node
            .deps
            .iter()
            .filter(|dep| graph::is_cross_dag(dep) || carried.contains(*dep))
            .cloned()
            .collect();
        // The watch passes through a consumer the transition carried out, to
        // whatever still depends on that consumer.
        let inherited: Vec<String> = state
            .graph
            .get(&node.id)
            .map(|original| original.deps.clone())
            .unwrap_or_default()
            .iter()
            .filter(|dep| !carried.contains(*dep) && !graph::is_cross_dag(dep))
            .filter_map(|dep| state.graph.get(dep))
            .flat_map(|dropped| {
                dropped
                    .deps
                    .iter()
                    .filter(|d| graph::is_cross_dag(d))
                    .cloned()
            })
            .collect();
        for reference in inherited {
            if !node.deps.contains(&reference) {
                node.deps.push(reference);
            }
        }
        // Context follows a node id, and the set is replaced rather than
        // appended: a note reports state observed while one attempt ran.
        node.context = state.notes_this_round.get(&node.id).cloned();
        let settled = statuses.get(&node.id).copied();
        carry_preserved_branch(&mut node, state, settled);
        next.insert(node);
    }
    next
}

/// Statuses whose work is still on the branch the round left behind.
///
/// A node that settled one of these ran, committed, and stopped — so the branch
/// holds work, and the next round has to continue it rather than cut a fresh one
/// beside it. `done` never reaches here (it falls out of the transition), and
/// `waiting` and `skipped` never dispatched, so there is nothing to preserve.
fn preserves_its_branch(status: Option<NodeStatus>) -> bool {
    matches!(
        status,
        Some(NodeStatus::Failed | NodeStatus::Cancelled | NodeStatus::Parked)
    )
}

/// Pin a carried node to the branch its last attempt left behind.
///
/// Without this the continuation cuts a fresh branch beside committed work
/// nothing points at any more: the publication that failed is retried against an
/// empty tree, and the branch that holds the work is left for a person to find.
///
/// A `branch` the *planner* wrote wins outright — naming one is a decision
/// somebody made after reading the result — and the `resume` follows it rather
/// than pointing somewhere else, which is the same agreement `retry` refuses to
/// break.
fn carry_preserved_branch(node: &mut Node, state: &RunState, status: Option<NodeStatus>) {
    if !preserves_its_branch(status) {
        return;
    }
    let Some(preserved) = state.branches.get(&node.id) else {
        return;
    };
    let branch = node.branch.clone().unwrap_or_else(|| preserved.clone());
    node.resume = Some(crate::plan::Resume {
        // The checkpoint is the sibling's to name; this crate records the branch
        // it was told about and nothing it was not.
        checkpoint: node.resume.as_ref().and_then(|r| r.checkpoint.clone()),
        branch: branch.clone(),
        // What the attempt actually finished, so the continuation re-runs only
        // what is left. Carried forward from an earlier continuation as well:
        // steps a round skipped are still on the branch it preserved.
        completed_steps: state
            .completed_steps
            .get(&node.id)
            .cloned()
            .unwrap_or_default(),
    });
    node.branch = Some(branch);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plan::PLAN_SCHEMA_VERSION;

    fn agent(id: &str, deps: &[&str]) -> Node {
        Node {
            id: id.into(),
            persona: Some("engineer".into()),
            task: Some("## What\ndo it".into()),
            deps: deps.iter().map(|d| (*d).to_string()).collect(),
            ..Node::default()
        }
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
                .map(|(id, status)| ((*id).to_string(), *status))
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
    fn a_done_node_falls_out_and_its_dependents_lose_the_satisfied_id() {
        let state = state_of(
            vec![agent("build", &[]), agent("ship", &["build"])],
            &[("build", NodeStatus::Done)],
        );
        let next = derive_next(&state);
        assert!(!next.contains("build"), "a done node was rescheduled");
        assert!(next.get("ship").expect("ship").deps.is_empty());
    }

    #[test]
    fn a_superseded_node_is_removed_exactly_as_a_drop_would_remove_it() {
        let mut state = state_of(
            vec![agent("build", &[]), agent("build-2", &[])],
            &[("build", NodeStatus::Cancelled)],
        );
        state.superseded.insert("build".into());
        let next = derive_next(&state);
        assert!(
            !next.contains("build"),
            "the superseded node was carried forward"
        );
        assert!(next.contains("build-2"));
    }

    /// A node parked while it was running is *not* a superseded node.
    ///
    /// `cancel` parks the node and stops its dispatch, and a stopped dispatch
    /// settles `cancelled` — the same status a `retry` leaves on the node it
    /// replaced. Read off that status, the transition deleted a node no `retry`
    /// had replaced: `requeue` then had nothing to bring back, and the
    /// dependents the park was holding lost their gate along with it and ran.
    #[test]
    fn a_node_parked_while_it_was_running_is_carried_rather_than_removed() {
        let mut parked = agent("sweep", &[]);
        parked.parked = true;
        let state = state_of(
            vec![parked, agent("after", &["sweep"])],
            &[("sweep", NodeStatus::Cancelled)],
        );
        let next = derive_next(&state);
        assert!(
            next.get("sweep").is_some_and(|node| node.parked),
            "a node parked mid-flight was removed instead of carried"
        );
        assert_eq!(
            next.get("after").expect("after").deps,
            vec!["sweep".to_string()],
            "the parked node's dependent lost the gate it was held behind"
        );
    }

    #[test]
    fn a_parked_node_is_carried_forward_without_being_dispatched() {
        let mut parked = agent("sweep", &[]);
        parked.parked = true;
        let state = state_of(vec![parked], &[]);
        let next = derive_next(&state);
        assert!(
            next.get("sweep").expect("sweep").parked,
            "the park did not carry"
        );
    }

    #[test]
    fn a_cross_dag_reference_is_never_removed_as_a_satisfied_dependency() {
        let state = state_of(vec![agent("consume", &["run:other#build"])], &[]);
        let next = derive_next(&state);
        assert_eq!(
            next.get("consume").expect("consume").deps,
            vec!["run:other#build".to_string()]
        );
    }

    #[test]
    fn a_watch_passes_through_the_consumer_the_transition_carried_out() {
        let state = state_of(
            vec![
                agent("consume", &["run:other#build"]),
                agent("after", &["consume"]),
            ],
            &[("consume", NodeStatus::Done)],
        );
        let next = derive_next(&state);
        assert!(!next.contains("consume"));
        assert_eq!(
            next.get("after").expect("after").deps,
            vec!["run:other#build".to_string()],
            "the watch ended with the consumer that carried it"
        );
    }

    #[test]
    fn only_this_rounds_notes_carry_and_they_replace_rather_than_append() {
        let mut stale = agent("build", &[]);
        stale.context = Some("last round's note".into());
        let mut state = state_of(vec![stale, agent("other", &[])], &[]);
        state
            .notes_this_round
            .insert("other".into(), "this round's note".into());

        let next = derive_next(&state);
        assert_eq!(
            next.get("build").expect("build").context,
            None,
            "a stale note was carried forward"
        );
        assert_eq!(
            next.get("other").expect("other").context.as_deref(),
            Some("this round's note")
        );
    }

    #[test]
    fn a_graph_whose_every_node_is_done_derives_no_next_round() {
        let state = state_of(vec![agent("build", &[])], &[("build", NodeStatus::Done)]);
        assert!(derive_next(&state).is_empty());
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

    #[test]
    fn the_stall_threshold_falls_back_when_the_environment_is_unusable() {
        // Read through the same helper the round uses, so an unusable value
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
    fn dispatch_labels_carry_only_the_reserved_keys() {
        let labels = dispatch_labels("demo", 2, "build", Some("implement"), Some("engineer"));
        assert_eq!(labels.run_id.as_deref(), Some("demo"));
        assert_eq!(labels.round, Some(2));
        assert_eq!(labels.step.as_deref(), Some("implement"));
        assert!(labels.extra.is_empty());
    }
}
