//! A node's guard: a program the engine runs when the node first becomes actionable,
//! before anything else happens to it, whose exit status says whether the node is needed
//! at all.
//!
//! Any node but a step may carry one — an agent node, direct or lifecycle, a `kind:
//! command` node and a `kind: human` node. When such a node is ready, or for a human node
//! about to be recorded `waiting`, the engine starts its guard *instead of* whatever it
//! would have started, through the one runner a command node's command runs through
//! ([`crate::commandstep::run_program`]): no shell, from the launch directory, with the
//! driver's environment and the run's coordinates on top, its stdout and stderr appended to
//! `<run>/guards/<node>.log`, its process tree stopped at its deadline or by a `cancel`,
//! `drop` or `retry` that reaches it. It runs on a thread of its own, as a command does,
//! and counts against the plan's `concurrency`; the node reads `running` meanwhile.
//!
//! - Exit [`NOT_NEEDED_EXIT`] settles the node `done` with outcome [`NOT_NEEDED`] — no
//!   dispatch, session, worktree, branch, agent or human item, and no landing — its detail
//!   the end of the guard's stderr, where it said anything.
//! - Exit 0 is *needed*: the node proceeds exactly as an unguarded node would from there,
//!   and its guard is not run again for that attempt.
//! - Anything else — another exit, a signal, a guard that cannot be started, one past its
//!   deadline — settles the node `failed` with outcome [`GUARD_FAILED`]. A broken guard is
//!   never read as not needed.
//!
//! `guard-started` is the record that makes the node `running`, and `guard-evaluated` is
//! journalled ahead of any settlement or dispatch its verdict leads to — which is what lets
//! `adopt` reuse a verdict and re-run a guard whose driver died before it was recorded.

use std::num::NonZeroU64;
use std::path::PathBuf;
use std::sync::mpsc::Sender;
use std::time::Instant;

use crate::commandstep::{run_program, with_tail, Ended};
use crate::engine::{Message, Settled, Settlement};
use crate::error::{Error, Result};
use crate::executor::CancellationToken;
use crate::filter::Argv;
use crate::graph::NodeStatus;
use crate::ledger::{LaunchRecord, RunPaths};
use crate::payload::{CommandEnding, GuardEvaluated, GuardFailure, GuardStarted, GuardVerdict};
use crate::plan::Node;

/// The outcome a node its guard found not needed settles `done` under.
pub(crate) const NOT_NEEDED: &str = "not-needed";

/// The outcome a node whose guard failed settles `failed` under.
pub(crate) const GUARD_FAILED: &str = "guard-failed";

/// The exit status a guard answers *not needed* with.
pub(crate) const NOT_NEEDED_EXIT: i32 = 3;

/// How long a guard may run when its node states no `guard_timeout_seconds`.
pub(crate) const DEFAULT_TIMEOUT_SECONDS: NonZeroU64 = match NonZeroU64::new(300) {
    Some(seconds) => seconds,
    None => unreachable!(),
};

/// Where a guarded node's guard output is kept.
pub(crate) fn log_of(paths: &RunPaths, node: &str) -> PathBuf {
    crate::hooks::run_root(paths)
        .join("guards")
        .join(format!("{node}.log"))
}

/// The deadline `node`'s guard is held to.
pub(crate) fn timeout_of(node: &Node) -> NonZeroU64 {
    node.guard_timeout_seconds
        .unwrap_or(DEFAULT_TIMEOUT_SECONDS)
}

/// What `guard-started` records for this node, running `argv`.
pub(crate) fn started(paths: &RunPaths, node: &Node, argv: &Argv) -> GuardStarted {
    GuardStarted {
        guard: argv.clone(),
        log: log_of(paths, &node.id).display().to_string(),
        timeout_seconds: timeout_of(node),
    }
}

/// What follows a guard's `guard-evaluated` for its node.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Then {
    /// The node proceeds, as an unguarded node would: it comes out of flight and is
    /// started on the loop's next pass.
    Proceeds,
    /// The node's settlement follows, as its own message.
    SettlementFollows,
}

