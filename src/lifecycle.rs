//! Lifecycle nodes: composing a `onevcs` session with the dispatches that work
//! in it.
//!
//! A lifecycle node names a `repo`, so its work happens on an isolated branch
//! and is published through that repository's registered policy. This module is
//! the composition and nothing more — the branch, the worktree, the merge-path
//! gate, and the publication are all `onevcs`'s, and the dispatch inside them is
//! `oneagentgraph`'s.
//!
//! Several `steps` share one branch and run **serially in topological order**,
//! because concurrent writers cannot safely share a worktree.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::mpsc::Sender;

use onevcs::SessionRequest;

use crate::engine::{self, Message, Settlement};
use crate::executor::{DispatchRequest, Executor, WorkspaceSpec};
use crate::graph::NodeStatus;
use crate::plan::{Node, NodeKind, Step};

/// The persona that drafts a change request's title and body.
pub const PR_AUTHOR_PERSONA: &str = "pr-author";

/// Run one lifecycle node to settlement.
pub fn execute(
    executor: &dyn Executor,
    run: &str,
    round: u64,
    node: &Node,
    cancel: &crate::executor::CancellationToken,
    tx: &Sender<Message>,
) -> Settlement {
    let Some(request) = crate::vcs::request_for(node) else {
        return Settlement {
            detail: Some("a lifecycle node needs a repo".into()),
            ..Settlement::plain(&node.id, NodeStatus::Failed, Some("invalid-node"))
        };
    };

    // A node that declared no steps has one dispatch and no step, so nothing
    // stamps a `step` label the plan never wrote.
    let declared_steps = node.steps.is_some();
    let steps = match ordered_steps(node) {
        Ok(steps) => steps,
        Err(reason) => {
            return Settlement {
                detail: Some(reason),
                ..Settlement::plain(&node.id, NodeStatus::Failed, Some("invalid-node"))
            }
        }
    };

    let mut session: Option<String> = None;
    let mut branch: Option<String> = node.branch.clone();

    for step in &steps {
        if step.kind == NodeKind::Human {
            // A ready human step needs a person, and the workstream holds its
            // branch until one acts. The harness never infers that it happened.
            return Settlement {
                branch,
                ..Settlement::plain(&node.id, NodeStatus::Waiting, None)
            };
        }
        if step.expects_no_diff {
            continue;
        }
        // Every step after the first names the branch the first opened, which
        // is what makes them one workstream rather than several beside it.
        let request = SessionRequest {
            branch: branch.clone().or_else(|| request.branch.clone()),
            ..request.clone()
        };
        let build = || DispatchRequest {
            graph: engine::node_graph(step.agent_graph.as_ref().or(node.agent_graph.as_ref())),
            task: step.rendered_task(node.context.as_deref()),
            labels: engine::dispatch_labels(
                run,
                round,
                &node.id,
                declared_steps.then_some(step.id.as_str()),
                step.persona.as_deref(),
            ),
            workspace: WorkspaceSpec::VcsSession(request.clone()),
            cancel: cancel.clone(),
        };
        let drained = engine::attempt(executor, &node.id, "worker", cancel, tx, &build);
        // The session the dispatch opened is what publication needs, whether or
        // not the step succeeded: a cancelled step's commits are preserved on
        // the branch it left behind.
        session = drained.session.or(session);
        branch = drained.branch.or(branch);
        if drained.settlement.status != NodeStatus::Done {
            relay_session_events(tx, session.as_deref());
            close(session.as_deref());
            return Settlement {
                branch,
                ..drained.settlement
            };
        }
    }

    let Some(token) = session else {
        // Every step declared no diff, so there is nothing to publish and the
        // node settles on the existing no-changes outcome.
        return Settlement {
            branch,
            ..Settlement::plain(&node.id, NodeStatus::Done, Some("no-changes"))
        };
    };

    publish(executor, run, round, node, cancel, tx, &token, branch)
}

