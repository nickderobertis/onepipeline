//! Folding the journal into the state a round, a transition, and every view
//! read from.
//!
//! **The plan of record is the graph the round executed.** A round's
//! `round-NN/plan.json` is its launch record and is never rewritten, so a
//! transition that derived the next round from it would lose every live edit the
//! reconciler committed — a `retry` replacement's new id, an amended budget, a
//! branch pin. This module folds the round's own authoritative journal instead,
//! and the next round derives from what actually ran.
//!
//! A journal that cannot be folded strictly falls back to the launch record,
//! which is the same state that makes a recovery report rather than guess.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::Value;

use crate::edits::{self, Frontier, Operation};
use crate::event::{Envelope, Source};
use crate::graph::{Graph, NodeStatus};
use crate::journal;
use crate::plan::Plan;

/// Everything the journal says about a run.
#[derive(Debug, Clone, Default)]
pub struct RunState {
    /// The desired graph the current round is converging toward, with every
    /// committed edit applied.
    pub graph: Graph,
    /// The plan the run was launched with, for the fields a graph does not
    /// carry — the goal and the name.
    pub plan: Option<Plan>,
    /// The statuses the journal recorded *in the current round*. A node absent
    /// from this map has not started, which is what `reparent` and `cancel`
    /// test for.
    pub recorded: BTreeMap<String, NodeStatus>,
    /// Each settled node's outcome, when it recorded one.
    pub outcomes: BTreeMap<String, String>,
    /// When each node was dispatched, in epoch milliseconds.
    pub dispatched_at: BTreeMap<String, u64>,
    /// When each node settled, in epoch milliseconds.
    pub settled_at: BTreeMap<String, u64>,
    /// The current round number. `0` before the first round starts.
    pub round: u64,
    /// Whether a round is executing. Edits require a live round.
    pub round_open: bool,
    /// Human actions attested across the whole run.
    pub attestations: BTreeSet<String>,
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
    /// Whether `stop` ended the run.
    pub stopped: bool,
    /// Whether the fold met a line it could not read. Strict replay reports
    /// rather than silently folding an incomplete graph.
    pub strict: bool,
    /// Every node this run's nodes named across another run's DAG.
    pub cross_dag_watches: BTreeMap<String, u64>,
    /// The notes each node was given *during the round just finished*.
    ///
    /// A note reports state observed while one attempt ran, so it is stale as
    /// soon as the next attempt moves. The transition sets this set on the next
    /// plan rather than appending to it, which is what stops a node
    /// accumulating instructions.
    pub notes_this_round: BTreeMap<String, String>,
}

impl RunState {
    /// The frontier an edit is judged against.
    pub fn frontier(&self) -> Frontier {
        Frontier {
            recorded: self.recorded.clone(),
            attestations: self.attestations.clone(),
        }
    }

