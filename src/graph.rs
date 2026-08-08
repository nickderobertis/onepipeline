//! The task DAG: what a plan must be, and what its nodes may do next.
//!
//! Two questions live here and nowhere else. **Is this graph legal?** — the node
//! shape rules, the reference rules, and acyclicity, checked at the trust
//! boundary so a typo fails loudly before any provider time is spent. And
//! **what may run now?** — each node's derived status against the nodes that
//! have already settled.
//!
//! `blocked` and `skipped` are *derived*, never stored: every committed edit
//! discards them and re-derives them against the new graph, so a node the
//! planner just made eligible is schedulable on the same pass.

use std::collections::{BTreeMap, BTreeSet};

use crate::error::{Error, Result};
use crate::plan::{Node, NodeKind, Plan, Step};

/// The separator reserved for addressing a step within its node.
pub const STEP_SEPARATOR: char = '/';

/// The prefix of a cross-DAG dependency, `run:<id>#<node>`.
pub const CROSS_DAG_PREFIX: &str = "run:";

/// Where a node has got to.
///
/// The settled statuses are recorded in the journal; [`Blocked`](Self::Blocked)
/// and [`Skipped`](Self::Skipped) are derived on every read and never written as
/// a node's own settlement.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum NodeStatus {
    /// Not yet eligible, and nothing prevents it becoming so.
    Pending,
    /// Eligible now: every dependency is `done`.
    Ready,
    /// A dispatch is in flight.
    Running,
    /// A ready human action needs a person.
    Waiting,
    /// Transitively gated by a waiting human or a parked dependency.
    Blocked,
    /// A planner `cancel` idled it. No later round dispatches it until a
    /// `requeue`.
    Parked,
    /// A `drop` or `retry` stopped its dispatch cooperatively. Deliberately not
    /// [`Parked`](Self::Parked): the round took this stop, so the harness
    /// finishes what it started, while a park is the planner's own idle.
    Cancelled,
    /// It executed and completed.
    Done,
    /// It executed and failed.
    Failed,
    /// A failed dependency made execution unsafe.
    Skipped,
}

impl NodeStatus {
    /// The word this status is written and rendered as.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Ready => "ready",
            Self::Running => "running",
            Self::Waiting => "waiting",
            Self::Blocked => "blocked",
            Self::Parked => "parked",
            Self::Cancelled => "cancelled",
            Self::Done => "done",
            Self::Failed => "failed",
            Self::Skipped => "skipped",
        }
    }

    /// Read a status back from a journal record.
    pub fn parse(text: &str) -> Option<Self> {
        Some(match text {
            "pending" => Self::Pending,
            "ready" => Self::Ready,
            "running" => Self::Running,
            "waiting" => Self::Waiting,
            "blocked" => Self::Blocked,
            "parked" => Self::Parked,
            "cancelled" => Self::Cancelled,
            "done" => Self::Done,
            "failed" => Self::Failed,
            "skipped" => Self::Skipped,
            _ => return None,
        })
    }

    /// Whether this round is finished with the node.
    pub fn is_settled(self) -> bool {
        matches!(
            self,
            Self::Done
                | Self::Failed
                | Self::Skipped
                | Self::Waiting
                | Self::Blocked
                | Self::Parked
                | Self::Cancelled
        )
    }

    /// Whether a later round may still dispatch the node.
    ///
    /// A `done` node is never rescheduled; a parked one waits for a `requeue`.
    pub fn is_dispatchable(self) -> bool {
        !matches!(self, Self::Done | Self::Parked)
    }
}

/// How a whole graph settled.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum GraphState {
    /// Every node is `done`.
    Complete,
    /// Something waits, is blocked, or is parked, and nothing failed.
    Waiting,
    /// A node failed or was skipped.
    Failed,
}

impl GraphState {
    /// The word this state is written and rendered as.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Complete => "complete",
            Self::Waiting => "waiting",
            Self::Failed => "failed",
        }
    }

    /// The process exit status a settled graph carries: 0 complete, 1 otherwise.
    pub fn exit_code(self) -> i32 {
        match self {
            Self::Complete => crate::error::EXIT_SUCCESS,
            _ => crate::error::EXIT_QUEUED,
        }
    }
}

