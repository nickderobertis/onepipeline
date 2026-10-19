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
use std::path::Path;
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
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Landing {
    /// The node whose landing it is, or `None` outside a run.
    pub(crate) node: Option<String>,
    /// The commit the landing reached its base at: what one landing is deduplicated by.
    pub(crate) commit: String,
    /// The issues, as the landing spelled them, each a readable `owner/name#N`.
    pub(crate) closes: Vec<String>,
}

impl Landing {
    /// The landing a relayed `onevcs` `merge-completed` or `change-merged` reports, labelled
    /// for the node it is about — `None` for any other event, one labelled for no node, one
    /// whose commit cannot be rendered, and one whose `closes` is missing or `[]`.
    ///
    /// A `closes` that is not a list of `owner/name#N` is said on standard error and read as
    /// no landing at all: handing the store part of a set as though it were the whole is the
    /// one thing this must never do.
    pub(crate) fn of_event(envelope: &Envelope) -> Option<Self> {
        let node = envelope.labels.node.clone()?;
        Self::of(envelope, Some(node))
    }

    /// The same, for a landing the run's own record does not label: what a landing verb's
    /// operation recorded, outside any run.
    pub(crate) fn of_unlabelled(envelope: &Envelope) -> Option<Self> {
        Self::of(envelope, None)
    }

    /// The landing one of a landing verb's own operation records reports.
    pub(crate) fn of_operation(envelope: &onevcs::Envelope) -> Option<Self> {
        let value = serde_json::to_value(envelope).ok()?;
        let crossed: Envelope = serde_json::from_value(value).ok()?;
        Self::of_unlabelled(&crossed)
    }

    fn of(envelope: &Envelope, node: Option<String>) -> Option<Self> {
        if envelope.source != Source::Vcs || !crate::vcs::is_landing(&envelope.kind) {
            return None;
        }
        let commit = crate::vcs::usable(envelope.payload.get("sha")?.as_str()?)?;
        let closes = match closes_of(envelope.payload.get("closes")) {
            Ok(closes) => closes,
            Err(why) => {
                eprintln!(
                    "onepipeline: the landing at {commit} reports `closes` this build cannot \
                     read ({why}), so no issue it closes is handed to the store"
                );
                return None;
            }
        };
        (!closes.is_empty()).then_some(Self {
            node,
            commit,
            closes,
        })
    }

    /// The issues, as the store takes them.
    fn issues(&self) -> Vec<IssueRef> {
        self.closes
            .iter()
            .filter_map(|spelled| IssueRef::from_str(spelled).ok())
            .collect()
    }
}

/// The `closes` a landing reports: `[]` where it has none — an event an older `onevcs` wrote —
/// and a refusal of the whole where any entry is not an issue reference.
fn closes_of(closes: Option<&Value>) -> Result<Vec<String>, String> {
    let Some(closes) = closes else {
        return Ok(Vec::new());
    };
    let items = closes
        .as_array()
        .ok_or_else(|| format!("{closes} is not a list"))?;
    items
        .iter()
        .map(|item| {
            let spelled = item
                .as_str()
                .ok_or_else(|| format!("{item} is not a string"))?;
            IssueRef::from_str(spelled)
                .map(|_| spelled.to_owned())
                .map_err(|why| format!("`{spelled}` is not `owner/name#N`: {why}"))
        })
        .collect()
}

/// Every landing commit a `landing-closures` record of `journal` already names: what a
/// catch-up asks nothing about again.
pub(crate) fn recorded(journal: &Path) -> BTreeSet<String> {
    crate::journal::read(journal)
        .into_iter()
        .filter(|event| {
            event.source == Source::Pipeline
                && event.kind.as_str() == PipelineKind::LandingClosures.as_str()
        })
        .filter_map(|event| {
            event
                .payload
                .get("commit")
                .and_then(Value::as_str)
                .map(str::to_owned)
        })
        .collect()
}

