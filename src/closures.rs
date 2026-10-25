//! The plan-store items whose GitHub issues a landing closed, moved to `done` through the
//! store: entry 132 of `docs/contract-divergences.md`.
//!
//! Every landing `onevcs` reports carries `closes`, the GitHub issues it closes as
//! `owner/name#N`. This crate writes no ticket a `delivers` names; it hands those issues to the
//! store in one [`Engine::complete_issues`] call naming no source, and the store decides, per
//! configured source and issue, what moves. A landing whose `closes` is empty is handed to
//! nobody, so it costs no request at all.
//!
//! This module is the three pieces every caller shares: reading a landing off a relayed
//! event ([`Landing::of_event`]) or a landing verb's own operation record
//! ([`Landing::of_operation`]), making the call ([`complete`]), and the `landing-closures`
//! record it ends in ([`LandingClosures`]). *When* a call is made is the caller's: the
//! write-back worker's closure thread for a run (`crate::writeback`), and the landing verbs
//! (`crate::land`) outside one.

use std::collections::BTreeSet;
use std::str::FromStr;
use std::time::Duration;

use onetaskgraph_core::config::Layer;
use onetaskgraph_core::{CompleteAction, DeliveryOutcome, IssuesCompleted};
use onetaskgraph_plugin_api::IssueRef;
use serde_json::Value;

use crate::event::{Envelope, PipelineKind, Source};
use crate::payload::{ClosureOutcome, LandingClosures};
use crate::taskgraph::Store;
use crate::writeback::FailureClass;

/// One landing whose `closes` names at least one issue: what a closure call is made for.
///
/// Built only by [`Landing::of`], which holds it to what a call needs — a usable landing
/// commit and at least one issue, each read as an [`IssueRef`] — so nothing past it can be
/// handed an empty or unread set.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Landing {
    /// The node whose landing it is, or `None` outside a run.
    node: Option<String>,
    /// The commit the landing reached its base at: what one landing is deduplicated by.
    commit: String,
    /// The issues, in the order the landing reported them; never empty.
    issues: Vec<IssueRef>,
}

impl Landing {
    /// The landing a relayed `onevcs` `merge-completed` or `change-merged` reports, labelled
    /// for the node it is about — `None` for any other event, one labelled for no node, one
    /// whose commit cannot be rendered, and one whose `closes` is missing or `[]`.
    pub(crate) fn of_event(envelope: &Envelope) -> Option<Self> {
        let node = envelope.labels.node.clone()?;
        Self::of(&crossed(envelope)?, Some(node))
    }

    /// The same, for a record of a node's own session stream, which the caller names the node
    /// of.
    pub(crate) fn of_unlabelled(envelope: &Envelope) -> Option<Self> {
        Self::of(&crossed(envelope)?, None)
    }

    /// The landing one of `onevcs`'s own records reports, for no node: what a landing verb's
    /// operation recorded, or what a node's session stream holds before the caller names the
    /// node it is.
    pub(crate) fn of_operation(envelope: &onevcs::Envelope) -> Option<Self> {
        Self::of(envelope, None)
    }

    pub(crate) fn of_node(self, node: String) -> Self {
        Self {
            node: Some(node),
            ..self
        }
    }

    pub(crate) fn commit(&self) -> &str {
        &self.commit
    }

    /// How many issues the landing closes, which is what a call's deadline is sized by.
    pub(crate) fn issue_count(&self) -> usize {
        self.issues.len()
    }

    /// The issues, each spelled `owner/name#N`.
    pub(crate) fn closes(&self) -> Vec<String> {
        self.issues.iter().map(ToString::to_string).collect()
    }