    /// Every node's status, with the derived gates recomputed against the graph
    /// as it stands now.
    pub fn statuses(&self) -> BTreeMap<String, NodeStatus> {
        crate::graph::derive(&self.graph, &self.recorded, &|_| None)
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
    state.last_write_at = Some(millis_of(&event.ts).unwrap_or(0).max(
        state.last_write_at.unwrap_or(0),
    ));
    if event.source != Source::Pipeline {
        // A relayed envelope is evidence the run is working, and nothing else:
        // a sibling library does not decide this crate's graph state.
        return;
    }
    let payload = &event.payload;
    match event.kind.0.as_str() {
        journal::RUN_STARTED => {
            if let Some(plan) = plan_of(payload) {
                state.graph = Graph::from_plan(&plan);
                state.plan = Some(plan);
            }
        }
        journal::ROUND_STARTED => {
            state.round = event.labels.round.unwrap_or(state.round + 1);
            state.round_open = true;
            // A round's recorded statuses are its own: the previous round's
            // settlements are folded into the graph it was handed, not carried
            // as live frontier state.
            state.recorded.clear();
            state.outcomes.clear();
            state.notes_this_round.clear();
            if let Some(plan) = plan_of(payload) {
                state.graph = Graph::from_plan(&plan);
                if state.plan.is_none() {
                    state.plan = Some(plan);
                }
            }
        }
        journal::ROUND_FINISHED => state.round_open = false,
        journal::NODE_DISPATCHED => {
            if let Some(node) = &event.labels.node {
                state.recorded.insert(node.clone(), NodeStatus::Running);
                if let Some(ts) = millis_of(&event.ts) {
                    state.dispatched_at.insert(node.clone(), ts);
                }
            }
        }
        journal::NODE_SETTLED => {
            let Some(node) = &event.labels.node else { return };
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
            if let Some(ts) = millis_of(&event.ts) {
                state.settled_at.insert(node.clone(), ts);
            }
        }
        journal::EDIT_COMMITTED => {
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
                    Operation::CompletionRequested { reason } => {
                        state.completion_requests.push(reason.clone());
                    }
                    Operation::RetryRequested { node, .. } => {
                        // The superseded node stays in the executed graph,
                        // cancelled, so the transition removes it exactly as an
                        // explicit `drop` would.
                        state.recorded.insert(node.clone(), NodeStatus::Cancelled);
                    }
                    Operation::NodeParked { node } => {
                        state.recorded.insert(node.clone(), NodeStatus::Parked);
                    }
                    Operation::NodeRequeued { node, .. } => {
                        state.recorded.remove(node);
                    }
                    Operation::ContextAdded { node, note } => {
                        state.notes_this_round.insert(node.clone(), note.clone());
                    }
                    _ => {}
                }
            }
        }
        journal::HUMAN_ATTESTED => {
            if let Some(reference) = payload.get("ref").and_then(Value::as_str) {
                state.attestations.insert(reference.to_string());
                state
                    .recorded
                    .insert(reference.to_string(), NodeStatus::Done);
            }
        }
        journal::COMPLETION_REQUESTED => {
            if let Some(reason) = payload.get("reason").and_then(Value::as_str) {
                state.completion_requests.push(reason.to_string());
            }
        }
        journal::PLANNER_SURFACE_QUEUED => state.surfaces_queued += 1,
        journal::PLANNER_SURFACED => {
            state.surfaces_read += 1;
            state.last_surface_at = millis_of(&event.ts);
        }
        journal::RUN_STOPPED => {
            state.stopped = true;
            state.round_open = false;
        }
        journal::UPSTREAM_MODIFIED => {
            if let Some(reference) = payload.get("ref").and_then(Value::as_str) {
                *state
                    .cross_dag_watches
                    .entry(reference.to_string())
                    .or_insert(0) += 1;
            }
        }
        _ => {}
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
    if bytes.len() != 24 || bytes[23] != b'Z' {
        return None;
    }
    let field = |from: usize, to: usize| ts.get(from..to)?.parse::<i64>().ok();
    let (year, month, day) = (field(0, 4)?, field(5, 7)?, field(8, 10)?);
    let (hour, minute, second) = (field(11, 13)?, field(14, 16)?, field(17, 19)?);
    let ms = field(20, 23)?;
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    let days = days_from_civil(year, month, day);
    let total = days * 86_400 + hour * 3_600 + minute * 60 + second;
    u64::try_from(total.checked_mul(1_000)?.checked_add(ms)?).ok()
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
    use crate::event::{EventKind, Labels, ENVELOPE_VERSION};
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

