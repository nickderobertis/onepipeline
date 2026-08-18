//! Folding the journal into the state the engine loop and every view read from.
//!
//! **The plan of record is the graph the run is executing.** The plan file is
//! the launch record and is never rewritten, so a reader that derived the live
//! graph from it would lose every live edit the reconciler committed — a `retry`
//! replacement's new id, an amended budget, a branch pin. This module folds the
//! run's own authoritative journal instead.
//!
//! There is no round here, and nothing is per-round: the frontier is continuous,
//! so what a node last recorded stands until it records something else.

use std::collections::{BTreeMap, BTreeSet};

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
    /// The statuses the journal recorded. A node absent from this map has not
    /// started, which is what `reparent` and `cancel` test for.
    pub recorded: BTreeMap<String, NodeStatus>,
    /// Each settled node's outcome, when it recorded one.
    pub outcomes: BTreeMap<String, String>,
    /// The branch each settled node left behind, as its dispatch reported it.
    ///
    /// Not the same thing as the branch a node's *plan* pins: this is what the
    /// work actually landed on, which for an unpinned node the sibling named,
    /// and it is the only record of where preserved work is.
    pub branches: BTreeMap<String, String>,
    /// Where a human reads the change each published node opened.
    pub change_urls: BTreeMap<String, String>,
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
    /// How many envelopes this node's dispatch has recorded.
    pub events: u64,
    /// When the last of them arrived, in epoch milliseconds.
    pub last_at: Option<u64>,
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
    /// How many records carried this same side, identity, and reason.
    ///
    /// Non-zero because a refusal exists only by having been recorded once.
    /// Deliberately not a count of *turns*: the producer stamps a turn on each
    /// advance and this does not read it, so claiming turns would be a
    /// measurement nothing here made.
    pub records: std::num::NonZeroU64,
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

/// The kind `oneagentgraph` reports a bounded tool summary as.
///
/// A wire string rather than one of [`journal::PipelineKind`]'s: it is the
/// sibling's vocabulary, and this crate reads that half of the merged store
/// without closing it.
const TURN_ACTIVITY: &str = "turn-activity";

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
            recorded: self.recorded.clone(),
            attestations: self.attestations.clone(),
            in_flight: BTreeMap::new(),
        }
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
        crate::graph::derive(&self.graph, &self.recorded, upstream)
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
        // of what the node it belongs to is doing while it runs, and the only
        // evidence of which identity refused when it stops running.
        fold_activity(state, event);
        fold_refusal(state, event);
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
                state.recorded.insert(node.clone(), NodeStatus::Running);
                if let Some(ts) = millis_of(&event.ts) {
                    state.dispatched_at.insert(node.clone(), ts);
                }
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
                state.recorded.insert(node.clone(), status);
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
                        state.recorded.insert(node.clone(), NodeStatus::Done);
                    }
                    // A completion request is recorded as its own event by
                    // whichever side took it, so folding it here too would
                    // count one request twice.
                    Operation::CompletionRequested { .. } => {}
                    Operation::RetryRequested { node, .. } => {
                        // What the supersession did to the node it replaced. The
                        // node itself leaves the graph with the same edit, so
                        // this is what the run's record says became of it.
                        state.recorded.insert(node.clone(), NodeStatus::Cancelled);
                    }
                    Operation::NodeParked { node } => {
                        state.recorded.insert(node.clone(), NodeStatus::Parked);
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
        Some(journal::PipelineKind::HumanAttested) => {
            if let Some(reference) = payload.get("ref").and_then(Value::as_str) {
                state.attestations.insert(reference.to_string());
                state
                    .recorded
                    .insert(reference.to_string(), NodeStatus::Done);
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
        Some(journal::PipelineKind::DriverAdopted) => {
            state
                .recorded
                .retain(|_, status| *status != NodeStatus::Running);
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

/// Statuses whose work is still on the branch the attempt left behind.
///
/// A node that settled one of these ran, committed, and stopped — so the branch
/// holds work, and anything that runs the node again has to continue it rather
/// than cut a fresh one beside it. `done` never reaches here, and `waiting` and
/// `skipped` never dispatched, so there is nothing to preserve.
fn preserves_its_branch(status: NodeStatus) -> bool {
    matches!(
        status,
        NodeStatus::Failed | NodeStatus::Cancelled | NodeStatus::Parked
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
/// Every relayed envelope counts — a dispatch that is fetching, gating, or
/// publishing is working just as much as one mid-turn — and only a
/// `turn-activity` names a tool, because that is the only kind carrying one.
fn fold_activity(state: &mut RunState, event: &Envelope) {
    let Some(node) = event.labels.node.as_deref() else {
        return;
    };
    let activity = state.activity.entry(node.to_string()).or_default();
    activity.events += 1;
    activity.last_at = millis_of(&event.ts).or(activity.last_at);
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
        // The label arrives in `extra`, because this crate's own envelope does
        // not declare `member` — so it is checked here rather than by a schema.
        // A value that is not a member name is kept apart from a producer that
        // stamped none: they are different facts about the record.
        member: match event.labels.extra.get("member") {
            None => MemberLabel::Unstamped,
            Some(Value::String(member)) => MemberLabel::Named(member.clone()),
            Some(_) => MemberLabel::Unreadable,
        },
        records: std::num::NonZeroU64::MIN,
    };
    let recorded = state.refusals.entry(node.to_string()).or_default();
    // The turn is deliberately not part of what makes two records the same: one
    // side's chain refusing the same identity the same way is one fact about
    // this node, however many turns asked it.
    if let Some(same) = recorded.iter_mut().find(|seen| {
        seen.advanced.identity == refusal.advanced.identity
            && seen.advanced.role == refusal.advanced.role
            && seen.advanced.reason == refusal.advanced.reason
            && seen.member == refusal.member
    }) {
        same.records = same.records.saturating_add(1);
        return;
    }
    recorded.push(refusal);
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
        assert_eq!(state.recorded["build"], NodeStatus::Cancelled);
        assert_eq!(state.outcomes["build"], "gate-failed");
        assert!(state.dispatched_at.contains_key("build"));
        assert!(state.settled_at.contains_key("build"));
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
        assert_eq!(state.recorded["build"], NodeStatus::Done);
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
        assert_eq!(held.recorded["build"], NodeStatus::Running);
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
        assert_eq!(state.recorded["approve"], NodeStatus::Done);
        assert_eq!(state.completion_requests, vec!["verified".to_string()]);
        assert_eq!(state.cross_dag_watches["run:o#n"], 1);
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
        assert_eq!(state.activity["build"].events, 1);
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
            seen.events, 2,
            "the node-dispatched was counted as activity"
        );
        assert_eq!(seen.last_at, millis_of(&activity(3, "Read", "x").ts));
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
        assert_eq!(state.activity["build"].events, 2);
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
        assert_eq!(state.activity["build"].events, 2);
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
        assert_eq!(state.recorded["sweep"], NodeStatus::Parked);
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
