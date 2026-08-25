//! Lifecycle nodes: composing a `onevcs` session with the dispatches that work
//! in it.
//!
//! A lifecycle node names a `repo`, so its work happens on an isolated branch
//! and is published through that repository's registered policy. This module is
//! the composition and nothing more — the branch, the worktree, and the
//! publication are all `onevcs`'s, and the dispatch inside them is
//! `oneagentgraph`'s. Nothing here verifies the change: that is the repository's
//! own merge path, at the publishing push.
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
use crate::filter::EventFilter;
use crate::graph::NodeStatus;
use crate::ledger::RunPaths;
use crate::plan::{Node, NodeKind, Step};

/// The persona that drafts a change request's body.
pub const PR_AUTHOR_PERSONA: &str = "pr-author";

// llmlint: ignore-block[invalid_states_unrepresentable] the graph references below are the
// same validated, launch-recorded strings the engine carries — the launch record's own
// fields, read off it strictly and passed straight back into oneagentgraph's transparent
// ConfigRef. Another newtype would duplicate that sibling type and widen this
// path-resolution change across unrelated composition.
/// What this run's launch decides about a lifecycle node's dispatches.
///
/// One value rather than three parameters, and it is the launch record's own
/// three: every one of them is read off the record the loop read **strictly** at
/// the start of the pass, so a dispatch cannot pick up a config this build could
/// not honour by re-reading `launch.json` leniently where nothing can refuse it.
#[derive(Debug, Clone, Default)]
pub struct Launch {
    /// The default node-scope agent-graph config every dispatch launches, unless
    /// the node or the step names one of its own.
    pub node_graph: String,
    /// The agent graph a change request's body is drafted by, when the launch
    /// named one. `None` is the shipped default: this crate ships the flag, not
    /// the document, and a launch that names no graph drafts nothing.
    pub pr_author_graph: Option<String>,
    /// What every followed `onevcs` session's stream is read through.
    pub vcs_filter: Option<EventFilter>,
}

/// Run one lifecycle node to settlement, re-dispatching it while its
/// publication fails in a way that leaves the work behind.
///
/// A publication that ends `checks-failed`, `checks-unsettled`, `push-rejected`,
/// `pushed-unverified`, or `sync-conflict` did not reject the node — it rejected
/// the **tree** the node produced, and that tree is still on the branch the
/// session handed back (`pushed-unverified` more than the rest: that one is
/// already on the origin, and what a further attempt re-reads is the merge path
/// rather than the push). There
/// is nothing left in the run that would ever look at it again: the node settles
/// `failed`, its dependents never start, and an operator hand-builds a
/// replacement node out of the settlement's detail. So the node is asked again
/// instead, on that same branch, with the diagnosis in its hands.
///
/// Bounded, because a check that will never pass would otherwise be answered by
/// an unbounded series of dispatches, each of them producing the same tree and
/// paying for the same refusal. The node that spends the budget settles `failed`
/// saying how many attempts were made and what each one ended with, which is what
/// tells a reader the difference between a failure and a loop.
#[allow(
    clippy::too_many_arguments,
    reason = "one node's whole execution: the executor, the run, the launch, the node, \
              its cross-repository references, its cancellation, and where to report"
)]
pub fn execute(
    executor: &dyn Executor,
    paths: &RunPaths,
    launch: &Launch,
    node: &Node,
    references: &[crate::plan::CrossRepoReference],
    cancel: &crate::executor::CancellationToken,
    tx: &Sender<Message>,
) -> Settlement {
    let attempts = engine::publication_attempts();
    // What each attempt's publication ended with, in order, for the settlement
    // that stops the loop. One word per attempt: the last attempt's own reason
    // leads the detail as it always has, and a detail carrying three sibling
    // diagnostics in full would carry none of them — every payload text this
    // crate writes is bounded.
    let mut endings: Vec<crate::vcs::Preserving> = Vec::new();
    let mut node = std::borrow::Cow::Borrowed(node);
    let mut attempt = std::num::NonZeroU32::MIN;
    loop {
        let preserved = match attempt_once(executor, paths, launch, &node, references, cancel, tx) {
            Attempt::Settled(settlement) => return settlement,
            Attempt::Preserving(preserved) => preserved,
        };
        endings.push(preserved.outcome);
        // Two reasons to stop, and one settlement for both: the budget is spent,
        // or the run is being stopped. A cancelled run must not be given another
        // dispatch — the teardown is on its way to reap it, and the node would
        // then settle as the cancellation rather than as the publication failure
        // that is the useful half of what happened.
        if attempt >= attempts || cancel.is_cancelled() {
            return stopped_retrying(&node.id, &preserved, &endings);
        }
        attempt = attempt.saturating_add(1);
        // Another `node-dispatched` rather than a kind of its own, so a reader
        // counting dispatches sees the retry without a second word to learn.
        let _ = tx.send(Message::Redispatched(Box::new(engine::Redispatch {
            node: node.id.clone(),
            attempt,
            attempts,
            reason: format!("{}: {}", preserved.outcome.outcome(), preserved.reason),
        })));
        node = std::borrow::Cow::Owned(continued(&node, &preserved, attempt, attempts, &endings));
    }
}

