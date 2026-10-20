//! A run's home — the plan project it was launched from — marked `done` through the store once
//! the run reaches the success ending: entry 134 of `docs/contract-divergences.md`.
//!
//! This crate writes no ticket a `delivers` names and no member project; the one item of its
//! own it writes is the run's home, once, to `done`, in one [`Engine::set_project_status`] call.
//! *When* is the engine's close-out of a run [`crate::hooks::succeeded`] judges complete, and
//! the call is the write-back's closure thread's (`crate::writeback`); this module is the call
//! ([`set_done`]), the `home-status` record it ends in, and what `results` says of that record
//! ([`results_line`]).
//!
//! [`Engine::set_project_status`]: onetaskgraph_core::Engine::set_project_status

use std::time::Duration;

use onetaskgraph_core::config::Layer;
use onetaskgraph_plugin_api::StatusCategory;
use serde_json::Value;

use crate::journal::{self, PipelineKind};
use crate::payload::{HomeOutcome, HomeStatus};
use crate::taskgraph::{QualifiedId, Store};
use crate::writeback::FailureClass;

/// What one call came to.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Answer {
    /// The call finished: written, or failed for good. This is the record.
    Finished(HomeStatus),
    /// The call failed in a way a wait can change, asked again on the caller's schedule;
    /// `reason` is the store's words for it.
    Transient(String),
}

/// Ask the store to mark `home` `done`, and nothing else about it, within `within`.
///
/// The store configuration is the one `store` discovers, which for a run is its launch
/// directory's. Never panics and never fails: every way the call can go wrong is an
/// [`Answer`].
pub(crate) fn set_done(store: &Store, home: &QualifiedId, within: Duration) -> Answer {
    let failed = |class, reason: String| record(home, Err((class, reason)));
    let built = match store.engine(&Layer::default()) {
        Ok(built) => built,
        Err(error) => {
            return Answer::Finished(failed(
                FailureClass::Refused,
                format!("the store configuration does not load: {error}"),
            ))
        }
    };
    let runtime = match crate::taskgraph::runtime() {
        Ok(runtime) => runtime,
        // llmlint: ignore-block[changed_behavior_has_e2e] a host refusing this process a runtime
        // is resource exhaustion no CLI journey can arrange; it is said and asked again.
        Err(error) => return Answer::Transient(format!("the store cannot be called: {error}")),
        // llmlint: ignore-end[changed_behavior_has_e2e]
    };
    let answered = runtime.block_on(async {
        tokio::time::timeout(
            within,
            built
                .engine
                .set_project_status(&home.global(), StatusCategory::Done),
        )
        .await
    });
    match answered {
        // llmlint: ignore-block[changed_behavior_has_e2e] the deadline is the sixty-second
        // command floor plus one item, and a settled run's close-out waits thirty seconds for its
        // home, so a driver lets go — leaving no record, for the next driver to make the call —
        // before this arm can answer at close-out; it bounds a call a driver driving again after
        // an edit left running, and answers as the transient arm the failing-board journeys drive.
        Err(_) => Answer::Transient(format!(
            "project status set exceeded {} seconds",
            within.as_secs()
        )),
        // llmlint: ignore-end[changed_behavior_has_e2e]
        Ok(Err(error)) => match crate::writeback::class_of_engine(&error) {
            FailureClass::Transient => {
                Answer::Transient(crate::views::one_line(&error.to_string()))
            }
            FailureClass::Refused => Answer::Finished(failed(
                FailureClass::Refused,
                crate::views::one_line(&error.to_string()),
            )),
        },
        Ok(Ok(set)) => Answer::Finished(record(home, Ok(set.status))),
    }
}

/// The one record a finished call makes: `status` exactly when it was written, `class` and
/// `reason` exactly when it failed.
fn record(
    home: &QualifiedId,
    answered: Result<onetaskgraph_plugin_api::Status, (FailureClass, String)>,
) -> HomeStatus {
    match answered {
        Ok(status) => HomeStatus {
            project: home.to_string(),
            outcome: HomeOutcome::Written,
            status: Some(status),
            class: None,
            reason: None,
        },
        Err((class, reason)) => HomeStatus {
            project: home.to_string(),
            outcome: HomeOutcome::Failed,
            status: None,
            class: Some(class),
            reason: Some(reason),
        },
    }
}

/// Whether `run`'s journal already holds a `home-status` record: a run that has one writes
/// its home nothing more, whatever endings follow.
pub(crate) fn recorded(paths: &crate::ledger::RunPaths) -> bool {
    journal::read(&paths.journal())
        .iter()
        .any(|event| PipelineKind::from_wire(&event.kind) == Some(PipelineKind::HomeStatus))
}

/// What `results` says of the run's `home-status` record, or nothing where it holds none.
pub(crate) fn results_line(view: &crate::views::RunView) -> String {
    let events = journal::read(&view.paths.journal());
    let Some(record) = events
        .iter()
        .find(|event| PipelineKind::from_wire(&event.kind) == Some(PipelineKind::HomeStatus))
    else {
        return String::new();
    };
    let read = |key: &str| {
        record
            .payload
            .get(key)
            .and_then(Value::as_str)
            .map(crate::views::one_line)
            .unwrap_or_else(|| "-".to_owned())
    };
    match record.payload.get("outcome").and_then(Value::as_str) {
        Some("written") => {
            let status = record
                .payload
                .get("status")
                .and_then(|status| status.get("name"))
                .and_then(Value::as_str)
                .map(crate::views::one_line)
                .unwrap_or_else(|| "-".to_owned());
            format!("  home {} — written: {status}\n", read("project"))
        }
        _ => format!(
            "  home {} — failed ({}): {}\n",
            read("project"),
            read("class"),
            read("reason")
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn home() -> QualifiedId {
        "plans:ship-the-widget".parse().expect("a qualified id")
    }

    #[test]
    fn a_record_carries_the_status_only_when_written_and_the_failure_only_when_failed() {
        let written = record(
            &home(),
            Ok(onetaskgraph_plugin_api::Status {
                category: StatusCategory::Done,
                name: "done".into(),
            }),
        );
        assert_eq!(
            serde_json::to_value(&written).expect("a record"),
            serde_json::json!({
                "project": "plans:ship-the-widget", "outcome": "written",
                "status": {"category": "done", "name": "done"}, "class": null, "reason": null,
            })
        );
        let failed = record(&home(), Err((FailureClass::Refused, "no".into())));
        assert_eq!(
            serde_json::to_value(&failed).expect("a record"),
            serde_json::json!({
                "project": "plans:ship-the-widget", "outcome": "failed",
                "status": null, "class": "refused", "reason": "no",
            })
        );
    }
}