/// The desired graph: the nodes a round is converging toward.
///
/// Insertion-ordered so a plan, a round's launch record, and the graph a
/// transition folds all render their nodes in the order the planner wrote them.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Graph {
    order: Vec<String>,
    nodes: BTreeMap<String, Node>,
    /// How many nodes may be dispatched at once.
    pub concurrency: u32,
}

impl Graph {
    /// The graph a plan describes.
    pub fn from_plan(plan: &Plan) -> Self {
        let mut graph = Self {
            concurrency: plan.concurrency,
            ..Self::default()
        };
        for node in &plan.tasks {
            graph.insert(node.clone());
        }
        graph
    }

    /// The plan this graph would be written as.
    pub fn to_plan(&self, source: &Plan) -> Plan {
        Plan {
            schema_version: source.schema_version,
            goal: source.goal.clone(),
            name: source.name.clone(),
            concurrency: self.concurrency,
            tasks: self.iter().cloned().collect(),
        }
    }

    /// Add a node, or replace one with the same id in place.
    pub fn insert(&mut self, node: Node) {
        if !self.nodes.contains_key(&node.id) {
            self.order.push(node.id.clone());
        }
        self.nodes.insert(node.id.clone(), node);
    }

    /// Remove a node, keeping the order of the rest.
    pub fn remove(&mut self, id: &str) -> Option<Node> {
        self.order.retain(|existing| existing != id);
        self.nodes.remove(id)
    }

    /// One node, by id.
    pub fn get(&self, id: &str) -> Option<&Node> {
        self.nodes.get(id)
    }

    /// One node, by id, to change.
    pub fn get_mut(&mut self, id: &str) -> Option<&mut Node> {
        self.nodes.get_mut(id)
    }

    /// Whether the graph holds a node with this id.
    pub fn contains(&self, id: &str) -> bool {
        self.nodes.contains_key(id)
    }

    /// The nodes, in the order the plan wrote them.
    pub fn iter(&self) -> impl Iterator<Item = &Node> {
        self.order.iter().filter_map(|id| self.nodes.get(id))
    }

    /// The node ids, in the order the plan wrote them.
    pub fn ids(&self) -> impl Iterator<Item = &String> {
        self.order.iter()
    }

    /// How many nodes the graph holds.
    pub fn len(&self) -> usize {
        self.order.len()
    }

    /// Whether the graph holds no nodes.
    pub fn is_empty(&self) -> bool {
        self.order.is_empty()
    }

    /// The nodes that name `id` as a dependency.
    pub fn dependents_of(&self, id: &str) -> Vec<String> {
        self.iter()
            .filter(|node| node.deps.iter().any(|dep| dep == id))
            .map(|node| node.id.clone())
            .collect()
    }
}

/// Whether a dependency reference names another run's node rather than this
/// graph's.
///
/// A cross-DAG reference names no node of this graph, so it was never in the
/// round to be satisfied: it is never removed as a satisfied dependency, and it
/// is carried to whatever depends on a consumer the transition carried out.
pub fn is_cross_dag(reference: &str) -> bool {
    reference.starts_with(CROSS_DAG_PREFIX) && reference.contains('#')
}

/// Check that a plan is one this engine may execute.
///
/// External input is validated here, at its trust boundary, so an unsatisfiable
/// graph is refused before any provider time is spent on it.
pub fn validate(plan: &Plan) -> Result<()> {
    if plan.schema_version != crate::plan::PLAN_SCHEMA_VERSION {
        return Err(Error::Invalid(format!(
            "plan schema_version {} is not {}",
            plan.schema_version,
            crate::plan::PLAN_SCHEMA_VERSION
        )));
    }
    if plan.concurrency == 0 {
        return Err(Error::Invalid("concurrency must be at least 1".into()));
    }
    if plan.tasks.is_empty() {
        return Err(Error::Invalid("a plan needs at least one node".into()));
    }
    if let Some(goal) = &plan.goal {
        if goal.text.trim().is_empty() {
            return Err(Error::Invalid("a goal needs non-empty text".into()));
        }
    }

    let mut seen = BTreeSet::new();
    for node in &plan.tasks {
        if node.id.trim().is_empty() {
            return Err(Error::Invalid("every node needs a non-empty id".into()));
        }
        if !seen.insert(node.id.clone()) {
            return Err(Error::Invalid(format!("duplicate node id '{}'", node.id)));
        }
        validate_node(node)?;
    }

    for node in &plan.tasks {
        for dep in &node.deps {
            if dep == &node.id {
                return Err(Error::Invalid(format!("node '{}' depends on itself", node.id)));
            }
            if is_cross_dag(dep) {
                continue;
            }
            if !seen.contains(dep) {
                return Err(Error::Invalid(format!(
                    "node '{}' depends on '{dep}', which is not in the plan",
                    node.id
                )));
            }
        }
    }

    if let Some(cycle) = find_cycle(&plan.tasks) {
        return Err(Error::Invalid(format!("dependency cycle: {cycle}")));
    }
    Ok(())
}

