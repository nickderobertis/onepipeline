//! The task a node's dispatch is given, composed in one place.
//!
//! A dispatch composes a node's task from the node and four things the run knows at the
//! moment it dispatches: the out-of-repository dependencies its branch builds against (the
//! reference block), the stack it is drafted on, the manager's notes a resumed attempt
//! carries, and — in a mixed plan — its side of the repository boundary. The direct dispatch,
//! the lifecycle step dispatch and the plan-edit hook all compose through [`direct`] and
//! [`step`], and the hook gathers those four through [`Inputs::now`] from the same state the
//! dispatching loop holds — so what the hook is shown is what the next dispatch is given.
//!
//! What only the session itself can know — a provisioning failure, a merge left in progress —
//! is added to the first turn after the session opens, and is no part of this composition.

use std::collections::BTreeMap;

use crate::graph::NodeStatus;
use crate::ledger::RunPaths;
use crate::plan::{CrossRepoReference, Node, NodeKind, Step};
use crate::projection::RunState;

/// The task a direct node's dispatch is given.
pub(crate) fn direct(
    node: &Node,
    references: &[CrossRepoReference],
    boundary: Option<&str>,
) -> String {
    crate::boundary::with_section(node.rendered_task_with(references), boundary)
}

/// The task one step of a lifecycle node is given: the step's own prose with the node's
/// amendment, note and references, the notes a resumed attempt carries, the Stack section
/// where the node is drafted on stack parents, and the boundary section last.
pub(crate) fn step(
    step: &Step,
    node: &Node,
    references: &[CrossRepoReference],
    notes: &[crate::note::RecordedNote],
    stack: Option<&crate::stack::Stack>,
    boundary: Option<&str>,
) -> String {
    let task = step.rendered_task_carrying(node, references, notes);
    let task = match stack {
        Some(stack) => format!("{task}\n\n{}", stack.task_section()),
        None => task,
    };
    crate::boundary::with_section(task, boundary)
}

/// Everything beside the node a dispatch composes its task from, as the run stands now.
pub(crate) struct Inputs {
    /// The node as the dispatch composes it: a resumed attempt's own, where one is pending.
    pub(crate) node: Node,
    pub(crate) references: Vec<CrossRepoReference>,
    pub(crate) notes: Vec<crate::note::RecordedNote>,
    pub(crate) stack: Option<crate::stack::Stack>,
    pub(crate) boundary: Option<String>,
    /// The steps a resumed attempt has already finished, which its dispatch skips.
    pub(crate) completed: Vec<String>,
}

impl Inputs {
    /// What `node`'s next dispatch would be composed from, read exactly as the dispatching
    /// loop reads it: the reference block from `references` — the release watch's rows — the
    /// stack from [`crate::stack::placement`], the boundary section from
    /// [`crate::boundary::section`] over the plan as it stands, and, where an attempt of the
    /// node is waiting to resume,
    /// that attempt's node and the notes the record now says it carries.
    ///
    /// # Errors
    ///
    /// A note record this build cannot read, as the dispatch itself would meet it.
    pub(crate) fn now(
        paths: &RunPaths,
        state: &RunState,
        statuses: &BTreeMap<String, NodeStatus>,
        node: &Node,
        references: Vec<CrossRepoReference>,
        resuming: Option<&crate::lifecycle::Continuation>,
        same_identity: &dyn Fn(&str, &str) -> bool,
    ) -> crate::error::Result<Self> {
        let plan_name = state
            .plan
            .as_ref()
            .and_then(|plan| plan.name.clone())
            .unwrap_or_else(|| paths.run.clone());
        let stack = crate::stack::placement(state, statuses, node, &plan_name, same_identity)
            .ok()
            .flatten();
        let plan = crate::boundary::plan_now(state);
        let boundary = crate::boundary::section(&crate::boundary::of_run(&plan), &plan, node);
        let (node, notes, completed) = match resuming {
            Some(continuation) => (
                continuation.node.clone(),
                crate::note::standing_for(paths, &node.id)?.notes(),
                continuation
                    .node
                    .resume
                    .as_ref()
                    .map(|resume| resume.completed_steps.clone())
                    .unwrap_or_default(),
            ),
            None => (
                node.clone(),
                Vec::new(),
                node.resume
                    .as_ref()
                    .map(|resume| resume.completed_steps.clone())
                    .unwrap_or_default(),
            ),
        };
        Ok(Self {
            node,
            references,
            notes,
            stack,
            boundary,
            completed,
        })
    }

    /// The task the node's next dispatch is given: a direct node's own, or the first agent
    /// step of a lifecycle node its attempt has not finished — the step that dispatch is.
    pub(crate) fn next_task(&self) -> String {
        let node = &self.node;
        if node.repo.is_none() {
            return direct(node, &self.references, self.boundary.as_deref());
        }
        let steps = crate::lifecycle::ordered_steps(node).unwrap_or_default();
        let declared = node.steps.is_some();
        let next = steps.iter().find(|candidate| {
            candidate.kind == NodeKind::Agent
                && !(declared && self.completed.iter().any(|done| done == &candidate.id))
        });
        match next {
            Some(next) => step(
                next,
                node,
                &self.references,
                &self.notes,
                self.stack.as_ref(),
                self.boundary.as_deref(),
            ),
            // Nothing left to dispatch: a node of human steps alone is shown its own prose.
            None => node.rendered_task(),
        }
    }
}

/// The task `node`'s next dispatch would be given by a driver starting on the run now: its
/// release watch freshly started from the run's record and asked about this node, and no
/// attempt of it pending a resume — a resumed attempt lives in the driver that held it, and
/// that driver is gone. What the process applying an edit with nothing driving the run shows
/// the plan-edit hook.
///
/// # Errors
///
/// A note record this build cannot read, as the dispatch itself would meet it.
pub(crate) fn of_a_fresh_driver(
    paths: &RunPaths,
    state: &RunState,
    node: &Node,
) -> crate::error::Result<String> {
    let mut releases = crate::release::Watch::of_run(paths);
    releases.refresh(paths, state, std::slice::from_ref(node));
    let references = releases.references(node);
    let statuses = state.statuses();
    Ok(Inputs::now(
        paths,
        state,
        &statuses,
        node,
        references,
        None,
        &crate::engine::same_identity,
    )?
    .next_task())
}
