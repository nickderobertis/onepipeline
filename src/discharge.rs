//! What becomes of a decision surface whose node a committed edit took out of
//! the graph.
//!
//! A blocking surface names the node it is about in its `workstream`: a
//! question `onepipeline ask --about NODE` raised, which the bus stamps with a
//! correlation, and a reconciler finding raised under a stable one. When a
//! `retry` supersedes that node, or a `drop` removes it, nothing is left for the
//! question to decide — and before this, nothing answered it either, so it sat
//! pending for the life of the run: `status` kept asking for it and
//! `watch --until surface` returned on it every time it was asked.
//!
//! So the commit that removed the node answers it, through the same reply record
//! a verdict is appended to, carrying the surface's correlation and a reason
//! naming that edit — and a blocking surface raised under no correlation, such
//! as a `finding` a watcher raised about the node, is marked by its own id as
//! one nobody is waiting on. What the rule is and the ruling it rests on are entry 119 of
//! `docs/contract-divergences.md`; they are not restated here.

use std::collections::BTreeMap;

use onemessagebus::Correlation;

use crate::channel::{Author, ChannelState, Command, Reply};
use crate::edits;
use crate::ledger::RunPaths;
use crate::Result;

/// Answer every decision surface about a node `operations` took out of the
/// graph.
///
/// Called beside [`findings::answer_requested`](crate::findings::answer_requested)
/// by both writers of the graph, for the reason that one gives: which writer
/// committed an edit is an accident of timing.
///
/// Only a surface that is standing — waiting to be read, or read and held
/// pending — blocking, and still being waited on is discharged. One raised
/// under a correlation no reply has answered yet is answered under it; one
/// raised under none is marked abandoned by its id. A surface about a node still
/// in the graph is never one `operations` removed, so it is left exactly where
/// it was.
///
/// # Errors
///
/// The reason the run's channel could not be written.
pub(crate) fn answer_removed(paths: &RunPaths, operations: &[edits::Operation]) -> Result<()> {
    let removed = removed_by(operations);
    if removed.is_empty() {
        return Ok(());
    }
    let channel = ChannelState::new(paths);
    let queue = channel.queue();
    let mut answered = channel.answered();
    let mut unkeyed = Vec::new();
    for surface in queue.waiting.iter().chain(queue.pending.iter()) {
        if !surface.blocking || surface.abandoned {
            continue;
        }
        let Some(node) = &surface.workstream else {
            continue;
        };
        let Some(edit) = removed.get(node.as_str()) else {
            continue;
        };
        match &surface.correlation {
            // Inserted as it is answered, so a surface the queue reports twice —
            // the same question waiting and held — is answered once.
            Some(correlation) => {
                if answered.insert(correlation.clone()) {
                    channel.answer_bound(&answer(node, edit), Some(correlation))?;
                }
            }
            // A surface raised under no correlation has nothing a reply could be
            // bound to — a reply carrying none is a verdict for whichever listener
            // reads next — so it is marked by its own id as one nobody is waiting
            // on, which is the mark every reader of what is standing already
            // consults. Nobody is: the node that would act on its answer is gone.
            None => unkeyed.push(surface.id),
        }
    }
    if !unkeyed.is_empty() {
        channel.abandon(&unkeyed)?;
    }
    Ok(())
}

/// Whether the question `correlation` names was answered by the removal of its
/// node, and `commands` carry the edit that removed it.
///
/// The discharge's half of the exception
/// [`findings::answered_alongside`](crate::findings::answered_alongside) states:
/// an envelope's commands are committed before its verdict half is delivered, so
/// a `reply --correlation C` carrying the `retry` or `drop` of C's node finds C
/// answered by its own edit by the time it is bound, and refusing it would report
/// a failure for a reply whose edit has landed. Asked of the envelope, so a
/// verdict carrying no such command still meets the refusal a question an
/// earlier reply answered always met.
pub(crate) fn answered_alongside(
    paths: &RunPaths,
    correlation: &Correlation,
    commands: &[Command],
) -> bool {
    let channel = ChannelState::new(paths);
    let Some(node) = channel
        .raised_under(correlation)
        .and_then(|surface| surface.workstream)
    else {
        return false;
    };
    commands.iter().any(|command| {
        matches!(command, Command::Retry { .. } | Command::Drop { .. })
            && crate::channel::target_of(command).as_deref() == Some(node.as_str())
    }) && channel.answered().contains(correlation)
}