/// Draft the change request, then publish through `onevcs`.
#[allow(
    clippy::too_many_arguments,
    reason = "publication needs the dispatch context (executor, run, round, node, cancel, \
              stream) as well as what the steps left behind (the session token and its \
              branch); the first six are the node's own dispatch identity and bundling \
              them would only move the same list one indirection away"
)]
fn publish(
    executor: &dyn Executor,
    run: &str,
    round: u64,
    node: &Node,
    cancel: &crate::executor::CancellationToken,
    tx: &Sender<Message>,
    token: &str,
    branch: Option<String>,
) -> Settlement {
    let title = node
        .title
        .clone()
        .unwrap_or_else(|| draft_title(executor, run, round, node, cancel, tx));

    let published = crate::vcs::publish(token, node.merge_policy, Some(&title));
    relay_session_events(tx, Some(token));
    close(Some(token));
    match published {
        Ok(published) => {
            let labels =
                engine::dispatch_labels(run, round, &node.id, None, node.persona.as_deref());
            let branch_name = branch.clone().unwrap_or_default();
            let _ = tx.send(Message::Event(Box::new(crate::vcs::published_event(
                &published,
                &branch_name,
                &labels,
            ))));
            Settlement {
                branch,
                change_url: published.url.clone(),
                outcome: published.outcome.clone().or(Some("published".into())),
                ..Settlement::plain(&node.id, NodeStatus::Done, None)
            }
        }
        Err(error) => Settlement {
            branch,
            detail: Some(error.to_string()),
            ..Settlement::plain(&node.id, NodeStatus::Failed, Some("publication-failed"))
        },
    }
}

/// One post-verification dispatch drafting the change request's title.
///
/// It runs **after** the branch has been verified and is not on the publication
/// path: a drafting failure falls back to the deterministic title and the change
/// still publishes. That is the whole point of running it here rather than
/// making it a step.
fn draft_title(
    executor: &dyn Executor,
    run: &str,
    round: u64,
    node: &Node,
    cancel: &crate::executor::CancellationToken,
    tx: &Sender<Message>,
) -> String {
    let fallback = deterministic_title(node);
    let Some(request) = crate::vcs::request_for(node) else {
        return fallback;
    };
    let dispatch = executor.dispatch(DispatchRequest {
        graph: engine::node_graph(None),
        task: format!(
            "Read this branch's diff and write the change request's title and body, \
             following the repository's own template. The task this branch delivered:\n\n{}",
            node.rendered_task()
        ),
        labels: engine::dispatch_labels(run, round, &node.id, None, Some(PR_AUTHOR_PERSONA)),
        workspace: WorkspaceSpec::VcsSession(SessionRequest {
            branch: node.branch.clone().or(request.branch.clone()),
            ..request
        }),
        cancel: cancel.clone(),
    });
    let Ok(mut handle) = dispatch else {
        return fallback;
    };
    let mut drafted = None;
    for envelope in handle.events() {
        let Ok(envelope) = envelope else { continue };
        if let Some(title) = envelope.payload.get("title").and_then(|v| v.as_str()) {
            drafted = Some(title.to_string());
        }
        let _ = tx.send(Message::Event(Box::new(envelope)));
    }
    match handle.wait() {
        Ok(outcome) if outcome.succeeded => drafted.unwrap_or(fallback),
        _ => fallback,
    }
}

/// The title a change gets when nothing drafted one.
///
/// Derived from what the plan already states, so it is the same title every
/// time rather than a guess that varies per run.
pub fn deterministic_title(node: &Node) -> String {
    node.title
        .clone()
        .unwrap_or_else(|| format!("chore: {}", node.id))
}

/// Fold the session's own event stream into the merged one.
///
/// `onevcs` records the gate, the commits, and the publication against the
/// session; without this the merged store would carry a lifecycle node's
/// settlement with none of the evidence behind it.
fn relay_session_events(tx: &Sender<Message>, token: Option<&str>) {
    let Some(token) = token else { return };
    for envelope in crate::vcs::events(token) {
        let _ = tx.send(Message::Event(Box::new(envelope)));
    }
}

fn close(token: Option<&str>) {
    // Best effort: a node that already failed must not be reported as a
    // different failure because its cleanup also failed.
    if let Some(token) = token {
        let _ = crate::vcs::session_close(token);
    }
}

