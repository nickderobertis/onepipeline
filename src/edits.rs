//! The live-edit reconciler: validating one delta against the live frontier,
//! and compiling it into the mutations the journal records.
//!
//! Every edit is **applied or rejected with a reason**, and the reason is the
//! one the reconciler would give — [`compile`] is the single validator both the
//! submission check and the reconciler run, so a planner is never told an edit
//! was accepted that the reconciler then quietly drops.
//!
//! An accepted delta compiles to a list of [`Operation`]s recorded as one
//! `edit-committed` event carrying both the submitted command and its compiled
//! operations. Replay therefore sees all of a delta's mutations or none of them,
//! and reconstructs the graph from what was actually submitted.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::channel::{Command, Dependents};
use crate::error::{Error, Result};
use crate::graph::{self, Graph, NodeStatus};
use crate::plan::Node;

/// One compiled mutation inside an atomic edit commit.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum Operation {
    /// A node joined the graph.
    NodeAdded {
        /// The node's full definition.
        node: Box<Node>,
        /// The node this one supersedes, when it came from a `retry`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        retry_of: Option<String>,
    },
    /// A dependency edge was added.
    EdgeAdded {
        /// The dependency.
        from: String,
        /// The dependent.
        to: String,
    },
    /// A dependency edge was removed.
    EdgeRemoved {
        /// The dependency.
        from: String,
        /// The dependent.
        to: String,
    },
    /// A node left the graph.
    NodeDropped {
        /// The node.
        node: String,
        /// What was asked of its dependents.
        dependents: Dependents,
    },
    /// A node's dependencies were replaced wholesale.
    Reparent {
        /// The node.
        node: String,
        /// Its dependencies before.
        from: Vec<String>,
        /// Its dependencies after.
        to: Vec<String>,
    },
    /// A node was superseded by a fresh lineage.
    RetryRequested {
        /// The superseded node.
        node: String,
        /// The replacement's id.
        replacement: String,
        /// The dependents whose derived state the replacement resets.
        reset: Vec<String>,
    },
    /// A node was parked by a planner `cancel`.
    NodeParked {
        /// The node.
        node: String,
    },
    /// A parked node returned to the desired frontier.
    NodeRequeued {
        /// The node.
        node: String,
        /// The overrides merged onto it. Omitted when there were none, so
        /// "amended nothing" and "amended with nothing" are one record.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        amend: Option<Map<String, Value>>,
    },
    /// A human action was completed.
    HumanAttested {
        /// The action's reference.
        node: String,
    },
    /// The planner asked for completion, independently of graph mutation.
    CompletionRequested {
        /// Why.
        reason: String,
    },
    /// A planner note reached a node.
    ContextAdded {
        /// The node.
        node: String,
        /// The note.
        note: String,
        /// Whether it went into the node's running turn or onto its next
        /// dispatch. Absent from a record written before delivery had modes,
        /// which is [`Delivery::Deferred`] — the only thing those records did.
        #[serde(default)]
        delivery: Delivery,
    },
}

/// How a planner note actually reached its node.
///
/// The fact `edit-committed` records, and what tells replay whether the note is
/// still owed to a later dispatch: a note the running turn took has been read,
/// so carrying it into the next round would repeat a correction the worker has
/// already acted on.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Delivery {
    /// Into the turn that was running when the edit arrived.
    Live,
    /// Onto the node's next dispatch.
    #[default]
    Deferred,
}

/// What the reconciler knows about the run when it judges an edit.
#[derive(Debug, Clone, Default)]
pub struct Frontier {
    /// The statuses the journal actually recorded. A node absent from this map
    /// has not started, which is what `reparent` and `cancel` test for.
    pub recorded: BTreeMap<String, NodeStatus>,
    /// The human actions already attested.
    pub attestations: BTreeSet<String>,
}

/// Validate one command against the live frontier and compile its mutations.
///
/// The graph is mutated in place only on success: a command that fails
/// validation leaves it exactly as it was, so a rejected edit in the middle of
/// an envelope cannot half-apply.
pub fn compile(
    graph: &mut Graph,
    frontier: &Frontier,
    command: &Command,
) -> Result<Vec<Operation>> {
    compile_with(graph, frontier, command, Delivery::Deferred)
}

/// [`compile`], for a caller that has already delivered the note.
///
/// Only a `context` command reads `delivery`, and only the reconciler has an
/// answer for it: whether a note reached the running turn is the outcome of an
/// `oneagentgraph interrupt`, not a judgement about the graph. Every other
/// caller — the submission check most of all — validates the same command
/// without pulling that lever, which is why the delivery is a parameter rather
/// than something this module works out.
pub fn compile_with(
    graph: &mut Graph,
    frontier: &Frontier,
    command: &Command,
    delivery: Delivery,
) -> Result<Vec<Operation>> {
    // Validate against a copy, so a refusal partway through a multi-edge
    // mutation cannot leave the caller's graph in a state nothing submitted.
    let mut candidate = graph.clone();
    let operations = compile_into(&mut candidate, frontier, command, delivery)?;
    if !matches!(command, Command::Complete { .. } | Command::Attest { .. }) {
        // The resulting graph must still satisfy the normal plan schema.
        let plan = candidate.to_plan(&crate::plan::Plan {
            schema_version: crate::plan::PLAN_SCHEMA_VERSION,
            goal: None,
            name: None,
            concurrency: candidate.concurrency,
            tasks: Vec::new(),
        });
        graph::validate_edited(&plan).map_err(|e| Error::Refused(e.to_string()))?;
    }
    *graph = candidate;
    Ok(operations)
}