/// The edit that took a node out of the graph, as the answer names it.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Removal {
    /// A `retry` superseded it with the named replacement.
    Superseded { replacement: String },
    /// A `drop` removed it, or removed a node it depended on and took it along.
    Dropped,
}

/// Every node `operations` took out of the graph, and by which edit.
///
/// A `retry` records its superseded node as dropped in the same edit, so a
/// dropped node the same list also retried was superseded rather than dropped.
fn removed_by(operations: &[edits::Operation]) -> BTreeMap<&str, Removal> {
    let superseded: BTreeMap<&str, &str> = operations
        .iter()
        .filter_map(|operation| match operation {
            edits::Operation::RetryRequested {
                node, replacement, ..
            } => Some((node.as_str(), replacement.as_str())),
            _ => None,
        })
        .collect();
    operations
        .iter()
        .filter_map(|operation| match operation {
            edits::Operation::NodeDropped { node, .. } => Some(node.as_str()),
            _ => None,
        })
        .map(|node| {
            let removal = superseded
                .get(node)
                .map_or(Removal::Dropped, |replacement| Removal::Superseded {
                    replacement: (*replacement).to_owned(),
                });
            (node, removal)
        })
        .collect()
}

/// The answer a committed edit appends for a surface about the node it removed.
///
/// Authored by the **planner**, as [`findings`](crate::findings)' answer is: the
/// channel's own writer, asking for nothing, recording what the graph now says.
fn answer(node: &str, removal: &Removal) -> Reply {
    let edit = match removal {
        Removal::Superseded { replacement } => {
            format!("a committed `retry` of '{node}' superseded it with '{replacement}'")
        }
        Removal::Dropped => format!("a committed `drop` removed '{node}' from the graph"),
    };
    Reply {
        author: Author::default(),
        message: Some(format!(
            "discharged: the node this decision is about, '{node}', has left the graph."
        )),
        reason: Some(edit),
        ..Reply::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::path::PathBuf;

    use crate::channel::{source, Surface, SurfaceKind};

    /// A run directory of its own, removed when the test ends.
    struct Scratch {
        root: PathBuf,
        paths: RunPaths,
    }

    impl Scratch {
        fn new(name: &str) -> Self {
            let root = std::env::temp_dir().join(format!(
                "onepipeline-discharge-{name}-{}",
                crate::sys::pid()
            ));
            let _ = std::fs::remove_dir_all(&root);
            let paths = RunPaths::under(&root, "demo");
            paths.create().expect("the run directory");
            Self { root, paths }
        }

        /// Queue a surface about `node` under `correlation`.
        fn raised(&self, node: &str, correlation: Option<&str>, blocking: bool) {
            ChannelState::new(&self.paths)
                .push(Surface {
                    id: 0,
                    kind: SurfaceKind::FINDING.into(),
                    message: format!("what should become of '{node}'?"),
                    source: source::PROPOSAL.into(),
                    blocking,
                    queued_at: crate::sys::now_millis(),
                    workstream: Some(node.to_owned()),
                    abandoned: false,
                    asker: None,
                    correlation: correlation.map(|key| key.parse().expect("a correlation")),
                })
                .expect("the surface queues");
        }

        fn answers(&self) -> Vec<crate::channel::QueuedReply> {
            ChannelState::new(&self.paths).replies()
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    fn retried(node: &str) -> Vec<edits::Operation> {
        vec![
            edits::Operation::RetryRequested {
                node: node.to_owned(),
                replacement: format!("{node}-again"),
                reset: Vec::new(),
            },
            edits::Operation::NodeDropped {
                node: node.to_owned(),
                dependents: crate::channel::Dependents::Detach,
            },
        ]
    }

    fn dropped(node: &str) -> Vec<edits::Operation> {
        vec![edits::Operation::NodeDropped {
            node: node.to_owned(),
            dependents: crate::channel::Dependents::Detach,
        }]
    }

    /// What the journeys cannot put a run into: surfaces the rule must leave
    /// alone even though their node left — one raised under no correlation, one
    /// that holds nothing back, one nobody is waiting on — beside the one it
    /// answers, which it answers once however often the removal is recorded.
    #[test]
    fn only_a_standing_decision_about_the_removed_node_is_discharged() {
        let scratch = Scratch::new("scope");
        scratch.raised("gone", Some("c-gone"), true);
        scratch.raised("gone", None, true);
        scratch.raised("gone", Some("c-report"), false);
        scratch.raised("kept", Some("c-kept"), true);

        answer_removed(&scratch.paths, &retried("gone")).expect("the commit is recorded");
        answer_removed(&scratch.paths, &dropped("gone")).expect("and recorded again");

        let answers = scratch.answers();
        assert_eq!(answers.len(), 1, "{answers:#?}");
        assert_eq!(
            answers[0].correlation.as_ref().map(Correlation::as_str),
            Some("c-gone")
        );
        assert_eq!(
            answers[0].reply.reason.as_deref(),
            Some("a committed `retry` of 'gone' superseded it with 'gone-again'")
        );

        // The one under no correlation is marked by its id, and the report and
        // the question about the node still in the graph are left as they were —
        // which is what keeps the run held on that one question.
        let queue = ChannelState::new(&scratch.paths).queue();
        let marked = |correlation: Option<&str>, blocking: bool| {
            queue
                .waiting
                .iter()
                .find(|surface| {
                    surface.correlation.as_ref().map(Correlation::as_str) == correlation
                        && surface.blocking == blocking
                })
                .expect("the surface is still on the queue")
                .abandoned
        };
        assert!(marked(None, true), "the unkeyed decision was left standing");
        assert!(!marked(Some("c-report"), false));
        assert!(!marked(Some("c-kept"), true));
        assert!(crate::views::blocking_surface(&scratch.paths));
    }

    /// A `drop` is named as the drop it was, and an edit removing nothing
    /// answers nothing.
    #[test]
    fn a_drop_is_named_as_a_drop_and_an_edit_removing_nothing_answers_nothing() {
        let scratch = Scratch::new("drop");
        scratch.raised("gone", Some("c-gone"), true);

        answer_removed(&scratch.paths, &[]).expect("nothing to answer");
        assert!(scratch.answers().is_empty());

        answer_removed(&scratch.paths, &dropped("gone")).expect("the commit is recorded");
        let answers = scratch.answers();
        assert_eq!(
            answers[0].reply.reason.as_deref(),
            Some("a committed `drop` removed 'gone' from the graph")
        );
    }

    /// The exception a bound verdict takes is its own envelope's removal of the
    /// question's node, and nothing else.
    #[test]
    fn the_exception_is_this_envelopes_own_removal_and_nothing_else() {
        let scratch = Scratch::new("alongside");
        scratch.raised("gone", Some("c-gone"), true);
        let key: Correlation = "c-gone".parse().expect("a correlation");
        let command = |op: &str, id: &str| {
            serde_json::from_value::<Command>(match op {
                "retry" => serde_json::json!({"op": "retry", "id": id, "node": {"id": "x"}}),
                _ => serde_json::json!({"op": op, "id": id, "dependents": "detach"}),
            })
            .expect("the command parses")
        };

        // Not answered yet: nothing to append beside.
        assert!(!answered_alongside(
            &scratch.paths,
            &key,
            &[command("retry", "gone")]
        ));
        answer_removed(&scratch.paths, &retried("gone")).expect("the commit is recorded");
        assert!(answered_alongside(
            &scratch.paths,
            &key,
            &[command("retry", "gone")]
        ));
        assert!(answered_alongside(
            &scratch.paths,
            &key,
            &[command("drop", "gone")]
        ));
        assert!(!answered_alongside(
            &scratch.paths,
            &key,
            &[command("retry", "other")]
        ));
        assert!(!answered_alongside(&scratch.paths, &key, &[]));
        let stranger: Correlation = "c-never".parse().expect("a correlation");
        assert!(!answered_alongside(
            &scratch.paths,
            &stranger,
            &[command("retry", "gone")]
        ));
    }
}
