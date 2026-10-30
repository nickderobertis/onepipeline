//! Projection of human tasks into the configured `task_source`, through the
//! linked store's own `Engine::project_human`.
//!
//! **Questions are never projected**; they live only in the journal and on the
//! owner's queue. A task is projected after its event is durable and outside the
//! journal's lock, so a slow tracker holds no other writer. What has been
//! projected is kept beside the journal as each task's last projected state; a
//! task whose state is ahead of it — a process that died between its journal
//! record and its projection, or a tracker that refused — is projected again by
//! the next write, and the store's durable intent, keyed by the item's stable id,
//! is what makes that replay create nothing twice. A refusal is said and leaves
//! the local item, and its hold, exactly as they are.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use super::{HumanItem, HumanItemKind, HumanItemState, HumanItemTracker};

fn word(state: HumanItemState) -> &'static str {
    match state {
        HumanItemState::Open => "open",
        HumanItemState::Completed => "completed",
        // A question never reaches here; a dismissed task is dismissed.
        HumanItemState::Answered | HumanItemState::Dismissed => "dismissed",
    }
}

fn projected_path(journal: &Path) -> PathBuf {
    journal.with_file_name("projected.json")
}

/// Each task's last projected state, by item id.
fn projected(journal: &Path) -> BTreeMap<String, String> {
    std::fs::read(projected_path(journal))
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

/// The tasks whose state is ahead of what has been projected.
pub(crate) fn pending<'a>(
    journal: &Path,
    items: impl Iterator<Item = &'a HumanItem>,
) -> Vec<HumanItem> {
    let done = projected(journal);
    items
        .filter(|item| item.kind == HumanItemKind::Task)
        .filter(|item| done.get(&item.id).map(String::as_str) != Some(word(item.state)))
        .cloned()
        .collect()
}

/// Project one task into `source`, as the store configured from `dir` places it.
/// Answers its tracker binding.
pub(crate) fn project(
    dir: &Path,
    source: &str,
    item: &HumanItem,
) -> Result<HumanItemTracker, String> {
    let resolution = item.resolution.as_ref();
    let request: onetaskgraph_core::task_projection::HumanTaskProjection =
        serde_json::from_value(serde_json::json!({
            "schema_version": 1,
            "item_id": item.id,
            "owner": item.owner,
            "title": item.title,
            "body": item.body,
            "priority": item.priority,
            "repositories": [],
            "project": item.owner.plan,
            "source": source,
            "state": word(item.state),
            "reason": if item.state == HumanItemState::Dismissed {
                resolution.and_then(|r| r.reason.clone())
            } else {
                None
            },
        }))
        .map_err(|e| format!("the projection request is not one the store accepts: {e}"))?;
    let built = crate::taskgraph::Store::at(dir)
        .engine(&Default::default())
        .map_err(|e| e.to_string())?;
    let runtime = crate::taskgraph::runtime().map_err(|e| e.to_string())?;
    let bound = runtime
        .block_on(
            built
                .engine
                .project_human(&request, &crate::sys::hostname()),
        )
        .map_err(|e| e.to_string())?;
    Ok(HumanItemTracker {
        task: bound.task().to_string(),
        url: bound.url().to_string(),
    })
}

/// Record that `item` has been projected at its current state.
pub(crate) fn record(journal: &Path, item: &HumanItem) {
    let mut done = projected(journal);
    done.insert(item.id.clone(), word(item.state).to_owned());
    if let Err(why) = crate::ledger::write_atomic(
        &projected_path(journal),
        &serde_json::to_vec(&done).unwrap_or_default(),
        crate::ledger::Durability::Record,
    ) {
        eprintln!(
            "onepipeline: {} was projected and that could not be recorded ({why}); the next \
             write projects it again, which the store answers without a second task",
            item.id
        );
    }
}