/// What a guard's ending comes to: what `guard-evaluated` records, and the settlement the
/// node takes — none where the node is needed and proceeds.
pub(crate) struct Evaluated {
    /// The record.
    pub(crate) record: GuardEvaluated,
    /// The node's settlement, where the verdict settles it.
    pub(crate) settlement: Option<Settlement>,
}

/// Run one guarded node's guard on a thread of its own, and hand the loop its verdict —
/// and, where the verdict settles the node, the settlement after it.
///
/// # Errors
///
/// A thread this host would not start; the node is then left for the loop to settle.
pub(crate) fn spawn(
    paths: &RunPaths,
    launch: &LaunchRecord,
    node: &Node,
    argv: &Argv,
    cancel: CancellationToken,
    tx: Sender<Message>,
) -> Result<()> {
    let paths = paths.clone();
    let launch = launch.clone();
    let node = node.clone();
    let argv = argv.clone();
    let named = node.id.clone();
    std::thread::Builder::new()
        .name(format!("guard-{}", node.id))
        .spawn(move || {
            let begun = Instant::now();
            let ran = run_program(
                &paths,
                &launch,
                &node.id,
                &argv,
                &log_of(&paths, &node.id),
                Some(timeout_of(&node)),
                &cancel,
            );
            let duration_ms = u64::try_from(begun.elapsed().as_millis()).unwrap_or(u64::MAX);
            let Evaluated { record, settlement } =
                evaluate(&node, ran.ended, &ran.tail, duration_ms, &cancel);
            // A node the loop started has an identity; one without is a node the loader
            // refused, and nothing is sent for it but its settlement.
            if let Some(named) = crate::graph::NodeRef::of(&node) {
                let then = if settlement.is_some() {
                    Then::SettlementFollows
                } else {
                    Then::Proceeds
                };
                let _ = tx.send(Message::GuardEvaluated(Box::new((named, record, then))));
            }
            if let Some(settlement) = settlement {
                let (recorded, _) = std::sync::mpsc::channel();
                let _ = tx.send(Message::Settled(Box::new(Settled {
                    settlement,
                    recorded,
                })));
            }
        })
        .map(|_| ())
        .map_err(|error| {
            Error::Invalid(format!(
                "node '{named}': no thread could be started to run its guard: {error}"
            ))
        })
}