/// What one closure call came to.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Answer {
    /// No configuration declares a source, so nothing was asked.
    Unconfigured,
    /// The call finished: projected, or failed for good. This is the record.
    Finished(LandingClosures),
    /// The call failed in a way a wait can change: asked again after `wait` where the store
    /// named one, or on the caller's schedule. `record` is what a caller that does not ask
    /// again records.
    Transient {
        record: LandingClosures,
        wait: Option<Duration>,
    },
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
            return Answer::Transient {
                record: failed(
                    FailureClass::Transient,
                    format!("the store cannot be called: {error}"),
                ),
                wait: None,
            }
        }
    };
    let issues = landing.issues();
    let answered = runtime.block_on(async {
        tokio::time::timeout(within, built.engine.complete_issues(&issues, &[], false)).await
    });
    match answered {
        Err(_) => Answer::Transient {
            record: failed(
                FailureClass::Transient,
                format!(
                    "task status complete exceeded {} seconds",
                    within.as_secs()
                ),
            ),
            wait: None,
        },
        Ok(Err(error)) => {
            let class = crate::writeback::class_of_engine(&error);
            let reason = error.to_string();
            match class {
                FailureClass::Refused => Answer::Finished(failed(class, reason)),
                FailureClass::Transient => Answer::Transient {
                    record: failed(class, reason),
                    wait: None,
                },
            }
        }
        Ok(Ok(completed)) => of_completed(landing, &completed),
    }
}

/// The record a finished call's answer makes, or the retry a transient item's failure asks
/// for.
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
    // Every failure the answer carries: an item's own, and a delivered task's the store could
    // not keep in step with it.
    let failures: Vec<&onetaskgraph_core::Failure> = completed
        .items
        .iter()
        .flat_map(|item| {
            let own = item
                .error
                .as_ref()
                .filter(|_| item.action == CompleteAction::Failed);
            let delivered = item.delivered.iter().filter_map(|entry| match &entry.outcome {
                DeliveryOutcome::Failed { failure, .. } => Some(failure),
                DeliveryOutcome::Written { .. }
                | DeliveryOutcome::Unchanged { .. }
                | DeliveryOutcome::Left { .. } => None,
            });
            own.into_iter().chain(delivered)
        })
        .collect();
    if failures.is_empty() {
        return Answer::Finished(verbatim);
    }
    let transient = failures
        .iter()
        .any(|failure| failure.class() == onetaskgraph_core::FailureClass::Transient);
    let class = if transient {
        FailureClass::Transient
    } else {
        FailureClass::Refused
    };
    let reason = failures
        .iter()
        .map(|failure| crate::views::one_line(failure.message()))
        .collect::<Vec<_>>()
        .join("; ");
    let failed = LandingClosures {
        outcome: ClosureOutcome::Failed,
        class: Some(class),
        reason: Some(reason),
        ..verbatim
    };
    if transient {
        let wait = failures
            .iter()
            .filter_map(|failure| failure.retry_after_seconds())
            .max()
            .map(Duration::from_secs);
        Answer::Transient {
            record: failed,
            wait,
        }
    } else {
        Answer::Finished(failed)
    }
}

/// The record of one landing, before anything was asked: projected, with nothing in it.
fn record(landing: &Landing) -> LandingClosures {
    LandingClosures {
        node: landing.node.clone(),
        commit: landing.commit.clone(),
        closes: landing.closes.clone(),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_closes_that_is_not_every_issue_reference_is_refused_whole() {
        assert_eq!(closes_of(None), Ok(Vec::new()));
        assert_eq!(
            closes_of(Some(&serde_json::json!(["owner/service#7", "owner/service#8"]))),
            Ok(vec!["owner/service#7".to_owned(), "owner/service#8".to_owned()])
        );
        assert!(closes_of(Some(&serde_json::json!(["owner/service#7", "#8"]))).is_err());
        assert!(closes_of(Some(&serde_json::json!("owner/service#7"))).is_err());
        assert!(closes_of(Some(&serde_json::json!([7]))).is_err());
    }
}