#[allow(
    clippy::too_many_arguments,
    reason = "one attempt's whole context, which is `execute`'s own — see the reason there"
)]
fn attempt_once(
    executor: &dyn Executor,
    paths: &RunPaths,
    launch: &Launch,
    node: &Node,
    references: &[crate::plan::CrossRepoReference],
    cancel: &crate::executor::CancellationToken,
    tx: &Sender<Message>,
) -> Attempt {
    let run = paths.run.as_str();
    let vcs_filter = launch.vcs_filter.as_ref();
    let Some(request) = crate::vcs::request_for(node) else {
        return Attempt::Settled(Settlement {
            detail: Some("a lifecycle node needs a repo".into()),
            ..Settlement::plain(&node.id, NodeStatus::Failed, Some("invalid-node"))
        });
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
            return Attempt::Settled(Settlement {
                detail: Some(reason),
                ..Settlement::plain(&node.id, NodeStatus::Failed, Some("invalid-node"))
            })
        }
    }; // llmlint: ignore-end[changed_behavior_has_e2e]

    let mut session: Option<onevcs::SessionToken> = None;
    // The session's own stream, followed from the moment there is a token to
    // follow, so the publication that comes after the steps is visible while it
    // runs rather than only once it is over.
    let mut stream: Option<crate::vcs::Follower> = None;
    // The one worktree this node's dispatches work in, once its session has
    // opened one. See where it is read: every dispatch after the first runs
    // *there* rather than opening a session beside it.
    let mut worktree: Option<std::path::PathBuf> = None;
    // And what a publication of that session measures its branch against, taken
    // from the same read: asking again at publication would ask the sibling a
    // question it has already answered, on a path where the answer cannot have
    // changed and a failure could not happen — a publication that succeeded is a
    // record that was readable.
    let mut base: Option<String> = None;
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
            return Attempt::Settled(Settlement {
                branch,
                completed_steps: completed,
                ..Settlement::plain(&node.id, NodeStatus::Waiting, None)
            });
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
            &launch.node_graph,
        );
        let build = || DispatchRequest {
            graph: graph.clone(),
            task: step.rendered_task_for(node, references),
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
                let opened = crate::vcs::working_session(token);
                worktree = opened.as_ref().map(|open| open.worktree.clone());
                base = opened.map(|open| open.base);
                stream = crate::vcs::follow(token, vcs_filter, relay_into(tx, whose.clone()));
            }
        }
        if drained.settlement.status != NodeStatus::Done {
            end_session(stream, tx, session.as_ref(), &whose, vcs_filter);
            return Attempt::Settled(Settlement {
                branch,
                completed_steps: completed,
                ..drained.settlement
            });
        }
        if declared_steps {
            completed.push(step.id.clone());
        }
    }

    let Some(token) = session else {
        // Every step declared no diff, so there is nothing to publish and the
        // node settles on the existing no-changes outcome.
        return Attempt::Settled(Settlement {
            branch,
            ..Settlement::plain(&node.id, NodeStatus::Done, Some("no-changes"))
        });
    };

    let attempted = publish(
        executor,
        paths,
        launch,
        node,
        worktree.as_deref(),
        base.as_deref(),
        cancel,
        tx,
        &token,
        branch,
    );
    end_session(stream, tx, Some(&token), &whose, vcs_filter);
    attempted
}