/// What a guard that ended as `ended`, having said `tail` on stderr, decides about `node`.
fn evaluate(
    node: &Node,
    ended: Ended,
    tail: &[u8],
    duration_ms: u64,
    cancel: &CancellationToken,
) -> Evaluated {
    let evaluated = |verdict: GuardVerdict, settlement: Option<Settlement>| Evaluated {
        record: GuardEvaluated {
            verdict,
            duration_ms,
        },
        settlement,
    };
    let cancelled = |verdict: GuardVerdict| {
        evaluated(
            verdict,
            Some(Settlement {
                detail: Some(
                    "the node was cancelled while its guard ran, so its process tree was \
                     stopped"
                        .to_owned(),
                ),
                ..Settlement::plain(&node.id, NodeStatus::Cancelled, None)
            }),
        )
    };
    // How it ended as the record writes it, and — where the host said why it never ran or
    // could not be watched — that reason, which opens the failure's detail.
    let (ending, why) = match ended {
        Ended::Exited { exit_code } => (CommandEnding::Exited { exit_code }, None),
        Ended::TimedOut => (CommandEnding::TimedOut, None),
        Ended::Cancelled => (CommandEnding::Cancelled, None),
        Ended::NotStarted(why) => (
            CommandEnding::NotStarted,
            Some(format!("the guard could not be started: {why}")),
        ),
        // llmlint: ignore[changed_behavior_has_e2e] reached only where the host refuses to say whether a child it started has exited (`try_wait` erring), which no plan or verb produces; a command reaches the same runner arm the same way and no journey stages it either. What it must do — fail rather than read as not needed — is the shared failure below, which the failure journey in `tests/e2e/guard.rs` drives.
        Ended::Unwaitable(why) => (
            CommandEnding::Exited { exit_code: None },
            Some(with_tail(
                format!("the guard could not be waited for: {why}"),
                tail,
            )),
        ),
    };
    // Exit 0 and exit 3 are the guard's two answers, and no failure.
    let Some(failure) = GuardFailure::new(ending) else {
        if ending
            == (CommandEnding::Exited {
                exit_code: Some(NOT_NEEDED_EXIT),
            })
        {
            let said = String::from_utf8_lossy(tail);
            let said = said.trim();
            return evaluated(
                GuardVerdict::NotNeeded,
                Some(Settlement {
                    detail: (!said.is_empty()).then(|| said.to_owned()),
                    ..Settlement::plain(&node.id, NodeStatus::Done, Some(NOT_NEEDED))
                }),
            );
        }
        // A cancellation that reached the node as its guard said *needed* is still a
        // cancellation: nothing is started for a node a manager has stopped. A race no
        // journey can stage on purpose — the cancel and the exit within one poll — so it is
        // held by this module's `each_ending_is_the_verdict_and_settlement_the_contract_names`.
        if cancel.is_cancelled() {
            return cancelled(GuardVerdict::Needed);
        }
        return evaluated(GuardVerdict::Needed, None);
    };
    if ending == CommandEnding::Cancelled {
        return cancelled(GuardVerdict::Failed(failure));
    }
    let detail = why.unwrap_or_else(|| with_tail(failure_of(node, ending), tail));
    evaluated(
        GuardVerdict::Failed(failure),
        Some(Settlement {
            detail: Some(detail),
            ..Settlement::plain(&node.id, NodeStatus::Failed, Some(GUARD_FAILED))
        }),
    )
}

/// What a failed guard's detail opens with, before its stderr: its exit status, the signal
/// that ended it, or the deadline it outlived.
fn failure_of(node: &Node, ending: CommandEnding) -> String {
    match ending {
        CommandEnding::Exited {
            exit_code: Some(code),
        } => format!("the guard exited with status {code}"),
        CommandEnding::Exited { exit_code: None } => "the guard was ended by a signal".to_owned(),
        CommandEnding::TimedOut => format!(
            "the guard timed out after {} s, so its process tree was stopped",
            timeout_of(node)
        ),
        CommandEnding::NotStarted => "the guard could not be started".to_owned(),
        CommandEnding::Cancelled => "the guard was stopped by a cancellation".to_owned(),
    }
}