    fn pipeline(kind: &str, seq: u64, node: Option<&str>, fields: &[(&str, Value)]) -> Envelope {
        Envelope {
            v: ENVELOPE_VERSION,
            ts: crate::sys::rfc3339_from_millis(1_786_000_000_000 + seq * 1_000),
            stream: "s".into(),
            seq,
            source: Source::Pipeline,
            kind: EventKind(kind.into()),
            labels: Labels {
                node: node.map(str::to_string),
                ..labels("demo", Some(1), None)
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

    #[test]
    fn the_fold_reconstructs_the_graph_the_round_executed() {
        let plan = plan_of_nodes(vec![agent("build", &[]), agent("ship", &["build"])]);
        let retry = Operation::NodeAdded {
            node: Box::new(agent("build-2", &[])),
            retry_of: Some("build".into()),
        };
        let events = vec![
            pipeline(journal::RUN_STARTED, 0, None, &[("plan", json!(plan))]),
            pipeline(journal::ROUND_STARTED, 1, None, &[("plan", json!(plan))]),
            pipeline(journal::NODE_DISPATCHED, 2, Some("build"), &[]),
            pipeline(
                journal::NODE_SETTLED,
                3,
                Some("build"),
                &[("status", json!("failed")), ("outcome", json!("gate-failed"))],
            ),
            pipeline(
                journal::EDIT_COMMITTED,
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
        assert_eq!(state.round, 1);
        assert!(state.round_open);
        assert!(state.graph.contains("build-2"), "the replacement is not in the plan of record");
        assert_eq!(state.recorded["build"], NodeStatus::Cancelled);
        assert_eq!(state.outcomes["build"], "gate-failed");
        assert!(state.dispatched_at.contains_key("build"));
        assert!(state.settled_at.contains_key("build"));
    }

    #[test]
    fn an_edit_whose_operations_cannot_be_folded_ends_strict_replay() {
        let plan = plan_of_nodes(vec![agent("build", &[])]);
        let events = vec![
            pipeline(journal::RUN_STARTED, 0, None, &[("plan", json!(plan))]),
            pipeline(
                journal::EDIT_COMMITTED,
                1,
                None,
                &[("operations", json!([{"kind": "from-the-future"}]))],
            ),
        ];
        let state = fold(&events);
        assert!(!state.strict, "an unfoldable operation was folded anyway");
    }

    #[test]
    fn a_new_round_clears_the_previous_rounds_frontier() {
        let plan = plan_of_nodes(vec![agent("build", &[])]);
        let mut second = pipeline(journal::ROUND_STARTED, 3, None, &[("plan", json!(plan))]);
        second.labels.round = Some(2);
        let events = vec![
            pipeline(journal::RUN_STARTED, 0, None, &[("plan", json!(plan))]),
            pipeline(journal::ROUND_STARTED, 1, None, &[("plan", json!(plan))]),
            pipeline(
                journal::NODE_SETTLED,
                2,
                Some("build"),
                &[("status", json!("failed"))],
            ),
            pipeline(journal::ROUND_FINISHED, 4, None, &[]),
            second,
        ];
        let state = fold(&events);
        assert_eq!(state.round, 2);
        assert!(state.round_open);
        assert!(state.recorded.is_empty(), "round 1's frontier leaked into round 2");
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
            pipeline(journal::RUN_STARTED, 0, None, &[("plan", json!(plan))]),
            pipeline(journal::PLANNER_SURFACE_QUEUED, 1, None, &[]),
            pipeline(journal::PLANNER_SURFACED, 2, None, &[]),
            pipeline(journal::HUMAN_ATTESTED, 3, None, &[("ref", json!("approve"))]),
            pipeline(
                journal::COMPLETION_REQUESTED,
                4,
                None,
                &[("reason", json!("verified"))],
            ),
            pipeline(journal::UPSTREAM_MODIFIED, 5, None, &[("ref", json!("run:o#n"))]),
            pipeline(journal::RUN_STOPPED, 6, None, &[]),
        ];
        let state = fold(&events);
        assert_eq!(state.surfaces_queued, 1);
        assert_eq!(state.surfaces_read, 1);
        assert!(state.last_surface_at.is_some());
        assert!(state.attestations.contains("approve"));
        assert_eq!(state.recorded["approve"], NodeStatus::Done);
        assert_eq!(state.completion_requests, vec!["verified".to_string()]);
        assert_eq!(state.cross_dag_watches["run:o#n"], 1);
        assert!(state.stopped);
        assert!(!state.round_open);
    }

    #[test]
    fn a_relayed_sibling_envelope_is_evidence_of_work_and_nothing_more() {
        let plan = plan_of_nodes(vec![agent("build", &[])]);
        let mut relayed = pipeline(journal::NODE_SETTLED, 1, Some("build"), &[("status", json!("done"))]);
        relayed.source = Source::Agentgraph;
        let state = fold(&[
            pipeline(journal::RUN_STARTED, 0, None, &[("plan", json!(plan))]),
            relayed,
        ]);
        assert!(
            state.recorded.is_empty(),
            "a sibling's envelope decided this crate's graph state"
        );
        assert!(state.last_write_at.is_some());
    }

    #[test]
    fn parking_and_requeueing_move_the_node_in_and_out_of_the_frontier() {
        let plan = plan_of_nodes(vec![agent("sweep", &[])]);
        let park = pipeline(
            journal::EDIT_COMMITTED,
            1,
            None,
            &[(
                "operations",
                json!([Operation::NodeParked { node: "sweep".into() }]),
            )],
        );
        let state = fold(&[
            pipeline(journal::RUN_STARTED, 0, None, &[("plan", json!(plan))]),
            park.clone(),
        ]);
        assert_eq!(state.recorded["sweep"], NodeStatus::Parked);
        assert!(state.graph.get("sweep").expect("sweep").parked);

        let requeue = pipeline(
            journal::EDIT_COMMITTED,
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
            pipeline(journal::RUN_STARTED, 0, None, &[("plan", json!(plan))]),
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
            pipeline(journal::RUN_STARTED, 0, None, &[("plan", json!(plan))]),
            pipeline(
                journal::NODE_SETTLED,
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