/// Draft the change request's body, then publish through `onevcs`.
#[allow(
    clippy::too_many_arguments,
    reason = "publication needs the dispatch context (executor, the run's paths, what its \
              launch decided, the node, cancellation, and the event stream) as well as what \
              the steps left behind (the session token, its branch, and the worktree and base \
              its record named); the first six are the node's own dispatch identity and \
              bundling them would only move the same list one indirection away"
)]
fn publish(
    executor: &dyn Executor,
    paths: &RunPaths,
    launch: &Launch,
    node: &Node,
    worktree: Option<&std::path::Path>,
    base: Option<&str>,
    cancel: &crate::executor::CancellationToken,
    tx: &Sender<Message>,
    token: &onevcs::SessionToken,
    branch: Option<String>,
) -> Attempt {
    // The plan's own body wins outright and spends no dispatch: a planner who
    // wrote the change request has already done the drafting.
    let (body, undrafted) = match node.body.clone() {
        Some(body) => (Some(body), None),
        // `None` is a launch that named no drafting graph, which drafts nothing
        // and is not a failure: this crate ships the flag, not the document.
        None => match drafted(executor, paths, launch, node, worktree, cancel, tx) {
            None => (None, None),
            Some(Drafted::Body(body)) => (Some(body), None),
            Some(Drafted::Undrafted(ending)) => (None, Some(ending)),
        },
    };
    // Said twice, and only where a drafting dispatch was configured and
    // attempted: in the run's own record at the moment it happened, and on the
    // node's settlement, where a planner reading `results` is shown it without
    // opening the store. Without either, a bodyless change request cannot say
    // whether the drafter ran and failed or was never wired at all — and those
    // need different fixes.
    //
    // On the settlement whatever the publication went on to do, not only where
    // it succeeded: what the drafter did is true either way, and a reader
    // looking for it must not have to know which failure came first. The
    // publication's own reason leads there, because that is what settled the
    // node.
    let undrafted = undrafted.map(|ending| {
        let why = ending.why();
        let _ = tx.send(Message::BodyNotDrafted(Box::new(engine::UndraftedBody {
            node: node.id.clone(),
            ending,
        })));
        why
    });
    // Through the one composition, so every place a publication's own words and
    // a drafting failure are put together agrees about the order and the
    // punctuation — including the failure paths below, which compose the same
    // two values from a different function.
    let with_undrafted = |detail: String| compose(&detail, undrafted.as_deref());
    // The residual: a publication this crate can say nothing more about than
    // that it failed. Every failure `onevcs` names a kind for goes through
    // `failed_publication` below instead, which is where the word and the routing
    // are decided together.
    let publication_failed = |detail: String| {
        Attempt::Settled(Settlement {
            branch: branch.clone(),
            detail: Some(with_undrafted(detail)),
            ..Settlement::plain(&node.id, NodeStatus::Failed, Some("publication-failed"))
        })
    };

    match crate::vcs::publish(
        token,
        node.merge_policy,
        node.title.as_deref(),
        body.as_deref(),
    ) {
        Ok(published) => {
            // A publication that did not land is an ending of the publication,
            // not a refused request: `onevcs` draws that line itself, in
            // `PublishOutcome::Failed`, and this crate reads its line rather
            // than a second one. The reason is the sibling's own — what turned
            // the publication down, and what it said — and it is what the node
            // settles with, or what a re-dispatch carries back to the worker.
            if let onevcs::PublishOutcome::Failed {
                kind,
                reason,
                retained,
            } = &published.outcome
            {
                return failed_publication(
                    &node.id,
                    token,
                    branch.or_else(|| Some(published.branch.clone())),
                    *kind,
                    reason,
                    retained.as_ref(),
                    undrafted.clone(),
                );
            }
            let labels =
                engine::dispatch_labels(&paths.run, &node.id, None, node.persona.as_deref());
            let _ = tx.send(Message::Event(Box::new(crate::vcs::published_event(
                &published, &labels,
            ))));
            // What a `no-changes` compared against. It is the one outcome whose
            // word says nothing a reader can act on — a worker that wrote
            // nothing, a worker whose change was already on the base, and a base
            // that has since taken the work another way all settle identically,
            // and only the ref separates them. The base is the session's own,
            // because a node naming none took the identity's default and this
            // crate never saw it; a session whose record could not be read has
            // none, and the settlement then says what it always did rather than
            // naming a ref nothing established.
            let compared = match published.outcome {
                // Composed out of two of the sibling's own strings and stripped
                // of controls on the way, by the same rule every view applies to
                // a relayed value it renders — see `views::one_line`. A
                // settlement's detail is read back on one line, and this is where
                // this crate makes one out of somebody else's.
                onevcs::PublishOutcome::NothingToPublish => base.map(|base| {
                    crate::views::one_line(&format!(
                        "compared against {base}: {} carries nothing it does not",
                        published.branch
                    ))
                }),
                _ => None,
            };
            Attempt::Settled(Settlement {
                // What the node settles on is its publication, exactly as
                // before; a drafting failure only ever adds words to it.
                detail: compared.map(&with_undrafted).or_else(|| undrafted.clone()),
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
                // The node is done either way — publishing is the whole of what
                // the plan asked of it — so whether the change *landed* is
                // carried beside the status rather than folded into it. Taken
                // from what the publication answered and never from the policy
                // it ran under: an identity that asks the host to merge
                // immediately still has to be observed doing it.
                landing: crate::vcs::landing_of(&published.outcome),
                ..Settlement::plain(&node.id, NodeStatus::Done, None)
            })
        }
        // llmlint: ignore[changed_behavior_has_e2e] this arm is `onevcs` refusing the
        // call outright, which no double can produce: the fake host answers a publish
        // request, and the one refusal this crate can provoke — a title the sibling
        // will not commit under — is caught before any dispatch runs, by
        // `a_title_the_sibling_will_not_commit_under_is_refused_before_any_dispatch`.
        // What the arm does with a drafting failure is not its own composition either:
        // it is the same `publication_failed` the outcome arm above takes, which
        // `a_publication_its_merge_path_refuses_settles_the_node_failed_by_name` drives
        // end to end beside an undrafted body.
        Err(error) => publication_failed(error.to_string()),
    }
}

/// How one attempt at a lifecycle node ended.
///
/// Two cases, and the split is the whole of the routing: a settlement is the
/// node's answer and the loop stops on it, and a publication that failed leaving
/// the work on its branch is an attempt rather than an answer. Cases rather than
/// a settlement a caller inspects afterwards, because everything a continuation
/// needs exists only on the second and would otherwise be `Option`s on every
/// settlement this crate makes.
enum Attempt {
    Settled(Settlement),
    Preserving(Box<Preserved>),
}

/// A publication that failed and handed its branch back.
///
/// What the *next* attempt is dispatched with, and what the settlement says if
/// there is no next attempt — the same four values serve both, because a budget
/// that runs out has to report exactly the failure it stopped re-dispatching.
struct Preserved {
    branch: String,
    outcome: crate::vcs::Preserving,
    /// Already bounded and folded onto one line, because it reaches a reader
    /// through an envelope payload and a settlement detail, both of which are
    /// read back a line at a time.
    reason: String,
    evidence: Vec<crate::vcs::Evidence>,
    /// A drafting ending this attempt also had, carried so that the settlement
    /// a spent budget writes says it exactly as one that settled straight away
    /// does.
    undrafted: Option<String>,
}