/// A node's steps in topological order, or why they have none.
///
/// Steps share one branch and run serially, so the order is a total one: ties
/// are broken by the order the plan wrote them, which keeps a workstream
/// reproducible.
pub fn ordered_steps(node: &Node) -> std::result::Result<Vec<Step>, String> {
    let Some(steps) = &node.steps else {
        // A lifecycle node with no steps is one implicit step: its own persona
        // and task, on its own branch.
        return Ok(vec![Step {
            id: node.id.clone(),
            kind: node.kind,
            task: node.task.clone(),
            persona: node.persona.clone(),
            deps: Vec::new(),
            done_when: node.done_when.clone(),
            max_turns: node.max_turns,
            expects_no_diff: node.expects_no_diff,
            executor: node.executor.clone(),
            agent_graph: node.agent_graph.clone(),
        }]);
    };

    let by_id: BTreeMap<&str, &Step> = steps.iter().map(|s| (s.id.as_str(), s)).collect();
    let mut settled: BTreeSet<&str> = BTreeSet::new();
    let mut order: Vec<Step> = Vec::new();
    while order.len() < steps.len() {
        let mut progressed = false;
        for step in steps {
            if settled.contains(step.id.as_str()) {
                continue;
            }
            if step
                .deps
                .iter()
                .all(|dep| settled.contains(dep.as_str()) || !by_id.contains_key(dep.as_str()))
            {
                settled.insert(step.id.as_str());
                order.push(step.clone());
                progressed = true;
            }
        }
        if !progressed {
            return Err(format!(
                "node '{}': its steps have a dependency cycle",
                node.id
            ));
        }
    }
    Ok(order)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn step(id: &str, deps: &[&str]) -> Step {
        Step {
            id: id.into(),
            persona: Some("engineer".into()),
            task: Some("## What\ndo it".into()),
            deps: deps.iter().map(|d| (*d).to_string()).collect(),
            ..Step::default()
        }
    }

    fn lifecycle(steps: Option<Vec<Step>>) -> Node {
        Node {
            id: "service".into(),
            repo: Some("owner/repo".into()),
            persona: steps.is_none().then(|| "engineer".into()),
            task: steps.is_none().then(|| "## What\nship".into()),
            steps,
            ..Node::default()
        }
    }

    #[test]
    fn steps_run_serially_in_topological_order() {
        let node = lifecycle(Some(vec![
            step("publish", &["review"]),
            step("implement", &[]),
            step("review", &["implement"]),
        ]));
        let order: Vec<String> = ordered_steps(&node)
            .expect("the steps order")
            .into_iter()
            .map(|s| s.id)
            .collect();
        assert_eq!(order, vec!["implement", "review", "publish"]);
    }

    #[test]
    fn steps_with_a_cycle_are_reported_rather_than_run_in_some_order() {
        let node = lifecycle(Some(vec![step("a", &["b"]), step("b", &["a"])]));
        let message = ordered_steps(&node).unwrap_err();
        assert!(message.contains("dependency cycle"), "{message}");
    }

    #[test]
    fn a_lifecycle_node_with_no_steps_is_one_implicit_step() {
        let node = lifecycle(None);
        let steps = ordered_steps(&node).expect("one implicit step");
        assert_eq!(steps.len(), 1);
        assert_eq!(steps[0].id, "service");
        assert_eq!(steps[0].persona.as_deref(), Some("engineer"));
    }

    #[test]
    fn a_deterministic_title_is_the_same_every_time() {
        let node = lifecycle(None);
        assert_eq!(deterministic_title(&node), "chore: service");
        let titled = Node {
            title: Some("feat: ship the thing".into()),
            ..node
        };
        assert_eq!(deterministic_title(&titled), "feat: ship the thing");
    }

    #[test]
    fn a_step_ordering_ignores_a_dependency_on_something_outside_the_node() {
        let node = lifecycle(Some(vec![step("only", &["elsewhere"])]));
        let steps = ordered_steps(&node).expect("an outside reference does not deadlock");
        assert_eq!(steps.len(), 1);
    }
}