/// Check one node's shape.
pub fn validate_node(node: &Node) -> Result<()> {
    let named = |what: &str| Error::Invalid(format!("node '{}': {what}", node.id));

    if node.kind == NodeKind::Human {
        if node.id.contains(STEP_SEPARATOR) {
            return Err(named("a human id cannot contain '/', which addresses a step"));
        }
        if node.task.as_ref().is_none_or(|t| t.trim().is_empty()) {
            return Err(named("a human node needs task prose"));
        }
        if node.persona.is_some() || node.done_when.is_some() {
            return Err(named("a human node has no dispatch, so no persona or done_when"));
        }
        if node.repo.is_some() || node.steps.is_some() || node.expects_no_diff {
            return Err(named("a human node has no execution fields"));
        }
        if node.context.is_some() {
            return Err(named("a planner note is addressed to a dispatch, and a human node has none"));
        }
        return Ok(());
    }

    if node.expects_no_diff {
        if node.task.as_ref().is_none_or(|t| t.trim().is_empty()) {
            return Err(named("an expects_no_diff node needs task prose"));
        }
        if node.persona.is_some() || node.done_when.is_some() {
            return Err(named(
                "expects_no_diff settles without a dispatch, so it takes no persona or done_when",
            ));
        }
        if node.steps.is_some() {
            return Err(named("expects_no_diff and steps cannot both be set"));
        }
        return Ok(());
    }

    match (&node.repo, &node.steps) {
        (None, Some(_)) => Err(named("steps run on one branch, so they need a repo")),
        (None, None) => {
            if node.persona.is_none() {
                return Err(named("a direct agent node needs a persona"));
            }
            if node.task.as_ref().is_none_or(|t| t.trim().is_empty()) {
                return Err(named("a direct agent node needs task prose"));
            }
            Ok(())
        }
        (Some(_), Some(steps)) => {
            if node.persona.is_some() || node.task.is_some() {
                return Err(named("a node with steps takes its persona and task from them"));
            }
            validate_steps(node, steps)
        }
        (Some(_), None) => {
            if node.persona.is_none() {
                return Err(named("a lifecycle node needs a persona or steps"));
            }
            if node.task.as_ref().is_none_or(|t| t.trim().is_empty()) {
                return Err(named("a lifecycle node needs task prose"));
            }
            Ok(())
        }
    }
}

fn validate_steps(node: &Node, steps: &[Step]) -> Result<()> {
    let named = |what: String| Error::Invalid(format!("node '{}': {what}", node.id));
    if steps.is_empty() {
        return Err(named("a steps list cannot be empty".into()));
    }
    let mut seen = BTreeSet::new();
    for step in steps {
        if step.id.trim().is_empty() {
            return Err(named("every step needs a non-empty id".into()));
        }
        if !seen.insert(step.id.clone()) {
            return Err(named(format!("duplicate step id '{}'", step.id)));
        }
        if step.kind == NodeKind::Human {
            if step.id.contains(STEP_SEPARATOR) {
                return Err(named(format!(
                    "human step '{}' cannot contain '/', which addresses it",
                    step.id
                )));
            }
            if step.persona.is_some() || step.done_when.is_some() || step.expects_no_diff {
                return Err(named(format!(
                    "human step '{}' has no dispatch, so no persona, done_when, or expects_no_diff",
                    step.id
                )));
            }
        } else if step.expects_no_diff {
            if step.persona.is_some() || step.done_when.is_some() {
                return Err(named(format!(
                    "step '{}': expects_no_diff settles without a dispatch",
                    step.id
                )));
            }
        } else if step.persona.is_none() {
            return Err(named(format!("agent step '{}' needs a persona", step.id)));
        }
        if step.task.as_ref().is_none_or(|t| t.trim().is_empty()) {
            return Err(named(format!("step '{}' needs task prose", step.id)));
        }
    }
    for step in steps {
        for dep in &step.deps {
            if dep == &step.id {
                return Err(named(format!("step '{}' depends on itself", step.id)));
            }
            if !seen.contains(dep) {
                return Err(named(format!(
                    "step '{}' depends on '{dep}', which is not a step of this node",
                    step.id
                )));
            }
        }
    }
    Ok(())
}