/// Settle or continue one failed publication.
///
/// Preserving is **two** conditions and not one. The failure has to be a kind a
/// further attempt could answer — [`crate::vcs::failure_of`] decides that — and
/// the branch has to still exist outside the session, which is `onevcs`'s
/// [`Retention`](onevcs::Retention) answer: a session's clone is disposable, so a
/// branch the execution checkout refused went with it and there is nothing left
/// to continue. Sending a node back to work on a branch nobody kept would cut a
/// fresh one from the base and republish an empty tree, reporting a recovery that
/// recovered nothing.
#[allow(
    clippy::too_many_arguments,
    reason = "the failure's own five values — which kind, what it said, what became of the \
              branch, which branch, and which node — plus the session the evidence is read \
              off and the drafting ending the settlement carries either way. Bundling them \
              would name a struct whose only constructor is this call site"
)]
fn failed_publication(
    node: &str,
    token: &onevcs::SessionToken,
    branch: Option<String>,
    kind: onevcs::FailureKind,
    reason: &str,
    retained: Option<&onevcs::Retention>,
    undrafted: Option<String>,
) -> Attempt {
    let failure = crate::vcs::failure_of(kind);
    let handed_back = matches!(retained, Some(onevcs::Retention::HandedBack(_)));
    let settled = || {
        Attempt::Settled(Settlement {
            branch: branch.clone(),
            detail: Some(compose(&format!("onevcs: {reason}"), undrafted.as_deref())),
            ..Settlement::plain(node, NodeStatus::Failed, Some(failure.outcome()))
        })
    };
    // llmlint: ignore-block[changed_behavior_has_e2e] the second arm covers two cases and
    // only one of them is new. The **terminal** one — a failure no further attempt can
    // answer — is driven end to end by
    // `a_publication_onevcs_refuses_outright_settles_the_residual_and_is_not_retried`,
    // which asserts the residual word and that the node was dispatched exactly once. It
    // reaches the arm through a hosted identity this build has no `RemoteHost` for,
    // because a repository's own verification no longer produces a terminal kind at all:
    // `onevcs` 0.11.0 runs no gate, and the merge path refusing a push is `push-rejected`,
    // which is preserving. The other case is a preserving failure whose branch the
    // execution checkout refused: `onevcs` hands a branch back on every failure it can and
    // reports `Refused` only when that copy itself failed — a checkout that could not be
    // written to — which no double here injects and which the hook script deliberately
    // keeps out of the repository. Reaching it would mean breaking the checkout
    // mid-publication, which proves the fixture rather than this arm, and what it does is
    // exactly what the terminal case does.
    match (failure, handed_back, branch.clone()) {
        (crate::vcs::Failure::Preserving(outcome), true, Some(branch)) => {
            Attempt::Preserving(Box::new(Preserved {
                branch,
                outcome,
                reason: engine::bounded(&crate::views::one_line(reason)),
                evidence: crate::vcs::evidence_in(token),
                undrafted,
            }))
        }
        _ => settled(),
    } // llmlint: ignore-end[changed_behavior_has_e2e]
}

/// A publication's own words and a drafting ending, in that order.
///
/// Written once because three settlements compose the pair, and three spellings
/// of it would come to disagree about the order or the punctuation.
fn compose(detail: &str, undrafted: Option<&str>) -> String {
    match undrafted {
        Some(why) => format!("{detail}. {why}"),
        None => detail.to_owned(),
    }
}

/// The settlement of a node that will not be dispatched again — because its
/// publication budget is spent, or because the run is being cancelled before it
/// could be. Both endings settle as the publication failure, so both are written
/// here.
///
/// The **last** failure's word, because that is the one standing in the way, over
/// a roll-up of every attempt: without it a reader sees one failure and cannot
/// tell it from a node that failed once — which is the difference between "fix
/// this check" and "this check is never going to pass".
fn stopped_retrying(
    node: &str,
    preserved: &Preserved,
    endings: &[crate::vcs::Preserving],
) -> Settlement {
    let each: Vec<String> = endings
        .iter()
        .enumerate()
        .map(|(index, ending)| format!("{} {}", index + 1, ending.outcome()))
        .collect();
    let roll_up = format!(
        "{} publication attempt{} on {}: {}",
        endings.len(),
        if endings.len() == 1 { "" } else { "s" },
        preserved.branch,
        each.join(", ")
    );
    Settlement {
        branch: Some(preserved.branch.clone()),
        detail: Some(compose(
            &format!("onevcs: {}. {roll_up}", preserved.reason),
            preserved.undrafted.as_deref(),
        )),
        // **No step** is recorded as completed, and that is the same rule the
        // re-dispatch was made under seen from the other end. The branch carries
        // a tree the merge path rejected, so a `retry` that skipped the steps it
        // already holds would publish that tree again unaltered and meet the same
        // refusal — the ending this whole loop exists to avoid, reached by hand
        // instead of automatically.
        ..Settlement::plain(node, NodeStatus::Failed, Some(preserved.outcome.outcome()))
    }
}

