//! What a reconciler finding asks the planner to commit, and what answers it.
//!
//! A finding the reconciler raises is a report until it asks for something. The
//! ones that ask — today the session-open conflict, whose text asks for a
//! `retry` of the node whose branch will not open — used to be unanswerable: a
//! commands-only envelope is the command path's alone and answers no *question*
//! (`docs/contract.md`'s Channel paragraph), so the manager committed exactly
//! the retry the finding asked for and the finding stayed pending for the life
//! of the run.
//!
//! So a finding that asks for an edit is raised under a **stable correlation**
//! derived from what it is about, the edit it asks for is recorded against that
//! correlation in the run's own directory, and committing that edit appends the
//! answer. The surface record and the reply envelope are untouched: what is new
//! is this side file and which reply the engine writes.

use std::collections::BTreeMap;
use std::path::PathBuf;

use onemessagebus::Correlation;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::channel::layout::Op;
use crate::channel::{Author, ChannelState, Reply};
use crate::edits;
use crate::ledger::RunPaths;
use crate::Result;

/// Where a run records the edit each of its findings asked for.
///
/// Beside the run's other side records rather than inside the surface: the
/// surface record is a published shape — `agent.planner-surface@1`, read
/// byte-for-byte by builds that know nothing of this — and what an engine is
/// waiting to see committed is the engine's own bookkeeping, not part of what a
/// planner reads.
pub(crate) const REQUESTS_FILE: &str = "finding-requests.json";

/// The kind of the finding a session open that conflicts raises.
///
/// Half of its correlation, so the key says what it is about as well as which
/// node it is about.
pub(crate) const SESSION_CONFLICT: &str = "session-conflict";

/// How much of a node id a correlation carries in readable form.
///
/// A correlation is bounded at 128 bytes and a node id is not, so the readable
/// half is cut and the digest below is what actually distinguishes two keys.
const READABLE: usize = 40;

/// How many hex digits of the digest a correlation carries.
const DIGEST: usize = 12;

/// The graph edit one finding asks the planner to commit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct RequestedEdit {
    /// The op word it asks for, as the reply envelope spells it.
    // llmlint: ignore[invalid_states_unrepresentable] `Op` is a `Copy` word table with no
    // serde impl and no place in any wire shape, and this is a **durable record field**: a
    // run written by this build is read back by the next one, so the record carries the op's
    // wire word — the spelling `docs/contract.md` states — and `Op::word` is what writes it.
    pub op: String,
    /// The node it asks for it on.
    pub node: String,
}

/// The correlation a finding of `kind` about `node` is raised under.
///
/// **Stable**: the same finding about the same node yields the same key however
/// often it is raised, which is what lets a manager bind a reply to it with
/// `--correlation` and what keeps a re-raise from minting a second question
/// nobody answered. Readable at the front so a key in a surface record says what
/// it is, and digested at the back because a node id is arbitrary text and a
/// correlation is a bounded alphabet: two ids that sanitize alike still differ
/// here.
pub(crate) fn correlation(kind: &str, node: &str) -> Correlation {
    let mut digest = Sha256::new();
    digest.update(kind.as_bytes());
    // A separator no side of the pair can contain, so ("a", "bc") and ("ab",
    // "c") are not one input.
    digest.update([0u8]);
    digest.update(node.as_bytes());
    let digest: String = digest
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .take(DIGEST)
        .collect();
    let readable: String = node
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
                c
            } else {
                '-'
            }
        })
        .take(READABLE)
        .collect();
    let key = format!("finding.{kind}.{readable}.{digest}");
    key.parse().unwrap_or_else(|_| {
        unreachable!("'{key}' is built from a correlation's own alphabet and is under its bound")
    })
}

/// Record that the finding `correlation` names asks for `op` on `node`.
///
/// Written where the finding is raised, by the single writer that raises it, so
/// there is exactly one author of this file.
///
/// # Errors
///
/// The reason the run's own directory could not be written.
pub(crate) fn requests(
    paths: &RunPaths,
    correlation: &Correlation,
    op: Op,
    node: &str,
) -> Result<()> {
    let mut recorded = recorded(paths);
    recorded.insert(
        correlation.as_str().to_owned(),
        RequestedEdit {
            op: op.word().to_owned(),
            node: node.to_owned(),
        },
    );
    crate::ledger::write_json(&file(paths), &recorded)
}