    /// Read `closes` through `onevcs`'s own reader, [`onevcs::landing_closes`], which answers
    /// `[]` for an event an older release wrote and refuses one whose `closes` is not a list of
    /// `owner/name#N` — read as no landing at all, and said: handing the store part of a set
    /// as though it were the whole is the one thing this must never do.
    fn of(envelope: &onevcs::Envelope, node: Option<String>) -> Option<Self> {
        let kind = crate::event::EventKind(envelope.kind.as_str().to_owned());
        if !crate::vcs::is_landing(&kind) {
            return None;
        }
        let commit = crate::vcs::usable(envelope.payload.get("sha")?.as_str()?)?;
        let read = onevcs::landing_closes(envelope).and_then(|closes| {
            closes
                .iter()
                .map(|spelled| {
                    IssueRef::from_str(spelled).map_err(|why| onevcs::Error::Invalid {
                        reason: format!("`{spelled}` is not an issue reference: {why}"),
                    })
                })
                .collect::<onevcs::Result<Vec<IssueRef>>>()
        });
        let issues = match read {
            Ok(issues) => issues,
            // llmlint: ignore[changed_behavior_has_e2e] `closes` is written by one function of
            // `onevcs`, `closing::report`, which always writes a list of `owner/name#N`; a value
            // this refuses is a session stream edited by hand, which no journey through either
            // CLI writes. The refusal itself is `onevcs::landing_closes`'s, held by its own suite.
            Err(why) => {
                eprintln!(
                    "onepipeline: the landing at {commit} reports `closes` this build cannot \
                     read ({why}), so no issue it closes is handed to the store"
                );
                return None;
            }
        };
        (!issues.is_empty()).then_some(Self {
            node,
            commit,
            issues,
        })
    }
}

/// A relayed record, back in `onevcs`'s own envelope: the wire shape the two share, so
/// `onevcs`'s reader reads it as it wrote it.
fn crossed(envelope: &Envelope) -> Option<onevcs::Envelope> {
    serde_json::to_value(envelope)
        .ok()
        .and_then(|value| serde_json::from_value(value).ok())
}

/// What one closure call came to.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Answer {
    /// No configuration declares a source, so nothing was asked.
    Unconfigured,
    /// The call finished: projected, or failed for good. This is the record.
    Finished(LandingClosures),
    /// The call failed in a way a wait can change, asked again on the caller's schedule.
    /// `record` is what a caller that does not ask again records.
    Transient(LandingClosures),
}

/// Hand the store the issues one landing closed, in one [`Engine::complete_issues`] call
/// naming no source, within `within`.
///
/// The store configuration is the one `store` discovers, with every source it declares; the
/// store asks each source able to hold an item for a GitHub issue and sends every other
/// source nothing. Never panics and never fails: every way the call can go wrong is an
/// [`Answer`].
///
/// [`Engine::complete_issues`]: onetaskgraph_core::Engine::complete_issues
pub(crate) fn complete(store: &Store, landing: &Landing, within: Duration) -> Answer {
    let failed = |class, reason: String| LandingClosures {
        outcome: ClosureOutcome::Failed,
        class: Some(class),
        reason: Some(reason),
        ..record(landing)
    };
    let built = match store.engine(&Layer::default()) {
        Ok(built) => built,
        Err(error) => {
            return Answer::Finished(failed(
                FailureClass::Refused,
                format!("the store configuration does not load: {error}"),
            ))
        }
    };
    if built.config.sources().is_empty() {
        return Answer::Unconfigured;
    }
    let runtime = match crate::taskgraph::runtime() {
        Ok(runtime) => runtime,
        // llmlint: ignore[changed_behavior_has_e2e] a host refusing this process a runtime is
        // resource exhaustion no CLI journey can arrange; it is said and asked again.
        Err(error) => {
            return Answer::Transient(failed(
                FailureClass::Transient,
                format!("the store cannot be called: {error}"),
            ))
        }
    };
    let answered = runtime.block_on(async {
        tokio::time::timeout(
            within,
            built.engine.complete_issues(&landing.issues, &[], false),
        )
        .await
    });
    match answered {
        Err(_) => Answer::Transient(failed(
            FailureClass::Transient,
            format!("task status complete exceeded {} seconds", within.as_secs()),
        )),
        // llmlint: ignore[changed_behavior_has_e2e] unreachable by the producer's own contract:
        // `complete_issues` fails only with `UnknownSource`, for a source its `only` names, and
        // this call names none — a source that fails to read and a write that fails are items
        // `failed` in an `Ok` answer, which the journeys drive. Recorded failed for good, in the
        // store's own class, rather than assumed away.
        Ok(Err(error)) => Answer::Finished(failed(
            crate::writeback::class_of_engine(&error),
            error.to_string(),
        )),
        Ok(Ok(completed)) => of_completed(landing, &completed),
    }
}