/// The node the next attempt is dispatched as.
///
/// Three changes and no others. It is **pinned to the preserved branch**, so the
/// session `onevcs` opens continues that branch from its own tip rather than
/// cutting a second one beside committed work. It records **no step as
/// completed**, so every step runs again — against the tree that was rejected,
/// which is the tree that has to change; a continuation that skipped the steps
/// the branch already carries would republish it unaltered and meet the same
/// refusal, which is the one failure this must not have. And it carries the
/// **diagnosis** as its node context, so the worker meets the failure rather than
/// having to go and find it.
///
/// The planner's own note does not survive: a note carries exactly one dispatch
/// and the attempt that just ran was it.
fn continued(
    node: &Node,
    preserved: &Preserved,
    attempt: std::num::NonZeroU32,
    attempts: std::num::NonZeroU32,
    endings: &[crate::vcs::Preserving],
) -> Node {
    Node {
        branch: Some(preserved.branch.clone()),
        resume: None,
        context: Some(diagnosis(preserved, attempt, attempts, endings)),
        ..node.clone()
    }
}

/// What the next attempt is told about the one before it.
///
/// The failure's own reason and a pointer to every artifact its publication
/// recorded — the check's log, the push's output, the conflict's hunks — by id,
/// because the artifact is somebody else's megabytes and what a worker needs is
/// the fetch that gets it. Named as *observed state* by the section it is
/// rendered into, so a worker cannot read a failure report as a new bar to clear.
fn diagnosis(
    preserved: &Preserved,
    attempt: std::num::NonZeroU32,
    attempts: std::num::NonZeroU32,
    endings: &[crate::vcs::Preserving],
) -> String {
    let mut note = format!(
        "The previous attempt's publication failed and its branch was preserved. This is \
         attempt {attempt} of {attempts}, and it continues that same branch — {branch} — so \
         the tree that was rejected is the tree this dispatch starts from. Change what the \
         failure below is about; republishing it unaltered meets the same refusal.\n\n\
         The publication ended `{ending}`, and `onevcs` said:\n\n{reason}\n",
        branch = preserved.branch,
        ending = preserved.outcome.outcome(),
        reason = preserved.reason,
    );
    if endings.len() > 1 {
        let each: Vec<&str> = endings.iter().map(|ending| ending.outcome()).collect();
        note.push_str(&format!(
            "\nEvery attempt so far ended: {}.\n",
            each.join(", ")
        ));
    }
    if !preserved.evidence.is_empty() {
        note.push_str(
            "\nThe publication recorded this evidence, each fetched with \
             `onevcs artifact cat ID`:\n",
        );
        for evidence in &preserved.evidence {
            note.push_str(&format!("- {} — {}\n", evidence.kind.0, evidence.id.0));
        }
    }
    note
}

/// The task a drafting dispatch is given, ahead of the node's own.
const DRAFTING_TASK: &str = "Read this branch's diff and write the change request's body, \
     following the repository's own template. The task this branch delivered:";

/// What one drafting dispatch ended as.
///
/// A body or an ending that is not one, because **every** ending here leaves the
/// publication to proceed with no body: the two are what a change request opens
/// with, not whether it opens.
enum Drafted {
    /// It drafted the change request's body.
    Body(String),
    /// It ended with none, and which of the three endings it was.
    Undrafted(Undrafted),
}

/// A drafting dispatch that produced no body, and how.
///
/// Three endings kept apart rather than one "it did not work", because they need
/// three different fixes: a graph that will not start or will not finish, one
/// whose answers the schema refuses, and one that answers inside the schema with
/// nothing in it. A run that had just wired a drafter could tell none of them
/// from a launch that had wired no drafter at all.
pub(crate) enum Undrafted {
    /// It could not be run, or it ran and did not succeed — in its own words.
    ///
    /// One ending rather than three: a dispatch that never started, one that
    /// failed, and one that was cancelled differ in the reason they carry and in
    /// nothing else a publication carrying no body either way can act on.
    Dispatch(String),
    /// It succeeded, and the schema it was validated against refused every
    /// answer it made.
    SchemaRefused,
    /// It succeeded and there was no body in what it answered with.
    ///
    /// The widest of the three on purpose. It is where a dispatch lands that
    /// succeeded and had nothing refused: one that answered inside its schema
    /// and put nothing in it, one no schema was asked of, and one whose reports
    /// this run holds no readable copy of. They differ in nothing a reader acts
    /// on differently — a drafter that succeeded and produced no prose is the
    /// same fix in each — and none of them is a schema to correct, which is
    /// what keeps them out of [`SchemaRefused`](Self::SchemaRefused).
    Bodyless,
}

impl Undrafted {
    /// The ending, as the event names it.
    pub(crate) fn ending(&self) -> &'static str {
        match self {
            Self::Dispatch(_) => "dispatch-failed",
            Self::SchemaRefused => "schema-refused",
            Self::Bodyless => "no-body",
        }
    }

    /// Why the change request opened with no body, in the words a planner reads
    /// off `results`.
    pub(crate) fn why(&self) -> String {
        match self {
            Self::Dispatch(reason) => {
                format!("the change request's body was not drafted: {reason}")
            }
            Self::SchemaRefused => "the change request's body was not drafted: the drafting \
                 dispatch answered nothing the schema it was validated against accepted"
                .to_owned(),
            Self::Bodyless => "the change request's body was not drafted: the drafting \
                 dispatch succeeded and there was no body in what it answered with"
                .to_owned(),
        }
    }
}