/// Answer every finding whose requested edit `operations` just committed.
///
/// Called by **both** writers of the graph — the reconcile loop, and a `reply`
/// that found nothing driving the run and became the single writer itself — for
/// [`engine::record_operation_facts`](crate::engine::record_operation_facts)'s
/// reason: which of them applied an edit is an accident of timing, and a finding
/// answered on one path only is a decision that never clears on the other.
///
/// A committed edit that is not the one a finding asked for answers nothing, and
/// a finding a reply already answered — one the manager bound with
/// `--correlation` — is not answered twice.
///
/// # Errors
///
/// The reason the run's channel could not be written.
pub(crate) fn answer_requested(paths: &RunPaths, operations: &[edits::Operation]) -> Result<()> {
    let recorded = recorded(paths);
    if recorded.is_empty() {
        return Ok(());
    }
    let channel = ChannelState::new(paths);
    let answered = channel.answered();
    for (key, request) in &recorded {
        let Ok(correlation) = key.parse::<Correlation>() else {
            continue;
        };
        if answered.contains(&correlation) {
            continue;
        }
        if !operations
            .iter()
            .any(|operation| satisfies(operation, request))
        {
            continue;
        }
        channel.answer_bound(&answer(request), Some(&correlation))?;
    }
    Ok(())
}

/// Whether one committed operation is the edit `request` asked for.
///
/// Matched on the **operation** and not on the command: what a command compiles
/// to is the reconciler's, and the operation list is the run's own record of
/// what actually changed.
fn satisfies(operation: &edits::Operation, request: &RequestedEdit) -> bool {
    match operation {
        edits::Operation::RetryRequested { node, .. } => {
            request.op == Op::Retry.word() && request.node == *node
        }
        _ => false,
    }
}

/// The answer a committed edit appends for the finding that asked for it.
///
/// Authored by the **planner**, which is what the channel's own writer is: a
/// reply's author is what decides which ops it may ask for, and this one asks
/// for nothing — it records that the edit the finding named is on the graph.
fn answer(request: &RequestedEdit) -> Reply {
    Reply {
        author: Author::default(),
        message: Some(format!(
            "answered by the edit it asked for: a `{op}` of '{node}' was committed.",
            op = request.op,
            node = request.node,
        )),
        ..Reply::default()
    }
}

/// The run's record of what each of its findings asked for.
///
/// Read leniently, as every side record is: a run whose file is absent or
/// unreadable has asked for nothing, and the findings it holds stay pending
/// rather than the commit that answers them failing.
fn recorded(paths: &RunPaths) -> BTreeMap<String, RequestedEdit> {
    crate::ledger::read_json_opt(&file(paths)).unwrap_or_default()
}