/// The record a finished call's answer makes, or the retry a transient item's failure asks
/// for.
///
/// Failed wherever the store says the answer failed ([`IssuesCompleted::failed`]): an item that
/// could not be written or whose source could not be read, and a ticket an updated item delivers
/// that the store could not keep in step with it — with the class and words of every one of those
/// failures, a transient one deciding the class. The items stay the store's, verbatim.
fn of_completed(landing: &Landing, completed: &IssuesCompleted) -> Answer {
    let verbatim = LandingClosures {
        items: completed
            .items
            .iter()
            .filter_map(|item| serde_json::to_value(item).ok())
            .collect(),
        unsupported: completed
            .unsupported
            .iter()
            .map(|source| source.as_str().to_owned())
            .collect(),
        ..record(landing)
    };
    if !completed.failed() {
        return Answer::Finished(verbatim);
    }
    let failures: Vec<&onetaskgraph_core::Failure> = completed
        .items
        .iter()
        .flat_map(|item| {
            let own = item
                .error
                .as_ref()
                .filter(|_| item.action == CompleteAction::Failed);
            let delivered = item
                .delivered
                .iter()
                .filter_map(|entry| match &entry.outcome {
                    DeliveryOutcome::Failed { failure, .. } => Some(failure),
                    DeliveryOutcome::Written { .. }
                    | DeliveryOutcome::Unchanged { .. }
                    | DeliveryOutcome::Left { .. } => None,
                });
            own.into_iter().chain(delivered)
        })
        .collect();
    let transient = failures
        .iter()
        .any(|failure| failure.class() == onetaskgraph_core::FailureClass::Transient);
    let failed = LandingClosures {
        outcome: ClosureOutcome::Failed,
        class: Some(if transient {
            FailureClass::Transient
        } else {
            FailureClass::Refused
        }),
        reason: Some(
            failures
                .iter()
                .map(|failure| crate::views::one_line(failure.message()))
                .collect::<Vec<_>>()
                .join("; "),
        ),
        ..verbatim
    };
    if transient {
        Answer::Transient(failed)
    } else {
        Answer::Finished(failed)
    }
}

/// Whether a record carries a ticket an updated item delivers that the store could not keep in
/// step: the one failure a retry cannot answer, since `complete_issues` re-evaluates delivered
/// tickets only as it writes their deliverer, and a retry reads that item back `unchanged`.
pub(crate) fn carries_delivered_failure(record: &LandingClosures) -> bool {
    record.items.iter().any(|item| {
        item.get("delivered")
            .and_then(Value::as_array)
            .is_some_and(|delivered| {
                delivered
                    .iter()
                    .any(|entry| entry.get("outcome").and_then(Value::as_str) == Some("failed"))
            })
    })
}

/// The record a retried closure ends in: what the last call answered — unless that finished
/// with nothing failed after an earlier attempt carried a delivered ticket's failure, which the
/// last call gave no word on. That earlier attempt is kept, failed, its items verbatim.
pub(crate) fn kept_after_retry(
    carried: Option<LandingClosures>,
    finished: LandingClosures,
) -> LandingClosures {
    match carried {
        Some(carried) if finished.outcome == ClosureOutcome::Projected => carried,
        _ => finished,
    }
}