/// The settlement a verdict journalled for `node`'s current attempt leads to, for a node
/// whose driver stopped between journalling that verdict and settling it: the adopting
/// driver settles it from the record rather than running its guard again. `None` for a
/// verdict that settles nothing an adopter owes — *needed*, which proceeds, and a
/// cancellation, whose park the adopter keeps.
///
/// What the guard said on stderr went with the driver that read it, so the detail names
/// the log that kept it.
pub(crate) fn recovered(
    paths: &RunPaths,
    node: &Node,
    verdict: GuardVerdict,
) -> Option<Settlement> {
    let kept = format!(
        "its verdict was journalled before its driver stopped, and what it said is kept in {}",
        log_of(paths, &node.id).display()
    );
    match verdict {
        GuardVerdict::Needed => None,
        GuardVerdict::Failed(failure) if failure.ending() == CommandEnding::Cancelled => None,
        GuardVerdict::NotNeeded => Some(Settlement {
            detail: Some(format!("the guard found the node not needed; {kept}")),
            ..Settlement::plain(&node.id, NodeStatus::Done, Some(NOT_NEEDED))
        }),
        GuardVerdict::Failed(failure) => Some(Settlement {
            detail: Some(format!("{}; {kept}", failure_of(node, failure.ending()))),
            ..Settlement::plain(&node.id, NodeStatus::Failed, Some(GUARD_FAILED))
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node() -> Node {
        Node {
            id: "guarded".to_owned(),
            guard: Some(vec!["true".to_owned()]),
            ..Node::default()
        }
    }

    /// The four readings of an exit status, and the two endings that are no exit at all,
    /// each to the verdict and settlement the contract names.
    #[test]
    fn each_ending_is_the_verdict_and_settlement_the_contract_names() {
        let live = CancellationToken::new();
        let exited = |code: i32| Ended::Exited {
            exit_code: Some(code),
        };
        let needed = evaluate(&node(), exited(0), b"ignored", 5, &live);
        assert!(matches!(needed.record.verdict, GuardVerdict::Needed));
        assert!(needed.settlement.is_none());

        let not_needed = evaluate(&node(), exited(3), b"\n  nothing to do  \n", 5, &live);
        assert!(matches!(not_needed.record.verdict, GuardVerdict::NotNeeded));
        let settled = not_needed.settlement.expect("it settles");
        assert_eq!(settled.status, NodeStatus::Done);
        assert_eq!(settled.outcome.as_deref(), Some(NOT_NEEDED));
        assert_eq!(settled.detail.as_deref(), Some("nothing to do"));
        let silent = evaluate(&node(), exited(3), b"  \n", 5, &live);
        assert_eq!(silent.settlement.expect("it settles").detail, None);

        for (ended, says) in [
            (exited(1), "exited with status 1"),
            (Ended::Exited { exit_code: None }, "ended by a signal"),
            (
                Ended::NotStarted("no such file".to_owned()),
                "the guard could not be started: no such file",
            ),
            (Ended::TimedOut, "the guard timed out after 300 s"),
        ] {
            let failed = evaluate(&node(), ended, b"why", 5, &live);
            assert!(matches!(failed.record.verdict, GuardVerdict::Failed(_)));
            let settled = failed.settlement.expect("it settles");
            assert_eq!(settled.status, NodeStatus::Failed);
            assert_eq!(settled.outcome.as_deref(), Some(GUARD_FAILED));
            let detail = settled.detail.expect("a detail");
            assert!(detail.contains(says), "{detail}");
        }

        // The typed verdict is written as the flat record the contract names, and read back.
        for (evaluated, wire) in [
            (
                not_needed.record,
                serde_json::json!({"verdict": "not-needed", "ending": "exited", "exit_code": 3,
                                   "duration_ms": 5}),
            ),
            (
                evaluate(&node(), Ended::TimedOut, b"", 5, &live).record,
                serde_json::json!({"verdict": "failed", "ending": "timed-out", "duration_ms": 5}),
            ),
        ] {
            assert_eq!(serde_json::to_value(evaluated).expect("it writes"), wire);
            assert_eq!(
                serde_json::from_value::<GuardEvaluated>(wire).expect("it reads"),
                evaluated
            );
        }
        // A record whose verdict its ending contradicts is refused where it is read.
        for contradicts in [
            serde_json::json!({"verdict": "needed", "ending": "timed-out", "duration_ms": 5}),
            serde_json::json!({"verdict": "needed", "ending": "exited", "exit_code": 3,
                               "duration_ms": 5}),
            serde_json::json!({"verdict": "not-needed", "ending": "exited", "exit_code": 1,
                               "duration_ms": 5}),
            serde_json::json!({"verdict": "failed", "ending": "exited", "exit_code": 0,
                               "duration_ms": 5}),
        ] {
            assert!(
                serde_json::from_value::<GuardEvaluated>(contradicts.clone()).is_err(),
                "{contradicts} read"
            );
        }

        let stopped = CancellationToken::new();
        stopped.cancel();
        let raced = evaluate(&node(), exited(0), b"", 5, &stopped);
        assert!(matches!(raced.record.verdict, GuardVerdict::Needed));
        assert_eq!(
            raced.settlement.expect("a cancelled node settles").status,
            NodeStatus::Cancelled
        );
    }
}