fn file(paths: &RunPaths) -> PathBuf {
    paths.dir.join(REQUESTS_FILE)
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::channel::{source, Surface, SurfaceKind};

    /// A run directory of its own, removed when the test ends.
    struct Scratch {
        root: PathBuf,
        paths: RunPaths,
    }

    impl Scratch {
        fn new(name: &str) -> Self {
            let root = std::env::temp_dir()
                .join(format!("onepipeline-findings-{name}-{}", crate::sys::pid()));
            let _ = std::fs::remove_dir_all(&root);
            let paths = RunPaths::under(&root, "demo");
            paths.create().expect("the run directory");
            Self { root, paths }
        }

        /// Raise the session-open finding about `node`, with the `retry` it asks
        /// for recorded against it, exactly as the engine raises it.
        fn conflicted(&self, node: &str) -> Correlation {
            let correlation = correlation(SESSION_CONFLICT, node);
            requests(&self.paths, &correlation, Op::Retry, node).expect("the request records");
            ChannelState::new(&self.paths)
                .push(Surface {
                    id: 0,
                    kind: SurfaceKind::FINDING.into(),
                    message: format!("node '{node}' cannot open a session; answer with a `retry`"),
                    source: source::RECONCILER.into(),
                    blocking: true,
                    queued_at: crate::sys::now_millis(),
                    workstream: Some(node.to_owned()),
                    abandoned: false,
                    asker: None,
                    correlation: Some(correlation.clone()),
                })
                .expect("the finding queues");
            correlation
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    fn retried(node: &str) -> Vec<edits::Operation> {
        vec![edits::Operation::RetryRequested {
            node: node.to_owned(),
            replacement: format!("{node}-again"),
            reset: Vec::new(),
        }]
    }

    /// The key a finding is raised under is the same one every time it is
    /// derived, and two findings are never one question.
    ///
    /// The node half is what a manager reads, and the digest half is what makes
    /// it a key: a node id is arbitrary text, so two ids that sanitize to the
    /// same readable prefix must still be two correlations.
    #[test]
    fn a_findings_key_is_stable_and_one_per_kind_and_node() {
        assert_eq!(
            correlation(SESSION_CONFLICT, "service"),
            correlation(SESSION_CONFLICT, "service"),
            "the same finding raised again carries a different key"
        );
        assert_ne!(
            correlation(SESSION_CONFLICT, "service"),
            correlation(SESSION_CONFLICT, "other")
        );
        assert_ne!(
            correlation(SESSION_CONFLICT, "service"),
            correlation("check-in", "service")
        );
        assert!(correlation(SESSION_CONFLICT, "service")
            .as_str()
            .contains("session-conflict.service."));

        // Two ids no correlation alphabet can spell, and no length bounds.
        let odd = correlation(SESSION_CONFLICT, "ship it/now (please)");
        assert_ne!(odd, correlation(SESSION_CONFLICT, "ship it/now (please!)"));
        let long = "n".repeat(4096);
        assert_eq!(
            correlation(SESSION_CONFLICT, &long),
            correlation(SESSION_CONFLICT, &long)
        );
        assert_ne!(
            correlation(SESSION_CONFLICT, &long),
            correlation(SESSION_CONFLICT, &format!("{long}n"))
        );
    }

    /// Committing the edit a finding asked for answers that finding — once — and
    /// leaves it out of both the pending slot and what a run is held on.
    #[test]
    fn committing_the_requested_edit_answers_the_finding_exactly_once() {
        let scratch = Scratch::new("answered");
        let correlation = scratch.conflicted("service");
        let channel = ChannelState::new(&scratch.paths);
        // Read off the queue, which is where a decision is answered from: the
        // finding is what `status` reports as the decision the run waits on.
        channel.claim().expect("the finding is claimed");
        assert!(channel.held().is_some());
        assert!(crate::views::blocking_surface(&scratch.paths));

        answer_requested(&scratch.paths, &retried("service")).expect("the commit answers it");

        let replies = channel.replies();
        assert_eq!(replies.len(), 1, "{replies:?}");
        assert_eq!(replies[0].correlation.as_ref(), Some(&correlation));
        assert!(
            replies[0]
                .reply
                .message
                .as_deref()
                .is_some_and(|said| said.contains("`retry` of 'service'")),
            "the answer does not name the edit that answered it: {replies:?}"
        );
        assert_eq!(channel.held(), None, "the pending slot was not released");
        assert!(!crate::views::blocking_surface(&scratch.paths));

        // The same edit committed again — or the reply the manager bound with
        // `--correlation` arriving beside it — answers an answered finding
        // nothing further.
        answer_requested(&scratch.paths, &retried("service")).expect("nothing is owed twice");
        assert_eq!(channel.replies().len(), 1);
    }

    /// A finding nobody claimed is answered the same way, and stops being a
    /// decision the run is held on while it is still sitting in `waiting`.
    #[test]
    fn an_unread_finding_answered_by_its_edit_is_no_longer_a_decision() {
        let scratch = Scratch::new("unread");
        scratch.conflicted("service");
        assert!(crate::views::blocking_surface(&scratch.paths));

        answer_requested(&scratch.paths, &retried("service")).expect("the commit answers it");

        assert!(!crate::views::blocking_surface(&scratch.paths));
        assert_eq!(
            ChannelState::new(&scratch.paths).queue().waiting.len(),
            1,
            "the record left the queue rather than being answered on it"
        );
    }

    /// A committed edit that is not the one the finding asked for answers
    /// nothing: neither another node's retry, nor another op on the same node.
    #[test]
    fn an_edit_a_finding_did_not_ask_for_answers_nothing() {
        let scratch = Scratch::new("unrelated");
        scratch.conflicted("service");
        let channel = ChannelState::new(&scratch.paths);

        for operations in [
            retried("other"),
            vec![edits::Operation::NodeDropped {
                node: "service".to_owned(),
                dependents: crate::channel::Dependents::Detach,
            }],
        ] {
            answer_requested(&scratch.paths, &operations).expect("the commit is recorded");
            assert!(
                channel.replies().is_empty(),
                "an edit nobody asked for answered a finding: {operations:?}"
            );
            assert!(crate::views::blocking_surface(&scratch.paths));
        }
    }

    /// A run that recorded no request pays no channel read, and one whose record
    /// cannot be read holds its findings rather than failing the commit.
    #[test]
    fn a_run_with_no_recorded_request_answers_nothing_and_an_unreadable_one_refuses_nothing() {
        let scratch = Scratch::new("absent");
        answer_requested(&scratch.paths, &retried("service")).expect("nothing to answer");
        assert!(ChannelState::new(&scratch.paths).replies().is_empty());

        std::fs::write(file(&scratch.paths), "{ this is not json").expect("the record is written");
        answer_requested(&scratch.paths, &retried("service")).expect("an unreadable record holds");
        assert!(ChannelState::new(&scratch.paths).replies().is_empty());
    }
}