/// The record of one landing, before anything was asked: projected, with nothing in it.
fn record(landing: &Landing) -> LandingClosures {
    LandingClosures {
        node: landing.node.clone(),
        commit: landing.commit.clone(),
        closes: landing.closes(),
        outcome: ClosureOutcome::Projected,
        items: Vec::new(),
        unsupported: Vec::new(),
        class: None,
        reason: None,
    }
}

/// What a record says, as the lines a landing verb prints on standard error.
pub(crate) fn said(record: &LandingClosures) -> String {
    let mut lines = vec![format!(
        "onepipeline: the landing at {} closes {}; the store answered:",
        record.commit,
        record.closes.join(", ")
    )];
    for item in &record.items {
        let read = |key: &str| item.get(key).and_then(Value::as_str).unwrap_or("-");
        let mut line = format!(
            "  {} on {}: {}",
            read("issue"),
            read("source"),
            read("action")
        );
        if let Some(task) = item.get("task").and_then(Value::as_str) {
            line.push_str(&format!(" ({task})"));
        }
        if let Some(reason) = item.get("reason").and_then(Value::as_str) {
            line.push_str(&format!(" — {reason}"));
        }
        lines.push(line);
    }
    if !record.unsupported.is_empty() {
        lines.push(format!(
            "  sent no request (no item by issue): {}",
            record.unsupported.join(", ")
        ));
    }
    match (&record.outcome, &record.class, &record.reason) {
        (ClosureOutcome::Failed, Some(class), Some(reason)) => lines.push(format!(
            "  failed ({}): {}",
            serde_json::to_value(class)
                .ok()
                .and_then(|class| class.as_str().map(str::to_owned))
                .unwrap_or_default(),
            crate::views::one_line(reason)
        )),
        _ => lines.push("  projected".to_owned()),
    }
    lines.join("\n")
}

/// What one driver's reconcile loop owes the closure thread, read off the run's own journal
/// and sessions and never off a board or another run.
///
/// Three things hand a landing over, each once per landing commit: a relayed landing event
/// labelled for one of the run's nodes whose `closes` is non-empty, read off the journal as it
/// grows; and a **catch-up** — on a driver's first pass, after an adoption's standings read,
/// and again whenever the run meets a landing it did not watch happen (a `node-landed`, or a
/// landing event on a settled node's session stream) — which reads the session stream of every
/// node with a recorded landing. A landing a `landing-closures` record already names is never
/// handed over again, so stopping and adopting a run asks the store nothing new.
pub(crate) struct Watch {
    /// How far into the journal this driver has read.
    read_to: u64,
    /// Every landing commit a `landing-closures` record names.
    recorded: BTreeSet<String>,
    /// Whether a catch-up is due on the next pass.
    catch_up: bool,
    /// The nodes whose session a catch-up of this driver has read since they landed.
    scanned: BTreeSet<String>,
    /// The settled nodes whose session streams carry a landing the run had not seen, whose
    /// streams the next pass reads whatever was read of them before.
    met: BTreeSet<String>,
}

impl Default for Watch {
    fn default() -> Self {
        Self::new()
    }
}

impl Watch {
    /// A driver's watch, owing a catch-up on its first pass.
    pub(crate) fn new() -> Self {
        Self {
            read_to: 0,
            recorded: BTreeSet::new(),
            catch_up: true,
            scanned: BTreeSet::new(),
            met: BTreeSet::new(),
        }
    }

    /// The settled `nodes` whose session streams carry a landing the run did not watch happen:
    /// read each of them on the next pass.
    pub(crate) fn met_landings(&mut self, nodes: BTreeSet<String>) {
        self.met.extend(nodes);
    }