/// The first dependency cycle, rendered as the path around it.
fn find_cycle(nodes: &[Node]) -> Option<String> {
    #[derive(Clone, Copy, PartialEq)]
    enum Mark {
        Open,
        Closed,
    }
    let deps: BTreeMap<&str, Vec<&str>> = nodes
        .iter()
        .map(|node| {
            (
                node.id.as_str(),
                node.deps
                    .iter()
                    .filter(|dep| !is_cross_dag(dep))
                    .map(String::as_str)
                    .collect(),
            )
        })
        .collect();
    let mut marks: BTreeMap<&str, Mark> = BTreeMap::new();
    let mut path: Vec<&str> = Vec::new();

    // Iterative depth-first search: a graph deep enough to overflow a recursive
    // walk is a graph a planner can legally write.
    for root in nodes.iter().map(|n| n.id.as_str()) {
        if marks.contains_key(root) {
            continue;
        }
        let mut stack: Vec<(&str, usize)> = vec![(root, 0)];
        marks.insert(root, Mark::Open);
        path.push(root);
        while let Some((node, index)) = stack.pop() {
            let children = deps.get(node).map(Vec::as_slice).unwrap_or_default();
            if index < children.len() {
                stack.push((node, index + 1));
                let child = children[index];
                match marks.get(child) {
                    Some(Mark::Open) => {
                        let start = path.iter().position(|n| *n == child).unwrap_or(0);
                        let mut cycle: Vec<&str> = path[start..].to_vec();
                        cycle.push(child);
                        return Some(cycle.join(" -> "));
                    }
                    Some(Mark::Closed) => {}
                    None => {
                        marks.insert(child, Mark::Open);
                        path.push(child);
                        stack.push((child, 0));
                    }
                }
            } else {
                marks.insert(node, Mark::Closed);
                path.pop();
            }
        }
    }
    None
}

/// Derive every node's status from the graph and what has already settled.
///
/// `recorded` holds only the statuses the journal wrote. Everything else —
/// `ready`, `pending`, and the two derived gates `blocked` and `skipped` — is
/// computed here against the current graph, so an edit that changes eligibility
/// takes effect on the same pass.
pub fn derive(
    graph: &Graph,
    recorded: &BTreeMap<String, NodeStatus>,
    resolved_cross_dag: &dyn Fn(&str) -> Option<NodeStatus>,
) -> BTreeMap<String, NodeStatus> {
    let mut statuses: BTreeMap<String, NodeStatus> = BTreeMap::new();
    for node in graph.iter() {
        if let Some(recorded) = recorded.get(&node.id) {
            // A recorded settlement stands, except for the two derived gates:
            // they are re-derived against the graph as it is now.
            if !matches!(recorded, NodeStatus::Blocked | NodeStatus::Skipped) {
                statuses.insert(node.id.clone(), *recorded);
            }
        }
    }

    // Repeat until nothing changes: a gate propagates along dependency edges,
    // and the plan does not promise a topological node order.
    loop {
        let mut changed = false;
        for node in graph.iter() {
            if statuses.contains_key(&node.id) {
                continue;
            }
            let Some(status) = eligibility(graph, node, &statuses, resolved_cross_dag) else {
                continue;
            };
            statuses.insert(node.id.clone(), status);
            changed = true;
        }
        if !changed {
            break;
        }
    }

    // Anything still unresolved has a dependency that has not settled.
    for node in graph.iter() {
        statuses.entry(node.id.clone()).or_insert(NodeStatus::Pending);
    }
    statuses
}