fn compile_into(
    graph: &mut Graph,
    frontier: &Frontier,
    command: &Command,
    delivery: Delivery,
) -> Result<Vec<Operation>> {
    match command {
        Command::Add { node } => compile_add(graph, node),
        Command::Drop { id, dependents } => compile_drop(graph, frontier, id, *dependents),
        Command::Reparent { id, deps } => compile_reparent(graph, frontier, id, deps),
        Command::Retry { id, node } => compile_retry(graph, frontier, id, node),
        Command::Cancel { id } => compile_cancel(graph, frontier, id),
        Command::Requeue { id, amend } => compile_requeue(graph, id, amend.as_ref()),
        Command::Attest { reference } => compile_attest(frontier, reference),
        Command::Complete { reason } => Ok(vec![Operation::CompletionRequested {
            reason: reason.clone(),
        }]),
        Command::Context { id, note, .. } => compile_context(graph, frontier, id, note, delivery),
    }
}

fn refuse(what: impl Into<String>) -> Error {
    Error::Refused(what.into())
}

fn compile_add(graph: &mut Graph, node: &Node) -> Result<Vec<Operation>> {
    if graph.contains(&node.id) {
        return Err(refuse(format!("add: node '{}' already exists", node.id)));
    }
    graph::validate_node(node).map_err(|e| refuse(e.to_string()))?;
    let mut operations = vec![Operation::NodeAdded {
        node: Box::new(node.clone()),
        retry_of: None,
    }];
    for dep in &node.deps {
        operations.push(Operation::EdgeAdded {
            from: dep.clone(),
            to: node.id.clone(),
        });
    }
    graph.insert(node.clone());
    Ok(operations)
}

fn compile_reparent(
    graph: &mut Graph,
    frontier: &Frontier,
    id: &str,
    deps: &[String],
) -> Result<Vec<Operation>> {
    let Some(node) = graph.get(id) else {
        return Err(refuse(format!("reparent: no node '{id}'")));
    };
    if frontier.recorded.contains_key(id) {
        return Err(refuse(format!("reparent: node '{id}' has already started")));
    }
    let previous = node.deps.clone();
    let mut operations: Vec<Operation> = previous
        .iter()
        .map(|dep| Operation::EdgeRemoved {
            from: dep.clone(),
            to: id.to_string(),
        })
        .collect();
    operations.extend(deps.iter().map(|dep| Operation::EdgeAdded {
        from: dep.clone(),
        to: id.to_string(),
    }));
    operations.push(Operation::Reparent {
        node: id.to_string(),
        from: previous,
        to: deps.to_vec(),
    });
    if let Some(node) = graph.get_mut(id) {
        node.deps = deps.to_vec();
    }
    Ok(operations)
}

fn compile_drop(
    graph: &mut Graph,
    frontier: &Frontier,
    id: &str,
    dependents: Dependents,
) -> Result<Vec<Operation>> {
    let Some(target) = graph.get(id).cloned() else {
        return Err(refuse(format!("drop: no node '{id}'")));
    };
    let direct = graph.dependents_of(id);

    // A lifecycle node is its repository's publication anchor. Removing the last
    // one while an unresolved dependent still targets that repository would cut
    // that dependent's branch from its root.
    if let Some(repo) = &target.repo {
        let unresolved_same_identity = direct.iter().any(|dependent| {
            graph.get(dependent).and_then(|n| n.repo.as_ref()) == Some(repo)
                && frontier.recorded.get(dependent) != Some(&NodeStatus::Done)
        });
        let alternative_anchor = graph.iter().any(|node| {
            node.id != id
                && node.repo.as_ref() == Some(repo)
                && frontier.recorded.get(&node.id) == Some(&NodeStatus::Done)
        });
        if unresolved_same_identity && !alternative_anchor {
            return Err(refuse(
                "drop: would remove the last unresolved publication anchor",
            ));
        }
    }

    let mut operations = Vec::new();
    let mut removed: BTreeSet<String> = BTreeSet::from([id.to_string()]);
    match dependents {
        Dependents::Drop => {
            let mut pending = direct;
            while let Some(candidate) = pending.pop() {
                if !removed.insert(candidate.clone()) {
                    continue;
                }
                pending.extend(graph.dependents_of(&candidate));
            }
        }
        Dependents::Detach => {
            for dependent in &direct {
                if let Some(node) = graph.get_mut(dependent) {
                    node.deps.retain(|dep| dep != id);
                }
                operations.push(Operation::EdgeRemoved {
                    from: id.to_string(),
                    to: dependent.clone(),
                });
            }
        }
    }
    for dropped in &removed {
        graph.remove(dropped);
        operations.push(Operation::NodeDropped {
            node: dropped.clone(),
            dependents,
        });
    }
    Ok(operations)
}