/// One post-verification dispatch drafting the change request's body, when the
/// launch named a graph to draft it with and the node carries none of its own.
///
/// It runs **after** the branch has been verified and is not on the publication
/// path: every way it can end badly leaves the change request to open with no
/// body, and the node settles on its publication as before. That is the whole
/// point of running it here rather than making it a step. What each of those
/// ways *was* is [`Undrafted`], reported beside the publication rather than
/// folded into it.
///
/// It runs in the node's **own** worktree, which is the only place the diff it is
/// asked to read exists: a session of its own would be a fresh clone cut from the
/// base, carrying nothing this node wrote — and opening one reclaims the session
/// still holding the work. So a node with no worktree to run it in drafts
/// nothing, out loud, rather than dispatching an agent to read an empty diff.
///
/// `None` is the one ending that is not a failure and is not reported: a launch
/// that named no drafting graph. This crate ships the flag, not the document, so
/// naming none is the shipped default and there is nothing to say about it. A
/// node carrying its own `body` never reaches here at all.
#[allow(
    clippy::too_many_arguments,
    reason = "the draft is a dispatch inside one lifecycle execution and needs that \
              execution's executor, the run's own paths, what its launch decided, the node, \
              the workspace, cancellation, and the event stream"
)]
fn drafted(
    executor: &dyn Executor,
    paths: &RunPaths,
    launch: &Launch,
    node: &Node,
    worktree: Option<&std::path::Path>,
    cancel: &crate::executor::CancellationToken,
    tx: &Sender<Message>,
) -> Option<Drafted> {
    let graph = launch.pr_author_graph.as_deref()?;
    let Some(worktree) = worktree else {
        // A dispatch that was configured and could not be run at all, which is
        // the same ending as one the executor refused: it is said out loud, as
        // it always was, and now recorded as well.
        let why = "there was no worktree to read this branch's diff in";
        eprintln!(
            "onepipeline: node '{}': no worktree to draft its change request in, \
             so it publishes with no body",
            node.id
        );
        return Some(Drafted::Undrafted(Undrafted::Dispatch(why.to_owned())));
    };
    let dispatch = executor.dispatch(DispatchRequest {
        graph: oneagentgraph::config::ConfigRef(graph.to_owned()),
        task: format!("{DRAFTING_TASK}\n\n{}", node.rendered_task()),
        labels: engine::dispatch_labels(&paths.run, &node.id, None, Some(PR_AUTHOR_PERSONA)),
        // None of the node's own: the drafting dispatch is not the node's work,
        // and a turn budget written for that work would be spent twice — once on
        // it and once here — if this dispatch inherited it.
        controls: NodeControls::default(),
        workspace: WorkspaceSpec::Path(worktree.to_path_buf()),
        cancel: cancel.clone(),
    });
    let mut handle = match dispatch {
        Ok(handle) => handle,
        Err(error) => {
            // Said out loud, because a launch that named a drafting graph and
            // silently drafted nothing is indistinguishable from one that named
            // none — and the change request it opens carries no sign of it.
            eprintln!(
                "onepipeline: node '{}': the drafting dispatch could not start, \
                 so it publishes with no body: {error}",
                node.id
            );
            return Some(Drafted::Undrafted(Undrafted::Dispatch(format!(
                "the drafting dispatch could not start: {error}"
            ))));
        }
    };
    let mut retained = Vec::new();
    for envelope in handle.events() {
        // A line off this stream that will not parse costs that line and nothing
        // more: what the loop is looking for is a `member-settled` naming a
        // retained report, and a drafting run that never produces one already
        // publishes with no body two lines below. Skipping is therefore the same
        // ending an unreadable stream would reach by any other route, reported
        // the same way.
        // llmlint: ignore[changed_behavior_has_e2e] no double can produce this: the
        // envelopes come off the real `oneagentgraph`'s own stdout, which is well-formed
        // by construction, and there is no fault-injection seam here to drive one through.
        // The ending it falls back to — a dispatch that yields no body, and a publication
        // that proceeds without one — is driven end to end by
        // `a_drafting_graph_the_runner_refuses_still_publishes_the_change_request`.
        let Ok(envelope) = envelope else { continue };
        // **Ingest**, and the same ingest the engine performs on the envelopes
        // this relays to it: the line is arriving on the stdout of a process
        // this crate started, which is the one moment the path it names carries
        // the producer's authority rather than the journal's. What is read
        // below is that copy, at a path derived from the settlement — this
        // crate never opens the path a producer named.
        crate::report::retain(paths, &envelope);
        if envelope.source == crate::event::Source::Agentgraph
            && envelope.kind.0 == crate::report::MEMBER_SETTLED
        {
            retained.push(paths.report_for(&envelope.stream, envelope.seq));
        }
        let _ = tx.send(Message::Event(Box::new(envelope)));
    }
    // llmlint: ignore-block[changed_behavior_has_e2e] the last arm below is reached by a
    // dispatch that failed and by one that was cancelled; the first has a journey of its
    // own in `tests/e2e/lifecycle.rs` and the second has none because it is not separately
    // reachable. Nothing cancels a drafting dispatch except the node's own token being
    // flipped, which happens when the run is being stopped — and a run whose driver is
    // being torn down has no publication left to protect, so a journey claiming "it
    // published anyway" would be asserting the opposite of what a stop means. Deleting the
    // arm is not the alternative either: it is the same `_` a failed settlement takes.
    match handle.wait() {
        Ok(outcome) if outcome.succeeded => {
            // Every report the dispatch retained, read as **one** answer: a
            // fallback chain records a candidate per identity it tried, and
            // which report an entry landed in is not the reader's business.
            let kept: Vec<serde_json::Value> = retained
                .iter()
                .filter_map(|kept| crate::report::read(kept))
                .collect();
            Some(match crate::report::drafted(&kept) {
                crate::report::Drafted::Body(body) => Drafted::Body(body),
                crate::report::Drafted::SchemaRefused => {
                    Drafted::Undrafted(Undrafted::SchemaRefused)
                }
                crate::report::Drafted::Bodyless => Drafted::Undrafted(Undrafted::Bodyless),
            })
        }
        Ok(outcome) => Some(Drafted::Undrafted(Undrafted::Dispatch(format!(
            "the drafting dispatch settled without succeeding: {}",
            first_line(&outcome.detail)
        )))),
        Err(error) => Some(Drafted::Undrafted(Undrafted::Dispatch(format!(
            "the drafting dispatch could not be waited on: {error}"
        )))),
    } // llmlint: ignore-end[changed_behavior_has_e2e]
}