    /// One pass: hand `writeback` every landing of the run the journal or a catch-up shows
    /// that no record names yet. Reads the journal only past where it last stopped, and a
    /// session stream only for a node that landed since this driver last read it.
    pub(crate) fn pass(
        &mut self,
        paths: &crate::ledger::RunPaths,
        state: &crate::projection::RunState,
        writeback: &crate::writeback::Writeback,
    ) {
        let grown = crate::journal::finished_records_after(&paths.journal(), self.read_to);
        let mut relayed = Vec::new();
        for (record, occupies) in grown {
            self.read_to += occupies;
            let Some(record) = record else { continue };
            if record.source == Source::Pipeline {
                match PipelineKind::from_wire(&record.kind) {
                    Some(PipelineKind::LandingClosures) => {
                        if let Some(commit) = record.payload.get("commit").and_then(Value::as_str) {
                            self.recorded.insert(commit.to_owned());
                        }
                    }
                    Some(PipelineKind::NodeLanded) => self.catch_up = true,
                    _ => {}
                }
                continue;
            }
            let ours = record
                .labels
                .node
                .as_deref()
                .is_some_and(|node| state.graph.get(node).is_some());
            if ours {
                if let Some(landing) = Landing::of_event(&record) {
                    relayed.push(landing);
                }
            }
        }
        // Records first: a record later in the same read names a landing relayed earlier in
        // it, which a run adopted after its closure was written must not ask about again.
        for landing in relayed {
            self.owe(landing, writeback);
        }
        let mut due = std::mem::take(&mut self.met);
        if std::mem::take(&mut self.catch_up) {
            due.extend(
                landed_nodes(state)
                    .into_iter()
                    .filter(|node| !self.scanned.contains(node)),
            );
        }
        for node in due {
            let Some(session) = state
                .sessions
                .get(&node)
                .or_else(|| state.abandoned.get(&node))
            else {
                continue;
            };
            self.scanned.insert(node.clone());
            for envelope in crate::vcs::events(session.token(), None) {
                if let Some(landing) = Landing::of_unlabelled(&envelope) {
                    self.owe(landing.of_node(node.clone()), writeback);
                }
            }
        }
    }

    fn owe(&self, landing: Landing, writeback: &crate::writeback::Writeback) {
        if !self.recorded.contains(landing.commit()) {
            writeback.close_landing(landing);
        }
    }
}

/// Every node of the run with a recorded landing: one its settlement saw land, one whose
/// `merge-completed` was relayed, and one a readjustment found landed.
fn landed_nodes(state: &crate::projection::RunState) -> BTreeSet<String> {
    state
        .landings
        .iter()
        .filter(|(_, landing)| **landing == crate::graph::Landing::Landed)
        .map(|(node, _)| node.clone())
        .chain(state.landing_commits.keys().cloned())
        .chain(state.landed_how.keys().cloned())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn landing(closes: &Value) -> Option<Landing> {
        let envelope: onevcs::Envelope = serde_json::from_value(serde_json::json!({
            "v": 2, "ts": "2026-10-19T00:00:00.000Z", "stream": "s", "seq": 0,
            "source": "vcs", "kind": "merge-completed", "labels": {},
            "payload": {"sha": "0123456789abcdef0123456789abcdef01234567", "closes": closes},
            "artifacts": [],
        }))
        .expect("an onevcs envelope");
        Landing::of_operation(&envelope)
    }

    #[test]
    fn a_landing_carries_every_issue_its_closes_names_or_none_at_all() {
        let read = landing(&serde_json::json!(["owner/service#7", "owner/service#8"]))
            .expect("a landing closing two issues");
        assert_eq!(read.closes(), ["owner/service#7", "owner/service#8"]);
        assert_eq!(read.issue_count(), 2);
        assert_eq!(read.commit(), "0123456789abcdef0123456789abcdef01234567");
        assert_eq!(landing(&serde_json::json!([])), None);
        assert_eq!(landing(&serde_json::json!(["owner/service#7", "#8"])), None);
        assert_eq!(landing(&serde_json::json!("owner/service#7")), None);
    }
}