fn compile_retry(
    graph: &mut Graph,
    frontier: &Frontier,
    id: &str,
    replacement: &Node,
) -> Result<Vec<Operation>> {
    let Some(target) = graph.get(id).cloned() else {
        return Err(refuse(format!("retry: no node '{id}'")));
    };
    match frontier.recorded.get(id) {
        Some(NodeStatus::Running | NodeStatus::Failed | NodeStatus::Cancelled) => {}
        _ => {
            return Err(refuse(format!(
                "retry: node '{id}' is not running, failed, or cancelled"
            )))
        }
    }
    if replacement.id.trim().is_empty() {
        return Err(refuse("retry: the replacement needs a non-empty id"));
    }
    if graph.contains(&replacement.id) {
        return Err(refuse(format!(
            "retry: replacement id '{}' must be new",
            replacement.id
        )));
    }
    let replacement = pin_retry_branch(inherit_preserved_branch(
        validate_retry_pin(replacement)?,
        &target,
    ));
    graph::validate_node(&replacement).map_err(|e| refuse(e.to_string()))?;

    let mut replacement = replacement;
    if replacement.deps.is_empty() {
        replacement.deps = target.deps.clone();
    }

    let direct = graph.dependents_of(id);
    let mut reset: BTreeSet<String> = direct.iter().cloned().collect();
    let mut pending: Vec<String> = direct.clone();
    while let Some(predecessor) = pending.pop() {
        for dependent in graph.dependents_of(&predecessor) {
            if reset.insert(dependent.clone()) {
                pending.push(dependent);
            }
        }
    }

    let mut operations = vec![
        Operation::RetryRequested {
            node: id.to_string(),
            replacement: replacement.id.clone(),
            reset: reset.into_iter().collect(),
        },
        Operation::NodeAdded {
            node: Box::new(replacement.clone()),
            retry_of: Some(id.to_string()),
        },
    ];
    for dep in &replacement.deps {
        operations.push(Operation::EdgeAdded {
            from: dep.clone(),
            to: replacement.id.clone(),
        });
    }
    graph.insert(replacement.clone());
    for dependent in &direct {
        if let Some(node) = graph.get_mut(dependent) {
            for dep in &mut node.deps {
                if dep == id {
                    dep.clone_from(&replacement.id);
                }
            }
        }
        operations.push(Operation::EdgeRemoved {
            from: id.to_string(),
            to: dependent.clone(),
        });
        operations.push(Operation::EdgeAdded {
            from: replacement.id.clone(),
            to: dependent.clone(),
        });
    }
    Ok(operations)
}

/// A retry may name only one branch, and it gets that branch every time.
///
/// A replacement carrying both a `branch` pin and a `resume` checkpoint that
/// name different branches is answering "which branch does this work live on?"
/// twice, and the lifecycle honours the checkpoint. The planner would not get
/// the branch it named, so the disagreement is refused at submission rather than
/// resolved silently.
fn validate_retry_pin(node: &Node) -> Result<Node> {
    if let (Some(branch), Some(resume)) = (&node.branch, &node.resume) {
        if &resume.branch != branch {
            return Err(refuse(format!(
                "retry: replacement '{}' pins branch '{branch}' but resumes branch '{}'; \
                 a retry may name only one branch",
                node.id, resume.branch
            )));
        }
    }
    Ok(node.clone())
}

/// A replacement that names no branch of its own continues the one the node it
/// supersedes left behind.
///
/// The superseded node's attempt ran, committed, and stopped, so its branch
/// holds work. A replacement that cut a fresh branch beside it would retry the
/// publication against an empty tree and leave the committed work for a person
/// to find. The pin is on the node because the fold put it there when the
/// attempt settled — see `projection::pin_preserved_branch` — so this reads what
/// the run recorded rather than guessing a name.
///
/// A planner who named either field is answered with what they named: naming a
/// branch is a decision somebody made after reading the result.
fn inherit_preserved_branch(mut node: Node, superseded: &Node) -> Node {
    if node.branch.is_some() || node.resume.is_some() {
        return node;
    }
    node.branch.clone_from(&superseded.branch);
    node.resume.clone_from(&superseded.resume);
    node
}

/// A retry that states a `resume` and no `branch` is pinned to the resume's own
/// branch, because naming a continuation is naming the branch it lives on.
fn pin_retry_branch(mut node: Node) -> Node {
    if node.branch.is_none() {
        if let Some(resume) = &node.resume {
            if !resume.branch.is_empty() {
                node.branch = Some(resume.branch.clone());
            }
        }
    }
    node
}

fn compile_cancel(graph: &mut Graph, frontier: &Frontier, id: &str) -> Result<Vec<Operation>> {
    let Some(node) = graph.get(id) else {
        return Err(refuse(format!("cancel: no node '{id}'")));
    };
    // The definition is the authoritative record of parking, not the frontier: a
    // node carried into a later round is parked with nothing journalled about it
    // there, so a frontier lookup alone would let the same node be parked twice.
    if node.parked {
        return Err(refuse(format!("cancel: node '{id}' is already parked")));
    }
    match frontier.recorded.get(id) {
        None | Some(NodeStatus::Running) => {}
        Some(status) => {
            return Err(refuse(format!(
                "cancel: node '{id}' is {}, not pending or running",
                status.as_str()
            )))
        }
    }
    if let Some(node) = graph.get_mut(id) {
        node.parked = true;
    }
    Ok(vec![Operation::NodeParked {
        node: id.to_string(),
    }])
}