/// A dispatch's own words, as one bounded line of a settlement detail.
///
/// The reason a dispatch gives is its stderr, which is many lines of a sibling's
/// diagnostics; what belongs beside a publication is the first of them, held to
/// the same bound every other payload text this crate writes is held to.
fn first_line(detail: &str) -> String {
    match detail.lines().find(|line| !line.trim().is_empty()) {
        Some(line) => engine::bounded(line.trim()),
        None => "it reported nothing".to_owned(),
    }
}
// llmlint: ignore-end[invalid_states_unrepresentable]

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
/// push, change request, merge — lands in the merged store belonging to no node,
/// so every per-node view reads it as work that happened to nobody.
///
/// An enricher, so it never rewrites: a key the producer stamped stands.
pub(crate) fn stamp(labels: &mut Labels, known: &Labels) {
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
    token: Option<&onevcs::SessionToken>,
    node: &Labels,
    filter: Option<&EventFilter>,
) {
    close(token);
    // Empty from either side is the whole stream still to read: no follow was
    // started, or one was and relayed nothing.
    let followed_through = stream.map(crate::vcs::Follower::finish).unwrap_or_default();
    relay_session_events(tx, token, node, &followed_through, filter);
}

/// Fold the part of the session's stream nothing has relayed into the merged one.
///
/// `onevcs` records the commits and the publication against the session, the
/// merge path's own verdict on the `push` among them; without this the merged
/// store would carry a lifecycle node's settlement with none of the evidence
/// behind it. `followed_through` is the highest `seq` the follow already relayed
/// **per stream**, so a record arrives **once**: each stream is numbered
/// monotonically and resumes its series across the processes that write to it,
/// which makes those marks the whole of the bookkeeping. Per stream and not one
/// mark over all of them, because reading one session hands back the identity's
/// release records as well as the session's own, in a series of their own.
fn relay_session_events(
    tx: &Sender<Message>,
    token: Option<&onevcs::SessionToken>,
    node: &Labels,
    followed_through: &crate::vcs::Watermarks,
    filter: Option<&EventFilter>,
) {
    let Some(token) = token else { return };
    let relay = relay_into(tx, node.clone());
    // The same filter the follow was opened with: the read-once fallback covers
    // the tail of the *same* stream, so a run that filtered what it followed and
    // not what it caught up on would relay events it said it did not want, for
    // no reason but which side of a settlement they landed on.
    for envelope in beyond(crate::vcs::events(token, filter), followed_through) {
        relay(envelope);
    }
}

/// The part of what a read handed back that a follow did not already relay.
///
/// Empty marks are the whole of it — no follow was started, or one was and
/// relayed nothing — which is a stream still to read rather than a stream that
/// held nothing. Otherwise everything numbered past the mark **its own stream**
/// stands at, and nothing at or below it: a record relayed twice is the same
/// defect as one lost, seen from the other side.
fn beyond(envelopes: Vec<Envelope>, followed_through: &crate::vcs::Watermarks) -> Vec<Envelope> {
    envelopes
        .into_iter()
        .filter(|envelope| followed_through.beyond(envelope))
        .collect()
}

