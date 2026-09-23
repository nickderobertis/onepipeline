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
use crate::channel::{Author, ChannelState, Command, Reply};
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

/// A finding that asks the planner for a graph edit, by what it is about.
///
/// Half of the correlation such a finding is raised under, so the key says what
/// it is about as well as which node it is about. A closed set and not a word,
/// because it is what the key's identity is derived from: a kind spelled two
/// ways would be two questions about one finding.
///
/// **None of them can recur per attempt**, which is why the attempt is not part
/// of the key: each asks for the edit that takes its node out of the graph, so
/// the finding and the node it is about go together. A kind that could recur
/// would take the attempt beside it, and
/// `the_key_is_derived_from_what_the_contract_states_it_is` is what would notice.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FindingKind {
    /// A session this run cannot open, because the node's branch and its base
    /// conflict. Answered by a `retry` of that node.
    SessionConflict,
}

impl FindingKind {
    /// Every kind, which is what the key's identity is held against.
    ///
    /// Only the gate reads it: nothing in the engine iterates the kinds — each
    /// raise names its own — so a listing compiled into the binary would be a
    /// table nothing consults.
    #[cfg(test)]
    pub(crate) const ALL: [FindingKind; 1] = [FindingKind::SessionConflict];

    /// The word the key carries for it.
    pub(crate) const fn word(self) -> &'static str {
        match self {
            FindingKind::SessionConflict => "session-conflict",
        }
    }

    /// The op a finding of this kind asks the planner to commit.
    pub(crate) const fn asks_for(self) -> Op {
        match self {
            FindingKind::SessionConflict => Op::Retry,
        }
    }
}

/// How much of a node id a correlation carries in readable form.
///
/// A correlation is bounded at 128 bytes and a node id is not, so the readable
/// half is cut and the digest below is what actually distinguishes two keys.
const READABLE: usize = 40;

/// How many hex digits of the digest a correlation carries.
const DIGEST: usize = 12;

/// The graph edit one finding asks the planner to commit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RequestedEdit {
    /// The op it asks for, written and read as the word the reply envelope
    /// spells it with.
    ///
    /// Read back **through the op vocabulary**, so a word no op of this build
    /// carries refuses the record rather than sitting in it matching nothing.
    #[serde(with = "op_word")]
    pub op: Op,
    /// The node it asks for it on, as a graph, a journal label and a surface's
    /// `workstream` all spell it.
    // llmlint: ignore[invalid_states_unrepresentable] `graph::NodeRef` is the repository's
    // node-identity type and it deliberately cannot be built from text — `NodeRef::of` takes
    // a `Node`, which is what keeps an unvalidated string from arriving as an identity — so
    // a record read back off disk cannot produce one. What is written here comes from a
    // `NodeRef` at the raise site, and what it is read for is comparison against the id a
    // committed operation records: the same string on both sides, never resolved into a
    // graph node. `src/channel.rs` carries the same reasoning for the same reason.
    pub node: String,
}

/// `RequestedEdit::op` on the wire: the op's own word, read back through the
/// vocabulary that owns it.
mod op_word {
    use super::Op;
    use serde::{Deserialize, Deserializer, Serializer};

    pub(super) fn serialize<S: Serializer>(op: &Op, into: S) -> Result<S::Ok, S::Error> {
        into.serialize_str(op.word())
    }