fn compile_requeue(
    graph: &mut Graph,
    id: &str,
    amend: Option<&Map<String, Value>>,
) -> Result<Vec<Operation>> {
    let Some(node) = graph.get(id).cloned() else {
        return Err(refuse(format!("requeue: no node '{id}'")));
    };
    if !node.parked {
        return Err(refuse(format!("requeue: node '{id}' is not parked")));
    }
    // `id` names the node being requeued and `deps` is `reparent`'s to change;
    // letting an amendment rewrite either would make one op silently do the work
    // of another, with no separate record of the rewiring.
    if let Some(amend) = amend {
        let forbidden: Vec<&str> = ["id", "deps"]
            .into_iter()
            .filter(|key| amend.contains_key(*key))
            .collect();
        if !forbidden.is_empty() {
            return Err(refuse(format!(
                "requeue: cannot amend {}: use 'add' or 'reparent' for that",
                forbidden.join(", ")
            )));
        }
    }

    let mut merged =
        serde_json::to_value(&node).map_err(|e| refuse(format!("requeue: node '{id}': {e}")))?;
    if let (Some(object), Some(amend)) = (merged.as_object_mut(), amend) {
        for (key, value) in amend {
            object.insert(key.clone(), value.clone());
        }
    }
    if let Some(object) = merged.as_object_mut() {
        object.remove("parked");
    }
    // A retired field can only have come from the amendment — the node's own
    // serialization no longer carries one — and it is named here rather than
    // reaching the schema as an unknown field, because a retry that carries a
    // corrected review bar is exactly where a planner writes one.
    if let Some(retired) = crate::plan::retired_field_refusal(&merged) {
        return Err(refuse(format!("requeue: node {retired}")));
    }
    // The amended node is validated as the node it produces, so a malformed pin
    // is refused at submission rather than at the next dispatch.
    let amended: Node = serde_json::from_value(merged)
        .map_err(|e| refuse(format!("requeue: amended node '{id}' is invalid: {e}")))?;
    graph::validate_node(&amended).map_err(|e| refuse(e.to_string()))?;
    graph.insert(amended);

    Ok(vec![Operation::NodeRequeued {
        node: id.to_string(),
        amend: amend.filter(|a| !a.is_empty()).cloned(),
    }])
}

fn compile_attest(frontier: &Frontier, reference: &str) -> Result<Vec<Operation>> {
    if frontier.recorded.get(reference) != Some(&NodeStatus::Waiting) {
        return Err(refuse(format!(
            "attest: '{reference}' is not a ready, waiting human action"
        )));
    }
    if frontier.attestations.contains(reference) {
        return Err(refuse(format!(
            "attest: '{reference}' was already attested"
        )));
    }
    Ok(vec![Operation::HumanAttested {
        node: reference.to_string(),
    }])
}

fn compile_context(
    graph: &mut Graph,
    frontier: &Frontier,
    id: &str,
    note: &str,
    delivery: Delivery,
) -> Result<Vec<Operation>> {
    if !graph.contains(id) {
        return Err(refuse(format!("context: no node '{id}'")));
    }
    if note.trim().is_empty() {
        return Err(refuse("context: the note cannot be empty"));
    }
    // A note is read by the node's running turn or by a *later* dispatch of it.
    // A node that already settled `done` has neither, so the planner is told
    // rather than left believing it landed.
    if frontier.recorded.get(id) == Some(&NodeStatus::Done) {
        return Err(refuse(format!(
            "context: node '{id}' has settled done, so nothing will read the note"
        )));
    }
    // A note the running turn took has been read, so it is not also owed to the
    // next dispatch: attaching it there too would re-state a correction the
    // worker has already acted on.
    if delivery == Delivery::Deferred {
        if let Some(node) = graph.get_mut(id) {
            node.context = Some(note.to_string());
        }
    }
    Ok(vec![Operation::ContextAdded {
        node: id.to_string(),
        note: note.to_string(),
        delivery,
    }])
}

