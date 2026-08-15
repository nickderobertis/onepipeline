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

use crate::controls::NodeControls;
use crate::engine::{self, Message, Settlement};
use crate::event::{Envelope, Labels};
use crate::executor::{DispatchRequest, Executor, WorkspaceSpec};
use crate::graph::NodeStatus;
use crate::plan::{Node, NodeKind, Step};

/// The persona that drafts a change request's title and body.
pub const PR_AUTHOR_PERSONA: &str = "pr-author";

/// Run one lifecycle node to settlement.
// llmlint: ignore-block[invalid_states_unrepresentable] `default_graph` is the same
// validated, launch-recorded string carried by the engine. Lifecycle only passes it into
// oneagentgraph's transparent ConfigRef; another newtype would duplicate that sibling
// type and widen this path-resolution change across unrelated composition.
pub fn execute(
    executor: &dyn Executor,
    run: &str,
    default_graph: &str,
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
    // Every step's controls are narrowed here, before a session exists: a
    // workstream that cannot dispatch one of its steps must not first cut a
    // branch and run the steps before it, because that leaves work on a branch
    // for a node that was never going to finish.
    // llmlint: ignore-block[changed_behavior_has_e2e] what this arm newly carries — a
    // step whose declaration no dispatch can run under — is refused by `graph::validate`
    // at `start` and at every live edit, so only a graph
    // folded from a journal an *earlier build* wrote reaches it. Reaching that end to
    // end means writing that journal by hand, which proves the fixture rather than the
    // code, and deleting the arm would reinstate the silent default this control exists
    // to remove. Held instead by the unit test below, which drives the real
    // `LocalExecutor`; the step cycle this arm already reported keeps its own journey.
    let steps = match dispatchable_steps(node) {
        Ok(steps) => steps,
        Err(reason) => {
            return Settlement {
                detail: Some(reason),
                ..Settlement::plain(&node.id, NodeStatus::Failed, Some("invalid-node"))
            }
        }
    }; // llmlint: ignore-end[changed_behavior_has_e2e]

    let mut session: Option<String> = None;
    // The session's own stream, followed from the moment there is a token to
    // follow, so the publication that comes after the steps is visible while it
    // runs rather than only once it is over.
    let mut stream: Option<crate::vcs::Follower> = None;
    // The one worktree this node's dispatches work in, once its session has
    // opened one. See where it is read: every dispatch after the first runs
    // *there* rather than opening a session beside it.
    let mut worktree: Option<std::path::PathBuf> = None;
    // Where in the run the session's own envelopes belong. The node, not a
    // step: a session outlives every step that wrote in it, and the publication
    // that follows them belongs to none.
    let whose = engine::dispatch_labels(run, &node.id, None, node.persona.as_deref());
    let mut branch: Option<String> = node.branch.clone();
    // The steps the preserved branch already carries, plus the ones this attempt
    // adds. Carried forward whole, because the branch a later attempt preserves
    // is the same branch: a step skipped on one attempt is still on it.
    let mut completed: Vec<String> = node
        .resume
        .as_ref()
        .map(|resume| resume.completed_steps.clone())
        .unwrap_or_default();

    for (step, controls) in &steps {
        if declared_steps && completed.iter().any(|id| id == &step.id) {
            // Already on the preserved branch. Re-running it would redo work the
            // branch carries, which for a step that opened a change is not
            // idempotent.
            continue;
        }
        if step.kind == NodeKind::Human {
            // A ready human step needs a person, and the workstream holds its
            // branch until one acts. The harness never infers that it happened.
            // The session stays open for them and the follow does not: dropping
            // it here ends a process that would otherwise read a stream nobody
            // is waiting for, for as long as the driver lives.
            return Settlement {
                branch,
                completed_steps: completed,
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
        // And works in the worktree the first step's session opened, rather than
        // asking for a session of its own. `onevcs` cuts every session its own
        // clone from the execution checkout, so a second session on the same
        // branch starts from the base with none of the earlier steps' work — and
        // opening it reclaims the first session's workspace, uncommitted work
        // and all. Steps are serial by construction, so one worktree is what
        // "several steps share one branch" has always meant.
        let workspace = match &worktree {
            Some(dir) => WorkspaceSpec::Path(dir.clone()),
            None => WorkspaceSpec::VcsSession(request.clone()),
        };
        let graph = engine::node_graph(
            step.agent_graph.as_ref().or(node.agent_graph.as_ref()),
            default_graph,
        );
        let build = || DispatchRequest {
            graph: graph.clone(),
            task: step.rendered_task(node.context.as_deref()),
            labels: engine::dispatch_labels(
                run,
                &node.id,
                declared_steps.then_some(step.id.as_str()),
                step.persona.as_deref(),
            ),
            controls: *controls,
            workspace: workspace.clone(),
            cancel: cancel.clone(),
        };
        let drained = engine::attempt(executor, &node.id, cancel, tx, &build);
        // The session the dispatch opened is what publication needs, whether or
        // not the step succeeded: a cancelled step's commits are preserved on
        // the branch it left behind.
        session = drained.session.or(session);
        branch = drained.branch.or(branch);
        if stream.is_none() {
            if let Some(token) = &session {
                worktree = crate::vcs::worktree_of(token);
                stream = crate::vcs::follow(token, relay_into(tx, whose.clone()));
            }
        }
        if drained.settlement.status != NodeStatus::Done {
            end_session(stream, tx, session.as_deref(), &whose);
            return Settlement {
                branch,
                completed_steps: completed,
                ..drained.settlement
            };
        }
        if declared_steps {
            completed.push(step.id.clone());
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

    let settlement = publish(
        executor,
        run,
        default_graph,
        node,
        worktree.as_deref(),
        cancel,
        tx,
        &token,
        branch,
    );
    end_session(stream, tx, Some(&token), &whose);
    settlement
}

/// Draft the change request, then publish through `onevcs`.
#[allow(
    clippy::too_many_arguments,
    reason = "publication needs the dispatch context (executor, run, node, cancel, \
              stream) as well as what the steps left behind (the session token, its branch, \
              and the worktree they worked in); the first six are the node's own dispatch \
              identity and bundling them would only move the same list one indirection away"
)]
fn publish(
    executor: &dyn Executor,
    run: &str,
    default_graph: &str,
    node: &Node,
    worktree: Option<&std::path::Path>,
    cancel: &crate::executor::CancellationToken,
    tx: &Sender<Message>,
    token: &str,
    branch: Option<String>,
) -> Settlement {
    let title = node
        .title
        .clone()
        .unwrap_or_else(|| draft_title(executor, run, default_graph, node, worktree, cancel, tx));

    match crate::vcs::publish(token, node.merge_policy, Some(&title)) {
        Ok(published) => {
            // A publication that did not land is an ending of the publication,
            // not a refused request: `onevcs` draws that line itself, in
            // `PublishOutcome::Failed`, and this crate reads its line rather
            // than a second one. The reason is the sibling's own — the gate it
            // ran and what that said — and it is what the node settles with.
            if let onevcs::PublishOutcome::Failed { reason, .. } = &published.outcome {
                return Settlement {
                    branch,
                    detail: Some(format!("onevcs: {reason}")),
                    ..Settlement::plain(&node.id, NodeStatus::Failed, Some("publication-failed"))
                };
            }
            let labels = engine::dispatch_labels(run, &node.id, None, node.persona.as_deref());
            let _ = tx.send(Message::Event(Box::new(crate::vcs::published_event(
                &published, &labels,
            ))));
            Settlement {
                // The branch the publication says carried the change, where a
                // dispatch reported none: they are the same branch, and the
                // sibling is the one that knows it.
                branch: branch.or_else(|| Some(published.branch.clone())),
                change_url: crate::vcs::change_url(&published.outcome),
                // Every ending has its own name. A branch whose base already
                // carries it settles `no-changes` rather than a bare
                // "published", which is what let a node whose worker wrote
                // nothing report as one that landed work.
                outcome: Some(crate::vcs::outcome_of(&published.outcome).to_owned()),
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
///
/// It runs in the node's **own** worktree, which is the only place the diff it is
/// asked to read exists: a session of its own would be a fresh clone cut from the
/// base, carrying nothing this node wrote — and opening one reclaims the session
/// still holding the work.
#[allow(
    clippy::too_many_arguments,
    reason = "the draft is a dispatch inside one lifecycle execution and needs that execution's \
              executor, labels, resolved graph, workspace, cancellation, and event stream"
)]
fn draft_title(
    executor: &dyn Executor,
    run: &str,
    default_graph: &str,
    node: &Node,
    worktree: Option<&std::path::Path>,
    cancel: &crate::executor::CancellationToken,
    tx: &Sender<Message>,
) -> String {
    let fallback = deterministic_title(node);
    let Some(request) = crate::vcs::request_for(node) else {
        return fallback;
    };
    let workspace = match worktree {
        Some(dir) => WorkspaceSpec::Path(dir.to_path_buf()),
        None => WorkspaceSpec::VcsSession(SessionRequest {
            branch: node.branch.clone().or(request.branch.clone()),
            ..request
        }),
    };
    let dispatch = executor.dispatch(DispatchRequest {
        graph: engine::node_graph(None, default_graph),
        task: format!(
            "Read this branch's diff and write the change request's title and body, \
             following the repository's own template. The task this branch delivered:\n\n{}",
            node.rendered_task()
        ),
        labels: engine::dispatch_labels(run, &node.id, None, Some(PR_AUTHOR_PERSONA)),
        // None of the node's own: the drafting dispatch is not the node's work,
        // and a turn budget written for that work would be spent twice — once on
        // it and once here — if this dispatch inherited it.
        controls: crate::controls::NodeControls::default(),
        workspace,
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
// llmlint: ignore-end[invalid_states_unrepresentable]

/// The title a change gets when nothing drafted one.
///
/// Derived from what the plan already states, so it is the same title every
/// time rather than a guess that varies per run.
pub fn deterministic_title(node: &Node) -> String {
    node.title
        .clone()
        .unwrap_or_else(|| format!("chore: {}", node.id))
}

/// Put every envelope a followed session writes into the merged stream.
fn relay_into(tx: &Sender<Message>, node: Labels) -> Box<dyn Fn(Envelope) + Send> {
    let tx = tx.clone();
    Box::new(move |mut envelope| {
        stamp(&mut envelope.labels, &node);
        let _ = tx.send(Message::Event(Box::new(envelope)));
    })
}

/// Say which node a session's envelope belongs to, where its producer could not.
///
/// `onevcs` stamps what it knows, and a session does not know it is a graph
/// node: the crate that opened it does. Without this a whole publication —
/// gate, push, change request, merge — lands in the merged store belonging to
/// no node, so every per-node view reads it as work that happened to nobody.
///
/// An enricher, so it never rewrites: a key the producer stamped stands.
fn stamp(labels: &mut Labels, known: &Labels) {
    labels.run_id = labels.run_id.take().or_else(|| known.run_id.clone());
    labels.node = labels.node.take().or_else(|| known.node.clone());
    labels.persona = labels.persona.take().or_else(|| known.persona.clone());
}

/// Close the session, and collect what its stream said.
///
/// Closing comes first, because it is what ends the follow *and* what writes the
/// session's last record: closing marks the session closed before it emits
/// `session-closed`, and the follow ends as soon as it reads a session closed. A
/// follow can therefore end cleanly having relayed everything but the tail — so
/// the stream is always read once more afterwards, from the point the follow
/// reached. A gap in the merged store is what makes a later reader think nothing
/// happened.
fn end_session(
    stream: Option<crate::vcs::Follower>,
    tx: &Sender<Message>,
    token: Option<&str>,
    node: &Labels,
) {
    close(token);
    // `None` from either side is the whole stream still to read: no follow was
    // started, or one was and relayed nothing.
    let followed_through = stream.and_then(crate::vcs::Follower::finish);
    relay_session_events(tx, token, node, followed_through);
}

/// Fold the part of the session's stream nothing has relayed into the merged one.
///
/// `onevcs` records the gate, the commits, and the publication against the
/// session; without this the merged store would carry a lifecycle node's
/// settlement with none of the evidence behind it. `followed_through` is the
/// highest `seq` the follow already relayed, so a record arrives **once**: the
/// stream is numbered monotonically and resumes its series across the processes
/// that write to it, which makes that one number the whole of the bookkeeping.
fn relay_session_events(
    tx: &Sender<Message>,
    token: Option<&str>,
    node: &Labels,
    followed_through: Option<u64>,
) {
    let Some(token) = token else { return };
    let relay = relay_into(tx, node.clone());
    for envelope in beyond(crate::vcs::events(token), followed_through) {
        relay(envelope);
    }
}

/// The part of a stream a follow did not already relay.
///
/// `None` is the whole of it — no follow was started, or one was and relayed
/// nothing — which is a stream still to read rather than a stream that held
/// nothing. Otherwise everything numbered past the highest `seq` the follow
/// reached, and nothing at or below it: a record relayed twice is the same
/// defect as one lost, seen from the other side.
fn beyond(envelopes: Vec<Envelope>, followed_through: Option<u64>) -> Vec<Envelope> {
    envelopes
        .into_iter()
        .filter(|envelope| !followed_through.is_some_and(|seq| envelope.seq <= seq))
        .collect()
}

fn close(token: Option<&str>) {
    // Best effort: a node that already failed must not be reported as a
    // different failure because its cleanup also failed.
    if let Some(token) = token {
        let _ = crate::vcs::session_close(token);
    }
}

/// A node's steps in dispatch order, each with the controls its dispatch runs
/// under, or why the node has none it can run.
///
/// One function for both refusals a workstream can carry before it starts — a
/// dependency cycle among its steps, and a step whose declaration no dispatch
/// can honour — because they cost the same thing if they are found late: a
/// branch cut for a node that was never going to finish.
fn dispatchable_steps(node: &Node) -> std::result::Result<Vec<(Step, NodeControls)>, String> {
    ordered_steps(node)?
        .into_iter()
        .map(|step| {
            NodeControls::of_step(&step)
                .map(|controls| (step.clone(), controls))
                .map_err(|why| format!("node '{}': step '{}': {why}", node.id, step.id))
        })
        .collect()
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

    /// A workstream refuses before it cuts a branch.
    ///
    /// The step's declaration is one no dispatch can run under, and it is found
    /// while the node is still a document: nothing here opens a session, so the
    /// refusal cannot leave commits on a branch for a node that was never going
    /// to finish. `execute` reports it through the arm a step cycle already
    /// takes.
    #[test]
    fn a_step_whose_budget_no_dispatch_can_run_under_stops_the_workstream() {
        let node = Node {
            id: "service".into(),
            repo: Some("owner/service".into()),
            steps: Some(vec![
                Step {
                    max_turns: Some(45),
                    ..step("implement", &[])
                },
                Step {
                    max_turns: Some(0),
                    ..step("review", &["implement"])
                },
            ]),
            ..Node::default()
        };
        let why = dispatchable_steps(&node)
            .expect_err("a step that can take no turn is not dispatchable");
        assert!(why.contains("node 'service': step 'review':"), "{why}");
        assert!(why.contains("no turn at all"), "{why}");

        // And the workstream itself stops there. The repository is one nothing
        // has registered, so if this refusal came any later the failure would be
        // `onevcs`'s — which is the same as saying a branch would already exist
        // for a node that was never going to finish. The executor is the real
        // one for the same reason: a regression here would go looking for
        // `oneagentgraph` rather than quietly running the step.
        let (tx, rx) = std::sync::mpsc::channel();
        let settlement = execute(
            &crate::executor::LocalExecutor,
            "demo",
            "graphs/node-scope.yaml",
            &node,
            &crate::executor::CancellationToken::new(),
            &tx,
        );
        assert_eq!(settlement.status, NodeStatus::Failed);
        assert_eq!(settlement.outcome.as_deref(), Some("invalid-node"));
        let detail = settlement.detail.expect("the settlement says why");
        assert!(detail.contains("step 'review'"), "{detail}");
        assert!(detail.contains("no turn at all"), "{detail}");
        assert_eq!(
            rx.try_iter().count(),
            0,
            "a workstream that could not dispatch a step opened a session anyway"
        );

        // The step that *can* run keeps the budget it declared, narrowed.
        let node = Node {
            steps: Some(vec![Step {
                max_turns: Some(45),
                ..step("implement", &[])
            }]),
            ..node
        };
        let dispatchable = dispatchable_steps(&node).expect("45 is a budget a step can run under");
        assert_eq!(
            dispatchable[0].1.max_turns,
            std::num::NonZeroU32::new(45),
            "the step's own budget did not survive the conversion"
        );
    }

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

    /// The record a follow ended one read short of, relayed exactly once.
    ///
    /// The window this covers is inside a library call now — closing a session
    /// flips its record and only then writes `session-closed`, and the follow
    /// reads and only then asks whether the session closed — so it cannot be
    /// forced from an e2e the way a delayed subprocess once could. This is the
    /// arithmetic that makes "once" true either way, held on its own.
    #[test]
    fn relays_only_what_the_follow_did_not() {
        let wrote = |seq: u64| Envelope {
            v: crate::event::ENVELOPE_VERSION,
            ts: "2026-01-01T00:00:00.000Z".into(),
            stream: "s-1".into(),
            seq,
            source: crate::event::Source::Vcs,
            kind: crate::event::EventKind("session-closed".into()),
            labels: Labels::default(),
            payload: serde_json::Map::new(),
            artifacts: Vec::new(),
        };
        let stream: Vec<Envelope> = (1..=4).map(wrote).collect();

        // A follow that reached the third record leaves the tail and nothing
        // else: re-reading the whole stream would put the first three in twice.
        let tail = beyond(stream.clone(), Some(3));
        assert_eq!(tail.iter().map(|e| e.seq).collect::<Vec<_>>(), vec![4]);

        // A follow that ended having relayed everything leaves nothing.
        assert!(beyond(stream.clone(), Some(4)).is_empty());

        // And one that relayed nothing at all leaves the whole stream, which is
        // a stream still to read rather than a stream that held nothing.
        assert_eq!(
            beyond(stream, None)
                .iter()
                .map(|e| e.seq)
                .collect::<Vec<_>>(),
            vec![1, 2, 3, 4]
        );
    }

    #[test]
    fn a_step_ordering_ignores_a_dependency_on_something_outside_the_node() {
        let node = lifecycle(Some(vec![step("only", &["elsewhere"])]));
        let steps = ordered_steps(&node).expect("an outside reference does not deadlock");
        assert_eq!(steps.len(), 1);
    }
}