fn eligibility(
    graph: &Graph,
    node: &Node,
    statuses: &BTreeMap<String, NodeStatus>,
    resolved_cross_dag: &dyn Fn(&str) -> Option<NodeStatus>,
) -> Option<NodeStatus> {
    let mut all_done = true;
    let mut failed = false;
    let mut gated = false;

    for dep in &node.deps {
        let status = if is_cross_dag(dep) {
            // An unknown or unfinished upstream leaves the consumer blocked
            // rather than failing it: the upstream may still arrive.
            match resolved_cross_dag(dep) {
                Some(status) => status,
                None => {
                    all_done = false;
                    gated = true;
                    continue;
                }
            }
        } else if graph.contains(dep) {
            match statuses.get(dep) {
                Some(status) => *status,
                None => {
                    all_done = false;
                    continue;
                }
            }
        } else {
            // A dependency the graph no longer holds was detached by a `drop`.
            continue;
        };

        match status {
            NodeStatus::Done => {}
            NodeStatus::Failed | NodeStatus::Skipped => {
                failed = true;
                all_done = false;
            }
            NodeStatus::Waiting
            | NodeStatus::Blocked
            | NodeStatus::Parked
            | NodeStatus::Cancelled => {
                gated = true;
                all_done = false;
            }
            _ => all_done = false,
        }
    }

    // Failure takes precedence over a simultaneous waiting path: such a
    // descendant is skipped, not blocked.
    if failed {
        return Some(NodeStatus::Skipped);
    }
    if gated {
        return Some(NodeStatus::Blocked);
    }
    if !all_done {
        return None;
    }
    if node.parked {
        return Some(NodeStatus::Parked);
    }
    if node.kind == NodeKind::Human {
        return Some(NodeStatus::Waiting);
    }
    Some(NodeStatus::Ready)
}

/// How a graph with these statuses settled.
pub fn state_of(statuses: &BTreeMap<String, NodeStatus>) -> GraphState {
    if statuses
        .values()
        .any(|s| matches!(s, NodeStatus::Failed | NodeStatus::Skipped))
    {
        GraphState::Failed
    } else if statuses.values().any(|s| {
        matches!(
            s,
            NodeStatus::Waiting | NodeStatus::Blocked | NodeStatus::Parked | NodeStatus::Cancelled
        )
    }) {
        GraphState::Waiting
    } else if statuses.values().all(|s| *s == NodeStatus::Done) {
        GraphState::Complete
    } else {
        GraphState::Waiting
    }
}

/// Whether every node has settled, so the round has nothing left to converge.
pub fn is_terminal(statuses: &BTreeMap<String, NodeStatus>) -> bool {
    statuses.values().all(|s| s.is_settled())
}