/// Fold one recorded operation back onto a graph.
///
/// This is replay's half of [`compile`]: the reconciler validated and mutated,
/// and this reconstructs the same mutation from the journal without re-judging
/// it. A rejected edit was never recorded, so nothing here can refuse.
pub fn apply(graph: &mut Graph, operation: &Operation) {
    match operation {
        Operation::NodeAdded { node, .. } => graph.insert((**node).clone()),
        Operation::NodeDropped { node, .. } => {
            graph.remove(node);
        }
        Operation::Reparent { node, to, .. } => {
            if let Some(node) = graph.get_mut(node) {
                node.deps.clone_from(to);
            }
        }
        Operation::EdgeRemoved { from, to } => {
            if let Some(node) = graph.get_mut(to) {
                node.deps.retain(|dep| dep != from);
            }
        }
        Operation::EdgeAdded { from, to } => {
            if let Some(node) = graph.get_mut(to) {
                if !node.deps.contains(from) {
                    node.deps.push(from.clone());
                }
            }
        }
        Operation::NodeParked { node } => {
            if let Some(node) = graph.get_mut(node) {
                node.parked = true;
            }
        }
        Operation::NodeRequeued { node, amend } => {
            let Some(existing) = graph.get(node).cloned() else {
                return;
            };
            let mut merged = match serde_json::to_value(&existing) {
                Ok(value) => value,
                Err(_) => return,
            };
            if let Some(object) = merged.as_object_mut() {
                object.remove("parked");
                for (key, value) in amend.iter().flatten() {
                    object.insert(key.clone(), value.clone());
                }
            }
            if let Ok(amended) = serde_json::from_value::<Node>(merged) {
                graph.insert(amended);
            }
        }
        // A live delivery went into a turn rather than onto the graph, so
        // replay reconstructs it by changing nothing — exactly as the
        // reconciler did.
        Operation::ContextAdded {
            node,
            note,
            delivery: Delivery::Deferred,
        } => {
            if let Some(node) = graph.get_mut(node) {
                node.context = Some(note.clone());
            }
        }
        Operation::ContextAdded { .. } => {}
        // Neither mutates the graph: an attestation settles a node and a
        // completion request is journalled for audit, both folded elsewhere.
        Operation::HumanAttested { .. }
        | Operation::CompletionRequested { .. }
        | Operation::RetryRequested { .. } => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plan::{NodeKind, Plan, Resume, PLAN_SCHEMA_VERSION};

    fn agent(id: &str, deps: &[&str]) -> Node {
        Node {
            id: id.into(),
            persona: Some("engineer".into()),
            task: Some("## What\ndo it".into()),
            deps: deps.iter().map(|d| (*d).to_string()).collect(),
            ..Node::default()
        }
    }

    fn graph_of(nodes: Vec<Node>) -> Graph {
        Graph::from_plan(&Plan {
            schema_version: PLAN_SCHEMA_VERSION,
            goal: None,
            name: None,
            concurrency: 4,
            tasks: nodes,
        })
    }

    /// A `context` edit as a planner who says nothing about delivery writes one.
    fn note_for(id: &str, note: &str) -> Command {
        Command::Context {
            id: id.into(),
            note: note.into(),
            deliver: crate::channel::Deliver::default(),
        }
    }

    fn frontier(entries: &[(&str, NodeStatus)]) -> Frontier {
        Frontier {
            recorded: entries
                .iter()
                .map(|(id, status)| ((*id).to_string(), *status))
                .collect(),
            attestations: BTreeSet::new(),
        }
    }

    #[test]
    fn add_inserts_a_node_and_records_its_edges() {
        let mut graph = graph_of(vec![agent("a", &[])]);
        let operations = compile(
            &mut graph,
            &Frontier::default(),
            &Command::Add {
                node: agent("b", &["a"]),
            },
        )
        .expect("the add is legal");
        assert!(graph.contains("b"));
        assert!(matches!(operations[0], Operation::NodeAdded { .. }));
        assert!(
            matches!(&operations[1], Operation::EdgeAdded { from, to } if from == "a" && to == "b")
        );
    }

    #[test]
    fn add_refuses_a_duplicate_id_an_invalid_node_and_a_dangling_dependency() {
        let mut graph = graph_of(vec![agent("a", &[])]);
        let refusals = [
            (
                Command::Add {
                    node: agent("a", &[]),
                },
                "already exists",
            ),
            (
                Command::Add {
                    node: Node {
                        id: "b".into(),
                        ..Node::default()
                    },
                },
                "needs a persona",
            ),
            (
                Command::Add {
                    node: agent("c", &["nowhere"]),
                },
                "not in the plan",
            ),
        ];
        for (command, expected) in refusals {
            let message = compile(&mut graph, &Frontier::default(), &command)
                .unwrap_err()
                .to_string();
            assert!(message.contains(expected), "{message:?} lacks {expected:?}");
        }
        assert_eq!(graph.len(), 1, "a refused add mutated the graph");
    }

    #[test]
    fn an_add_that_would_create_a_cycle_is_refused_and_changes_nothing() {
        let mut graph = graph_of(vec![agent("a", &["b"]), agent("b", &[])]);
        // `b` already exists, so this is a reparent-shaped cycle through add's
        // validation of the resulting graph.
        let message = compile(
            &mut graph,
            &Frontier::default(),
            &Command::Reparent {
                id: "b".into(),
                deps: vec!["a".into()],
            },
        )
        .unwrap_err()
        .to_string();
        assert!(message.contains("cycle"), "{message}");
        assert!(graph.get("b").expect("b").deps.is_empty());
    }

    #[test]
    fn reparent_requires_an_unstarted_node() {
        let mut graph = graph_of(vec![agent("a", &[]), agent("b", &[])]);
        let started = frontier(&[("b", NodeStatus::Running)]);
        let message = compile(
            &mut graph,
            &started,
            &Command::Reparent {
                id: "b".into(),
                deps: vec!["a".into()],
            },
        )
        .unwrap_err()
        .to_string();
        assert!(message.contains("already started"), "{message}");

        let operations = compile(
            &mut graph,
            &Frontier::default(),
            &Command::Reparent {
                id: "b".into(),
                deps: vec!["a".into()],
            },
        )
        .expect("an unstarted node reparents");
        assert_eq!(graph.get("b").expect("b").deps, vec!["a".to_string()]);
        assert!(operations
            .iter()
            .any(|op| matches!(op, Operation::Reparent { .. })));

        assert!(compile(
            &mut graph,
            &Frontier::default(),
            &Command::Reparent {
                id: "nowhere".into(),
                deps: vec![],
            },
        )
        .unwrap_err()
        .to_string()
        .contains("no node"));
    }

    #[test]
    fn drop_detaches_or_recursively_removes_and_must_state_which() {
        let mut graph = graph_of(vec![
            agent("a", &[]),
            agent("b", &["a"]),
            agent("c", &["b"]),
        ]);
        compile(
            &mut graph,
            &Frontier::default(),
            &Command::Drop {
                id: "a".into(),
                dependents: Dependents::Detach,
            },
        )
        .expect("detaching is legal");
        assert!(!graph.contains("a"));
        assert!(graph.get("b").expect("b").deps.is_empty());
        assert!(graph.contains("c"));

        let mut graph = graph_of(vec![
            agent("a", &[]),
            agent("b", &["a"]),
            agent("c", &["b"]),
        ]);
        compile(
            &mut graph,
            &Frontier::default(),
            &Command::Drop {
                id: "a".into(),
                dependents: Dependents::Drop,
            },
        )
        .expect("recursive dropping is legal");
        assert!(graph.is_empty(), "the dependents were not dropped too");
    }

    #[test]
    fn drop_refuses_to_remove_the_last_unresolved_publication_anchor() {
        let lifecycle = |id: &str, deps: &[&str]| Node {
            repo: Some("owner/repo".into()),
            ..agent(id, deps)
        };
        let mut graph = graph_of(vec![
            lifecycle("anchor", &[]),
            lifecycle("stacked", &["anchor"]),
        ]);
        let message = compile(
            &mut graph,
            &Frontier::default(),
            &Command::Drop {
                id: "anchor".into(),
                dependents: Dependents::Detach,
            },
        )
        .unwrap_err()
        .to_string();
        assert!(message.contains("publication anchor"), "{message}");
        assert!(graph.contains("anchor"));

        // Another settled anchor on the same identity makes it legal.
        let mut graph = graph_of(vec![
            lifecycle("anchor", &[]),
            lifecycle("stacked", &["anchor"]),
            lifecycle("landed", &[]),
        ]);
        compile(
            &mut graph,
            &frontier(&[("landed", NodeStatus::Done)]),
            &Command::Drop {
                id: "anchor".into(),
                dependents: Dependents::Detach,
            },
        )
        .expect("a settled alternative anchor allows the drop");
    }

    #[test]
    fn retry_supersedes_only_a_running_failed_or_cancelled_node() {
        let mut graph = graph_of(vec![agent("build", &[]), agent("ship", &["build"])]);
        let message = compile(
            &mut graph,
            &Frontier::default(),
            &Command::Retry {
                id: "build".into(),
                node: agent("build-2", &[]),
            },
        )
        .unwrap_err()
        .to_string();
        assert!(
            message.contains("not running, failed, or cancelled"),
            "{message}"
        );

        let operations = compile(
            &mut graph,
            &frontier(&[("build", NodeStatus::Failed)]),
            &Command::Retry {
                id: "build".into(),
                node: agent("build-2", &[]),
            },
        )
        .expect("a failed node retries");
        assert!(graph.contains("build-2"));
        assert_eq!(
            graph.get("ship").expect("ship").deps,
            vec!["build-2".to_string()],
            "the dependent was not redirected"
        );
        assert!(operations
            .iter()
            .any(|op| matches!(op, Operation::RetryRequested { .. })));
    }

    #[test]
    fn retry_demands_a_new_id() {
        let mut graph = graph_of(vec![agent("build", &[])]);
        let failed = frontier(&[("build", NodeStatus::Failed)]);
        for (node, expected) in [
            (agent("build", &[]), "must be new"),
            (
                Node {
                    id: String::new(),
                    ..agent("x", &[])
                },
                "non-empty id",
            ),
        ] {
            let message = compile(
                &mut graph,
                &failed,
                &Command::Retry {
                    id: "build".into(),
                    node,
                },
            )
            .unwrap_err()
            .to_string();
            assert!(message.contains(expected), "{message:?} lacks {expected:?}");
        }
        assert!(compile(
            &mut graph,
            &failed,
            &Command::Retry {
                id: "nowhere".into(),
                node: agent("x", &[])
            }
        )
        .unwrap_err()
        .to_string()
        .contains("no node"));
    }

    #[test]
    fn a_retry_may_name_only_one_branch() {
        let mut graph = graph_of(vec![Node {
            repo: Some("owner/repo".into()),
            ..agent("build", &[])
        }]);
        let failed = frontier(&[("build", NodeStatus::Failed)]);

        let disagreeing = Node {
            repo: Some("owner/repo".into()),
            branch: Some("pinned".into()),
            resume: Some(Resume {
                branch: "preserved".into(),
                checkpoint: None,
                completed_steps: Vec::new(),
            }),
            ..agent("build-2", &[])
        };
        let message = compile(
            &mut graph,
            &failed,
            &Command::Retry {
                id: "build".into(),
                node: disagreeing,
            },
        )
        .unwrap_err()
        .to_string();
        assert!(message.contains("only one branch"), "{message}");

        // A resume with no pin *is* the pin.
        let resuming = Node {
            repo: Some("owner/repo".into()),
            resume: Some(Resume {
                branch: "preserved".into(),
                checkpoint: Some("abc123".into()),
                completed_steps: Vec::new(),
            }),
            ..agent("build-3", &[])
        };
        compile(
            &mut graph,
            &failed,
            &Command::Retry {
                id: "build".into(),
                node: resuming,
            },
        )
        .expect("naming a continuation is naming its branch");
        assert_eq!(
            graph.get("build-3").expect("build-3").branch.as_deref(),
            Some("preserved")
        );
    }

    #[test]
    fn cancel_parks_a_pending_or_running_node_and_nothing_else() {
        let mut graph = graph_of(vec![agent("sweep", &[])]);
        compile(
            &mut graph,
            &Frontier::default(),
            &Command::Cancel { id: "sweep".into() },
        )
        .expect("a pending node parks");
        assert!(graph.get("sweep").expect("sweep").parked);

        let message = compile(
            &mut graph,
            &Frontier::default(),
            &Command::Cancel { id: "sweep".into() },
        )
        .unwrap_err()
        .to_string();
        assert!(message.contains("already parked"), "{message}");

        let mut graph = graph_of(vec![agent("done", &[])]);
        let message = compile(
            &mut graph,
            &frontier(&[("done", NodeStatus::Done)]),
            &Command::Cancel { id: "done".into() },
        )
        .unwrap_err()
        .to_string();
        assert!(message.contains("not pending or running"), "{message}");

        assert!(compile(
            &mut graph,
            &Frontier::default(),
            &Command::Cancel {
                id: "nowhere".into()
            }
        )
        .unwrap_err()
        .to_string()
        .contains("no node"));
    }

    #[test]
    fn requeue_returns_a_parked_node_and_refuses_to_rewrite_id_or_deps() {
        let mut parked = agent("sweep", &[]);
        parked.parked = true;
        let mut graph = graph_of(vec![parked]);

        for key in ["id", "deps"] {
            let mut amend = Map::new();
            amend.insert(key.to_string(), Value::String("other".into()));
            let message = compile(
                &mut graph,
                &Frontier::default(),
                &Command::Requeue {
                    id: "sweep".into(),
                    amend: Some(amend),
                },
            )
            .unwrap_err()
            .to_string();
            assert!(message.contains("cannot amend"), "{message}");
        }

        let mut amend = Map::new();
        amend.insert("max_turns".into(), Value::from(32));
        let operations = compile(
            &mut graph,
            &Frontier::default(),
            &Command::Requeue {
                id: "sweep".into(),
                amend: Some(amend),
            },
        )
        .expect("a parked node requeues");
        let node = graph.get("sweep").expect("sweep");
        assert!(!node.parked);
        assert_eq!(node.max_turns, Some(32));
        assert!(matches!(
            &operations[0],
            Operation::NodeRequeued { amend: Some(_), .. }
        ));
    }

    #[test]
    fn a_bare_requeue_records_no_amendment_at_all() {
        let mut parked = agent("sweep", &[]);
        parked.parked = true;
        let mut graph = graph_of(vec![parked]);
        let operations = compile(
            &mut graph,
            &Frontier::default(),
            &Command::Requeue {
                id: "sweep".into(),
                amend: Some(Map::new()),
            },
        )
        .expect("a bare requeue is legal");
        assert!(matches!(
            &operations[0],
            Operation::NodeRequeued { amend: None, .. }
        ));
    }

    #[test]
    fn requeue_refuses_an_unparked_or_unknown_node_and_a_malformed_amendment() {
        let mut graph = graph_of(vec![agent("live", &[])]);
        assert!(compile(
            &mut graph,
            &Frontier::default(),
            &Command::Requeue {
                id: "live".into(),
                amend: None
            }
        )
        .unwrap_err()
        .to_string()
        .contains("not parked"));
        assert!(compile(
            &mut graph,
            &Frontier::default(),
            &Command::Requeue {
                id: "nowhere".into(),
                amend: None
            }
        )
        .unwrap_err()
        .to_string()
        .contains("no node"));

        let mut parked = agent("sweep", &[]);
        parked.parked = true;
        let mut graph = graph_of(vec![parked]);
        let mut amend = Map::new();
        amend.insert("max_turns".into(), Value::String("lots".into()));
        let message = compile(
            &mut graph,
            &Frontier::default(),
            &Command::Requeue {
                id: "sweep".into(),
                amend: Some(amend),
            },
        )
        .unwrap_err()
        .to_string();
        assert!(message.contains("is invalid"), "{message}");
    }

    /// The retry that carried a corrected review bar is where a planner writes
    /// one, so the amendment gets the same named refusal a plan file does — not
    /// the schema's bare `unknown field`.
    #[test]
    fn requeue_refuses_a_retired_field_by_name_and_says_where_the_bar_goes() {
        let mut parked = agent("contract", &[]);
        parked.parked = true;
        let mut graph = graph_of(vec![parked]);
        let mut amend = Map::new();
        amend.insert(
            "done_when".into(),
            Value::String("the gate is green".into()),
        );
        let message = compile(
            &mut graph,
            &Frontier::default(),
            &Command::Requeue {
                id: "contract".into(),
                amend: Some(amend),
            },
        )
        .unwrap_err()
        .to_string();
        assert!(message.contains("'contract':"), "{message}");
        assert!(
            message.contains("`done_when` is no longer a plan field"),
            "{message}"
        );
        assert!(
            message.contains("`## Acceptance criteria` section of its own task"),
            "the refusal does not say where the bar goes: {message}"
        );
        assert!(!message.contains("unknown field"), "{message}");
    }

    #[test]
    fn attest_needs_a_ready_waiting_action_that_is_not_already_done() {
        let graph_node = Node {
            id: "approve".into(),
            kind: NodeKind::Human,
            task: Some("approve it".into()),
            ..Node::default()
        };
        let mut graph = graph_of(vec![graph_node]);

        assert!(compile(
            &mut graph,
            &Frontier::default(),
            &Command::Attest {
                reference: "approve".into()
            }
        )
        .unwrap_err()
        .to_string()
        .contains("not a ready, waiting human action"));

        let waiting = frontier(&[("approve", NodeStatus::Waiting)]);
        compile(
            &mut graph,
            &waiting,
            &Command::Attest {
                reference: "approve".into(),
            },
        )
        .expect("a waiting action attests");

        let mut already = waiting.clone();
        already.attestations.insert("approve".into());
        assert!(compile(
            &mut graph,
            &already,
            &Command::Attest {
                reference: "approve".into()
            }
        )
        .unwrap_err()
        .to_string()
        .contains("already attested"));
    }

    #[test]
    fn context_reaches_a_node_that_can_still_be_dispatched() {
        let mut graph = graph_of(vec![agent("build", &[])]);
        let operations = compile(
            &mut graph,
            &Frontier::default(),
            &note_for("build", "the fixture moved"),
        )
        .expect("a live node takes a note");
        assert_eq!(
            graph.get("build").expect("build").context.as_deref(),
            Some("the fixture moved")
        );
        assert!(
            matches!(
                &operations[0],
                Operation::ContextAdded {
                    delivery: Delivery::Deferred,
                    ..
                }
            ),
            "a note nobody delivered is owed to the next dispatch: {operations:?}"
        );

        for (frontier_state, command, expected) in [
            (
                frontier(&[("build", NodeStatus::Done)]),
                note_for("build", "too late"),
                "settled done",
            ),
            (
                Frontier::default(),
                note_for("build", "   "),
                "cannot be empty",
            ),
            (Frontier::default(), note_for("nowhere", "hello"), "no node"),
        ] {
            let message = compile(&mut graph, &frontier_state, &command)
                .unwrap_err()
                .to_string();
            assert!(message.contains(expected), "{message:?} lacks {expected:?}");
        }
    }

    /// A note the running turn took is not also owed to the next dispatch —
    /// otherwise the round transition would re-state a correction the worker has
    /// already acted on.
    #[test]
    fn a_note_delivered_live_leaves_nothing_on_the_node_for_a_later_dispatch() {
        let mut graph = graph_of(vec![agent("build", &[])]);
        let operations = compile_with(
            &mut graph,
            &frontier(&[("build", NodeStatus::Running)]),
            &note_for("build", "the fixture moved"),
            Delivery::Live,
        )
        .expect("a running node takes a note into its turn");
        assert_eq!(
            graph.get("build").expect("build").context,
            None,
            "a live note was also queued for the next dispatch"
        );
        assert!(
            matches!(
                &operations[0],
                Operation::ContextAdded {
                    delivery: Delivery::Live,
                    note,
                    ..
                } if note == "the fixture moved"
            ),
            "{operations:?}"
        );

        // And replay reconstructs exactly that: nothing on the graph.
        let mut replayed = graph_of(vec![agent("build", &[])]);
        apply(&mut replayed, &operations[0]);
        assert_eq!(replayed.get("build").expect("build").context, None);
    }

    /// A record written before delivery had modes says nothing about it, and the
    /// only thing those records ever did is what an absent value must mean.
    #[test]
    fn a_context_operation_from_before_this_field_replays_as_deferred() {
        let operation: Operation = serde_json::from_value(serde_json::json!({
            "kind": "context-added",
            "node": "build",
            "note": "the fixture moved",
        }))
        .expect("an operation without a delivery still parses");
        let mut graph = graph_of(vec![agent("build", &[])]);
        apply(&mut graph, &operation);
        assert_eq!(
            graph.get("build").expect("build").context.as_deref(),
            Some("the fixture moved"),
            "an older record stopped attaching its note"
        );
    }

    #[test]
    fn complete_journals_a_reason_without_touching_the_graph() {
        let mut graph = graph_of(vec![agent("a", &[])]);
        let before = graph.clone();
        let operations = compile(
            &mut graph,
            &Frontier::default(),
            &Command::Complete {
                reason: "publication verified".into(),
            },
        )
        .expect("complete is always legal");
        assert_eq!(graph, before);
        assert!(matches!(
            &operations[0],
            Operation::CompletionRequested { reason } if reason == "publication verified"
        ));
    }

    #[test]
    fn every_compiled_operation_replays_onto_the_same_graph() {
        let mut live = graph_of(vec![agent("a", &[]), agent("b", &["a"])]);
        let mut replayed = live.clone();
        let frontier_state = frontier(&[("a", NodeStatus::Failed)]);

        for command in [
            Command::Add {
                node: agent("c", &["a"]),
            },
            note_for("b", "a note"),
            Command::Retry {
                id: "a".into(),
                node: agent("a-2", &[]),
            },
            Command::Cancel { id: "c".into() },
            Command::Requeue {
                id: "c".into(),
                amend: None,
            },
            Command::Drop {
                id: "c".into(),
                dependents: Dependents::Detach,
            },
        ] {
            let operations =
                compile(&mut live, &frontier_state, &command).expect("each command is legal");
            for operation in &operations {
                apply(&mut replayed, operation);
            }
        }
        assert_eq!(replayed, live, "replay did not reconstruct the live graph");
    }

    #[test]
    fn replaying_an_operation_against_a_graph_that_lost_its_node_is_a_no_op() {
        let mut graph = graph_of(vec![agent("a", &[])]);
        for operation in [
            Operation::Reparent {
                node: "gone".into(),
                from: vec![],
                to: vec!["a".into()],
            },
            Operation::EdgeAdded {
                from: "a".into(),
                to: "gone".into(),
            },
            Operation::EdgeRemoved {
                from: "a".into(),
                to: "gone".into(),
            },
            Operation::NodeParked {
                node: "gone".into(),
            },
            Operation::NodeRequeued {
                node: "gone".into(),
                amend: None,
            },
            Operation::ContextAdded {
                node: "gone".into(),
                note: "n".into(),
                delivery: Delivery::Deferred,
            },
            Operation::HumanAttested {
                node: "gone".into(),
            },
            Operation::CompletionRequested { reason: "r".into() },
        ] {
            apply(&mut graph, &operation);
        }
        assert_eq!(graph.len(), 1);
    }

    #[test]
    fn an_added_edge_is_not_duplicated_on_replay() {
        let mut graph = graph_of(vec![agent("a", &[]), agent("b", &["a"])]);
        apply(
            &mut graph,
            &Operation::EdgeAdded {
                from: "a".into(),
                to: "b".into(),
            },
        );
        assert_eq!(graph.get("b").expect("b").deps, vec!["a".to_string()]);
    }
}