    pub(super) fn deserialize<'de, D: Deserializer<'de>>(from: D) -> Result<Op, D::Error> {
        let word = String::deserialize(from)?;
        Op::of_word(&word).ok_or_else(|| {
            serde::de::Error::custom(format!("'{word}' is not an op this build issues"))
        })
    }
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
///
/// The kind and the node are the whole of it — see [`FindingKind`] for why no
/// attempt rides beside them. `None` is a key this build could not spell, which
/// a closed kind and a sanitized, cut node leave no input for; a finding without
/// one is raised exactly as it was before findings had keys — readable,
/// blocking, and answered by a reply rather than by an edit — because a key that
/// could not be built is not a reason to end the loop that is reporting a
/// conflict.
pub(crate) fn correlation(kind: FindingKind, node: &str) -> Option<Correlation> {
    let kind = kind.word();
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
    format!("finding.{kind}.{readable}.{digest}").parse().ok()
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
        correlation.clone(),
        RequestedEdit {
            op,
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
    for (correlation, request) in &recorded {
        if answered.contains(correlation) {
            continue;
        }
        if !operations
            .iter()
            .any(|operation| satisfies(operation, request))
        {
            continue;
        }
        channel.answer_bound(&answer(request), Some(correlation))?;
    }
    Ok(())
}

/// Whether `commands` carry the edit the finding `correlation` names asked for,
/// and the run has already recorded the answer to it.
///
/// The question the one tolerant append in
/// [`ChannelState::answer_alongside`](crate::channel::ChannelState::answer_alongside)
/// turns on, asked of the **envelope** rather than of the channel: whether one
/// reply answered its own question is a fact about that reply. Both halves
/// matter — a verdict naming a question some earlier reply answered carries none
/// of these commands and is refused as it always was, and one whose edit the
/// reconciler turned away has answered nothing yet, so there is nothing to append
/// beside.
///
/// Matched on the command rather than on what it compiled to, because the caller
/// is the submitting process and the compile belongs to whichever writer took the
/// envelope.
pub(crate) fn answered_alongside(
    paths: &RunPaths,
    correlation: &Correlation,
    commands: &[Command],
) -> bool {
    let Some(request) = recorded(paths).remove(correlation) else {
        return false;
    };
    commands.iter().any(|command| {
        crate::channel::op_of(command) == request.op.word()
            && crate::channel::target_of(command).as_deref() == Some(request.node.as_str())
    }) && ChannelState::new(paths).answered().contains(correlation)
}

/// Whether one committed operation is the edit `request` asked for.
///
/// Matched on the **operation** and not on the command: what a command compiles
/// to is the reconciler's, and the operation list is the run's own record of
/// what actually changed.
fn satisfies(operation: &edits::Operation, request: &RequestedEdit) -> bool {
    match operation {
        edits::Operation::RetryRequested { node, .. } => {
            request.op == Op::Retry && request.node == *node
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
            op = request.op.word(),
            node = request.node,
        )),
        ..Reply::default()
    }
}

/// The run's record of what each of its findings asked for, keyed by the
/// correlation each was raised under.
///
/// Read leniently, as every side record is: a run whose file is absent or
/// unreadable has asked for nothing, and the findings it holds stay pending
/// rather than the commit that answers them failing. Keyed by the validated
/// correlation and not by text, so a key this run could not have written is
/// refused by the read rather than carried to the channel.
// llmlint: ignore[changed_behavior_has_e2e] the lenient read has no interface that
// can induce it: this file is written only by the single writer that raises a
// finding, through `ledger::write_json`, which is atomic — so an absent or
// truncated one is a run root somebody else corrupted, and no journey can reach
// that state through the binary. `views.rs`'s read of the channel is lenient on the
// same grounds and carries the same note. The two states it resolves — a run that
// recorded nothing, and a record this build cannot read — are driven by
// `a_run_with_no_recorded_request_answers_nothing_and_an_unreadable_one_refuses_nothing`.
fn recorded(paths: &RunPaths) -> BTreeMap<Correlation, RequestedEdit> {
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
            let kind = FindingKind::SessionConflict;
            let correlation = correlation(kind, node).expect("a key");
            requests(&self.paths, &correlation, kind.asks_for(), node)
                .expect("the request records");
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
        let key = |kind, node: &str| correlation(kind, node).expect("a key");
        let conflict = FindingKind::SessionConflict;
        assert_eq!(
            key(conflict, "service"),
            key(conflict, "service"),
            "the same finding raised again carries a different key"
        );
        assert_ne!(key(conflict, "service"), key(conflict, "other"));
        assert!(key(conflict, "service")
            .as_str()
            .contains("session-conflict.service."));

        // Two ids no correlation alphabet can spell, and no length bounds: the
        // node is sanitized and cut, and the digest is what keeps them apart.
        assert_ne!(
            key(conflict, "ship it/now (please)"),
            key(conflict, "ship it/now (please!)")
        );
        let long = "n".repeat(4096);
        assert_eq!(key(conflict, &long), key(conflict, &long));
        assert_ne!(key(conflict, &long), key(conflict, &format!("{long}n")));
    }

    /// The identity the contract states a finding's key is derived from is the
    /// identity this module derives it from.
    ///
    /// The drift gate for that sentence, beside the derivation rather than only
    /// beside the document: the contract makes the attempt part of the identity
    /// **for a finding that can recur per attempt**, and this build raises none —
    /// each kind asks for the edit that takes its node out of the graph. So the
    /// day a kind arrives that does not, this fails rather than the key quietly
    /// meaning something the contract does not say. `channel.rs`'s
    /// `the_token_prefix_is_the_one_the_contract_and_the_readme_state` is the
    /// same shape.
    #[test]
    fn the_key_is_derived_from_what_the_contract_states_it_is() {
        let contract = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("docs/contract.md"),
        )
        .expect("the contract ships");
        // Read with its wrapping collapsed, so a reflowed paragraph is still the
        // same statement.
        let contract = contract.split_whitespace().collect::<Vec<_>>().join(" ");
        for states in [
            "derived from the finding's kind and the node it concerns",
            "and from the attempt beside them for a finding that can recur per attempt, of which this build raises none",
        ] {
            assert!(
                contract.contains(states),
                "the contract no longer states: {states}"
            );
        }

        // Every kind this build raises asks for the edit that takes its node out
        // of the graph, which is what "raises none" above rests on.
        for kind in FindingKind::ALL {
            assert_eq!(
                kind.asks_for(),
                Op::Retry,
                "'{}' asks for an op that leaves its node in the graph, so its finding can \
                 recur per attempt and its key has to carry the attempt",
                kind.word()
            );
        }

        // And the key moves on the kind and on the node, and on nothing else: a
        // derivation reading a third thing would make one finding two questions.
        let keys: std::collections::BTreeSet<String> = FindingKind::ALL
            .into_iter()
            .flat_map(|kind| {
                ["service", "other"]
                    .into_iter()
                    .map(move |node| correlation(kind, node).expect("a key").as_str().to_owned())
            })
            .collect();
        assert_eq!(keys.len(), FindingKind::ALL.len() * 2, "{keys:?}");
        for kind in FindingKind::ALL {
            assert!(
                keys.contains(correlation(kind, "service").expect("a key").as_str()),
                "the key for '{}' is not the one derived a second time",
                kind.word()
            );
        }
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

        // Unreadable both ways a record can be: not a document at all, and a
        // document whose key is not a correlation this build would have written.
        for written in [
            "{ this is not json",
            r#"{"not a key": {"op": "retry", "node": "x"}}"#,
        ] {
            std::fs::write(file(&scratch.paths), written).expect("the record is written");
            answer_requested(&scratch.paths, &retried("service"))
                .expect("an unreadable record holds");
            assert!(ChannelState::new(&scratch.paths).replies().is_empty());
        }
    }
}