/// What each ready human action unblocks, for the view that reports it.
pub fn unblocks(graph: &Graph, id: &str) -> Vec<String> {
    graph.dependents_of(id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plan::{Goal, PLAN_SCHEMA_VERSION};

    fn agent(id: &str, deps: &[&str]) -> Node {
        Node {
            id: id.into(),
            persona: Some("engineer".into()),
            task: Some("## What\ndo it".into()),
            deps: deps.iter().map(|d| (*d).to_string()).collect(),
            ..Node::default()
        }
    }

    fn human(id: &str, deps: &[&str]) -> Node {
        Node {
            id: id.into(),
            kind: NodeKind::Human,
            task: Some("approve it".into()),
            deps: deps.iter().map(|d| (*d).to_string()).collect(),
            ..Node::default()
        }
    }

    fn plan_of(tasks: Vec<Node>) -> Plan {
        Plan {
            schema_version: PLAN_SCHEMA_VERSION,
            goal: None,
            name: Some("test".into()),
            concurrency: 4,
            tasks,
        }
    }

    fn no_cross_dag(_: &str) -> Option<NodeStatus> {
        None
    }

    #[test]
    fn a_legal_mixed_plan_validates() {
        let plan = plan_of(vec![
            agent("build", &[]),
            human("approve", &["build"]),
            Node {
                id: "publish".into(),
                repo: Some("owner/repo".into()),
                persona: Some("engineer".into()),
                task: Some("## What\nship".into()),
                deps: vec!["approve".into()],
                ..Node::default()
            },
        ]);
        validate(&plan).expect("the plan is legal");
    }

    #[test]
    fn a_self_edge_and_a_cycle_are_both_refused() {
        let mut plan = plan_of(vec![agent("a", &["a"])]);
        let message = validate(&plan).unwrap_err().to_string();
        assert!(message.contains("depends on itself"), "{message}");

        plan = plan_of(vec![agent("a", &["b"]), agent("b", &["a"])]);
        let message = validate(&plan).unwrap_err().to_string();
        assert!(message.contains("cycle"), "{message}");
    }

    #[test]
    fn a_deep_chain_does_not_overflow_the_cycle_walk() {
        let mut tasks: Vec<Node> = Vec::new();
        for index in 0..20_000u32 {
            let deps: Vec<&str> = Vec::new();
            let mut node = agent(&format!("n{index}"), &deps);
            if index > 0 {
                node.deps = vec![format!("n{}", index - 1)];
            }
            tasks.push(node);
        }
        validate(&plan_of(tasks)).expect("a 20k-node chain is legal");
    }

    #[test]
    fn a_dangling_dependency_is_refused_but_a_cross_dag_one_is_not() {
        let plan = plan_of(vec![agent("a", &["nowhere"])]);
        let message = validate(&plan).unwrap_err().to_string();
        assert!(message.contains("not in the plan"), "{message}");

        let plan = plan_of(vec![agent("a", &["run:other#build"])]);
        validate(&plan).expect("a cross-DAG reference is not a missing node");
    }

    #[test]
    fn every_node_shape_rule_the_contract_states_is_enforced() {
        let cases: &[(Node, &str)] = &[
            (
                Node {
                    id: "no-persona".into(),
                    task: Some("t".into()),
                    ..Node::default()
                },
                "needs a persona",
            ),
            (
                Node {
                    id: "with/slash".into(),
                    kind: NodeKind::Human,
                    task: Some("t".into()),
                    ..Node::default()
                },
                "cannot contain '/'",
            ),
            (
                Node {
                    id: "human-persona".into(),
                    kind: NodeKind::Human,
                    task: Some("t".into()),
                    persona: Some("engineer".into()),
                    ..Node::default()
                },
                "no persona or done_when",
            ),
            (
                Node {
                    id: "human-context".into(),
                    kind: NodeKind::Human,
                    task: Some("t".into()),
                    context: Some("a note".into()),
                    ..Node::default()
                },
                "has none",
            ),
            (
                Node {
                    id: "nodiff-persona".into(),
                    expects_no_diff: true,
                    task: Some("t".into()),
                    persona: Some("engineer".into()),
                    ..Node::default()
                },
                "takes no persona or done_when",
            ),
            (
                Node {
                    id: "steps-no-repo".into(),
                    steps: Some(vec![Step {
                        id: "one".into(),
                        persona: Some("engineer".into()),
                        task: Some("t".into()),
                        ..Step::default()
                    }]),
                    ..Node::default()
                },
                "need a repo",
            ),
            (
                Node {
                    id: "steps-and-task".into(),
                    repo: Some("o/r".into()),
                    task: Some("t".into()),
                    steps: Some(vec![Step {
                        id: "one".into(),
                        persona: Some("engineer".into()),
                        task: Some("t".into()),
                        ..Step::default()
                    }]),
                    ..Node::default()
                },
                "takes its persona and task from them",
            ),
            (
                Node {
                    id: "step-no-persona".into(),
                    repo: Some("o/r".into()),
                    steps: Some(vec![Step {
                        id: "one".into(),
                        task: Some("t".into()),
                        ..Step::default()
                    }]),
                    ..Node::default()
                },
                "needs a persona",
            ),
            (
                Node {
                    id: "step-cycle".into(),
                    repo: Some("o/r".into()),
                    steps: Some(vec![Step {
                        id: "one".into(),
                        persona: Some("engineer".into()),
                        task: Some("t".into()),
                        deps: vec!["one".into()],
                        ..Step::default()
                    }]),
                    ..Node::default()
                },
                "depends on itself",
            ),
        ];
        for (node, expected) in cases {
            let message = validate_node(node).unwrap_err().to_string();
            assert!(
                message.contains(expected),
                "node '{}': expected {expected:?} in {message:?}",
                node.id
            );
        }
    }

    #[test]
    fn the_plan_envelope_itself_is_validated() {
        let mut plan = plan_of(vec![agent("a", &[])]);
        plan.schema_version = 99;
        assert!(validate(&plan).unwrap_err().to_string().contains("schema_version"));

        let mut plan = plan_of(vec![agent("a", &[])]);
        plan.concurrency = 0;
        assert!(validate(&plan).unwrap_err().to_string().contains("concurrency"));

        let mut plan = plan_of(vec![]);
        plan.tasks = vec![];
        assert!(validate(&plan).unwrap_err().to_string().contains("at least one node"));

        let mut plan = plan_of(vec![agent("a", &[]), agent("a", &[])]);
        plan.tasks[1].id = "a".into();
        assert!(validate(&plan).unwrap_err().to_string().contains("duplicate"));

        let mut plan = plan_of(vec![agent("a", &[])]);
        plan.goal = Some(Goal { text: "  ".into() });
        assert!(validate(&plan).unwrap_err().to_string().contains("non-empty text"));
    }

    #[test]
    fn a_ready_frontier_is_everything_whose_dependencies_are_done() {
        let graph = Graph::from_plan(&plan_of(vec![
            agent("a", &[]),
            agent("b", &[]),
            agent("c", &["a", "b"]),
        ]));
        let mut recorded = BTreeMap::new();
        recorded.insert("a".to_string(), NodeStatus::Done);

        let statuses = derive(&graph, &recorded, &no_cross_dag);
        assert_eq!(statuses["a"], NodeStatus::Done);
        assert_eq!(statuses["b"], NodeStatus::Ready);
        assert_eq!(statuses["c"], NodeStatus::Pending);
    }

    #[test]
    fn a_waiting_human_blocks_and_a_failure_skips_the_same_descendant() {
        let graph = Graph::from_plan(&plan_of(vec![
            agent("build", &[]),
            human("approve", &["build"]),
            agent("ship", &["approve"]),
        ]));
        let mut recorded = BTreeMap::new();
        recorded.insert("build".to_string(), NodeStatus::Done);
        let statuses = derive(&graph, &recorded, &no_cross_dag);
        assert_eq!(statuses["approve"], NodeStatus::Waiting);
        assert_eq!(statuses["ship"], NodeStatus::Blocked);
        assert_eq!(state_of(&statuses), GraphState::Waiting);

        // Failure takes precedence over a simultaneous waiting path.
        let graph = Graph::from_plan(&plan_of(vec![
            agent("build", &[]),
            human("approve", &[]),
            agent("ship", &["approve", "build"]),
        ]));
        let mut recorded = BTreeMap::new();
        recorded.insert("build".to_string(), NodeStatus::Failed);
        let statuses = derive(&graph, &recorded, &no_cross_dag);
        assert_eq!(statuses["approve"], NodeStatus::Waiting);
        assert_eq!(statuses["ship"], NodeStatus::Skipped);
        assert_eq!(state_of(&statuses), GraphState::Failed);
    }

    #[test]
    fn a_parked_node_blocks_its_dependents_rather_than_skipping_them() {
        let mut parked = agent("sweep", &[]);
        parked.parked = true;
        let graph = Graph::from_plan(&plan_of(vec![parked, agent("after", &["sweep"])]));
        let statuses = derive(&graph, &BTreeMap::new(), &no_cross_dag);
        assert_eq!(statuses["sweep"], NodeStatus::Parked);
        assert_eq!(statuses["after"], NodeStatus::Blocked);
        assert_eq!(state_of(&statuses), GraphState::Waiting);
        assert!(is_terminal(&statuses));
    }

    #[test]
    fn a_recorded_gate_is_discarded_and_re_derived() {
        // The journal recorded `blocked` while a human waited. The human has
        // since been attested, so the same node must re-derive to ready.
        let graph = Graph::from_plan(&plan_of(vec![human("approve", &[]), agent("ship", &["approve"])]));
        let mut recorded = BTreeMap::new();
        recorded.insert("ship".to_string(), NodeStatus::Blocked);
        recorded.insert("approve".to_string(), NodeStatus::Done);
        let statuses = derive(&graph, &recorded, &no_cross_dag);
        assert_eq!(statuses["ship"], NodeStatus::Ready);
    }

    #[test]
    fn an_unresolved_cross_dag_reference_blocks_its_consumer() {
        let graph = Graph::from_plan(&plan_of(vec![agent("consume", &["run:other#build"])]));
        let statuses = derive(&graph, &BTreeMap::new(), &no_cross_dag);
        assert_eq!(statuses["consume"], NodeStatus::Blocked);

        let done = |_: &str| Some(NodeStatus::Done);
        let statuses = derive(&graph, &BTreeMap::new(), &done);
        assert_eq!(statuses["consume"], NodeStatus::Ready);

        let failed = |_: &str| Some(NodeStatus::Failed);
        let statuses = derive(&graph, &BTreeMap::new(), &failed);
        assert_eq!(statuses["consume"], NodeStatus::Skipped);
    }

    #[test]
    fn a_dropped_dependency_detaches_rather_than_blocking() {
        let mut graph = Graph::from_plan(&plan_of(vec![agent("a", &[]), agent("b", &["a"])]));
        graph.remove("a");
        let statuses = derive(&graph, &BTreeMap::new(), &no_cross_dag);
        assert_eq!(statuses["b"], NodeStatus::Ready);
    }

    #[test]
    fn a_complete_graph_is_complete_and_exits_zero() {
        let graph = Graph::from_plan(&plan_of(vec![agent("a", &[])]));
        let mut recorded = BTreeMap::new();
        recorded.insert("a".to_string(), NodeStatus::Done);
        let statuses = derive(&graph, &recorded, &no_cross_dag);
        assert_eq!(state_of(&statuses), GraphState::Complete);
        assert_eq!(GraphState::Complete.exit_code(), 0);
        assert_eq!(GraphState::Waiting.exit_code(), 1);
        assert_eq!(GraphState::Failed.exit_code(), 1);
    }

    #[test]
    fn a_graph_still_running_has_not_settled() {
        let graph = Graph::from_plan(&plan_of(vec![agent("a", &[])]));
        let mut recorded = BTreeMap::new();
        recorded.insert("a".to_string(), NodeStatus::Running);
        let statuses = derive(&graph, &recorded, &no_cross_dag);
        assert!(!is_terminal(&statuses));
        assert_eq!(state_of(&statuses), GraphState::Waiting);
    }

    #[test]
    fn statuses_round_trip_through_their_written_word() {
        for status in [
            NodeStatus::Pending,
            NodeStatus::Ready,
            NodeStatus::Running,
            NodeStatus::Waiting,
            NodeStatus::Blocked,
            NodeStatus::Parked,
            NodeStatus::Cancelled,
            NodeStatus::Done,
            NodeStatus::Failed,
            NodeStatus::Skipped,
        ] {
            assert_eq!(NodeStatus::parse(status.as_str()), Some(status));
        }
        assert_eq!(NodeStatus::parse("invented"), None);
        assert!(!NodeStatus::Done.is_dispatchable());
        assert!(!NodeStatus::Parked.is_dispatchable());
        assert!(NodeStatus::Failed.is_dispatchable());
    }

    #[test]
    fn a_graph_keeps_the_order_the_plan_wrote_and_reports_dependents() {
        let mut graph = Graph::from_plan(&plan_of(vec![
            agent("first", &[]),
            agent("second", &["first"]),
        ]));
        graph.insert(agent("third", &["first"]));
        assert_eq!(
            graph.ids().cloned().collect::<Vec<_>>(),
            vec!["first", "second", "third"]
        );
        assert_eq!(unblocks(&graph, "first"), vec!["second", "third"]);
        assert_eq!(graph.len(), 3);
        assert!(!graph.is_empty());

        // Replacing a node in place keeps its position.
        graph.insert(agent("second", &[]));
        assert_eq!(
            graph.ids().cloned().collect::<Vec<_>>(),
            vec!["first", "second", "third"]
        );
        assert!(graph.get_mut("second").is_some());
        assert_eq!(graph.remove("second").map(|n| n.id), Some("second".into()));
        assert!(!graph.contains("second"));
        assert!(Graph::default().is_empty());
    }

    #[test]
    fn a_graph_renders_back_as_the_plan_it_came_from() {
        let source = plan_of(vec![agent("a", &[])]);
        let graph = Graph::from_plan(&source);
        let round_trip = graph.to_plan(&source);
        assert_eq!(round_trip, source);
    }
}