fn close(token: Option<&onevcs::SessionToken>) {
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

    /// The endings this module emits and the endings the contract names are one
    /// set.
    ///
    /// The wire spellings are stated twice — in `docs/contract.md`'s pr-author
    /// paragraph and in [`Undrafted::ending`] — and only one of them is
    /// compiled, so the document needs a gate the way the closed set of kinds
    /// has one in `tests/contract.rs`. It cannot live there: the type is private
    /// to this module, and a public one would widen the surface past what the
    /// contract names.
    #[test]
    fn every_ending_this_module_emits_is_one_the_contract_names() {
        let contract = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("docs/contract.md"),
        )
        .expect("the contract ships");
        let endings = [
            Undrafted::Dispatch(String::new()),
            Undrafted::SchemaRefused,
            Undrafted::Bodyless,
        ];
        for ending in &endings {
            assert!(
                contract.contains(&format!("`{}`", ending.ending())),
                "docs/contract.md does not name the `{}` ending this module emits",
                ending.ending()
            );
        }

        // And the other direction: a spelling the document carries and nothing
        // emits is a promise nobody keeps. The contract lists them in one
        // clause, so the clause is read and its backticked tokens compared with
        // the set above rather than the whole document searched.
        let clause = contract
            .split_once("carrying `ending` —")
            .expect("the contract lists the endings `body-not-drafted` carries")
            .1
            .split_once("— and `detail`")
            .expect("the clause ends where the detail begins")
            .0;
        let listed: Vec<&str> = clause.split('`').skip(1).step_by(2).collect();
        assert_eq!(
            listed,
            endings
                .iter()
                .map(Undrafted::ending)
                .collect::<Vec<&'static str>>(),
            "the contract's endings are not the ones this module emits"
        );

        // The sentences a reader is given are each the ending's own, so two
        // endings cannot arrive under one set of words.
        let why: std::collections::BTreeSet<String> = endings.iter().map(Undrafted::why).collect();
        assert_eq!(why.len(), endings.len(), "two endings say the same thing");
    }

    /// The README summarises the same set, so it is gated the same way.
    ///
    /// It is a third copy of the endings — the enum, the contract, and the
    /// user-facing prose — and the first two already hold each other. Left
    /// ungated the README is the one that goes quietly stale: nothing compiles
    /// it, and a reader meeting an ending it does not list has no way to know
    /// which of the two is behind.
    #[test]
    fn the_readmes_ending_summary_is_the_set_this_module_emits() {
        let raw = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("README.md"),
        )
        .expect("the README ships");
        // Wrapped prose, so match on its words rather than its line breaks.
        let readme = raw.split_whitespace().collect::<Vec<_>>().join(" ");
        let clause = readme
            .split_once("under one of three endings —")
            .expect("the README summarises the endings a drafting dispatch can reach")
            .1
            .split_once("— and the node's own settlement")
            .expect("the clause ends where the settlement's own half begins")
            .0;
        let listed: Vec<&str> = clause.split('`').skip(1).step_by(2).collect();
        assert_eq!(
            listed,
            [
                Undrafted::Dispatch(String::new()),
                Undrafted::SchemaRefused,
                Undrafted::Bodyless,
            ]
            .iter()
            .map(Undrafted::ending)
            .collect::<Vec<&'static str>>(),
            "the README's endings are not the ones this module emits"
        );
    }

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
            &RunPaths::under(std::path::Path::new("/nowhere"), "demo"),
            &Launch {
                node_graph: "graphs/node-scope.yaml".into(),
                ..Launch::default()
            },
            &node,
            &[],
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

    /// The record a follow ended one read short of, relayed exactly once — and
    /// counted against **its own** stream.
    ///
    /// The window this covers is inside a library call now — closing a session
    /// flips its record and only then writes `session-closed`, and the follow
    /// reads and only then asks whether the session closed — so it cannot be
    /// forced from an e2e the way a delayed subprocess once could. This is the
    /// arithmetic that makes "once" true either way, held on its own.
    ///
    /// Two streams rather than one, because reading a session hands back two:
    /// its own records, and the identity's release records that name its
    /// landing, numbered in a series of their own over every session in that
    /// repository. Under one mark over both, a release numbered higher than the
    /// session had reached would hide the session's next record — a relayed
    /// record lost, silently.
    #[test]
    fn relays_only_what_the_follow_did_not_counting_each_stream_on_its_own() {
        let wrote = |stream: &str, seq: u64| Envelope {
            v: crate::event::ENVELOPE_VERSION,
            ts: "2026-01-01T00:00:00.000Z".into(),
            stream: stream.to_owned(),
            seq,
            source: crate::event::Source::Vcs,
            kind: crate::event::EventKind("session-closed".into()),
            phase: None,
            labels: Labels::default(),
            payload: serde_json::Map::new(),
            artifacts: Vec::new(),
        };
        let session: Vec<Envelope> = (1..=4).map(|seq| wrote("s-1", seq)).collect();

        // A follow that reached the third record leaves the tail and nothing
        // else: re-reading the whole stream would put the first three in twice.
        let mut reached = crate::vcs::Watermarks::default();
        for envelope in &session[..3] {
            reached.reached(envelope);
        }
        let tail = beyond(session.clone(), &reached);
        assert_eq!(tail.iter().map(|e| e.seq).collect::<Vec<_>>(), vec![4]);

        // A follow that ended having relayed everything leaves nothing.
        reached.reached(&session[3]);
        assert!(beyond(session.clone(), &reached).is_empty());

        // A release the same read handed back is another stream's record, so a
        // mark four records into the session's says nothing about it.
        let released = wrote("releases-0a1b2c3d4e5f", 2);
        assert_eq!(
            beyond(vec![released.clone()], &reached)
                .iter()
                .map(|e| e.stream.clone())
                .collect::<Vec<_>>(),
            vec!["releases-0a1b2c3d4e5f".to_owned()],
            "a release was hidden by how far the session's own stream had got"
        );
        // And once it has been relayed, the session's tail is still relayable
        // from the mark it stands at rather than from that release's.
        reached.reached(&released);
        assert!(beyond(vec![released], &reached).is_empty());
        assert_eq!(
            beyond(vec![wrote("s-1", 5)], &reached)
                .iter()
                .map(|e| e.seq)
                .collect::<Vec<_>>(),
            vec![5]
        );

        // And a follow that relayed nothing at all leaves the whole stream,
        // which is a stream still to read rather than a stream that held
        // nothing.
        assert_eq!(
            beyond(session, &crate::vcs::Watermarks::default())
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
