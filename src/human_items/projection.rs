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

/// The lifecycle a task is projected at: the store's own three.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
enum Projected {
    Open,
    Completed,
    Dismissed,
}

impl Projected {
    fn of(state: HumanItemState) -> Self {
        match state {
            HumanItemState::Open => Self::Open,
            HumanItemState::Completed => Self::Completed,
            // A question never reaches here; a dismissed task is dismissed.
            HumanItemState::Answered | HumanItemState::Dismissed => Self::Dismissed,
        }
    }
}

fn projected_path(journal: &Path) -> PathBuf {
    journal.with_file_name("projected.json")
}

/// Each task's last projected state, by item id. A record that is not one is
/// said and read as empty: every task is then projected again, which the store's
/// intent answers without a second ticket.
fn projected(journal: &Path) -> BTreeMap<String, Projected> {
    let path = projected_path(journal);
    let Ok(bytes) = std::fs::read(&path) else {
        return BTreeMap::new();
    };
    serde_json::from_slice(&bytes).unwrap_or_else(|why| {
        eprintln!(
            "onepipeline: {} is not a projection record ({why}); every task is projected again",
            path.display()
        );
        BTreeMap::new()
    })
}

/// The tasks whose state is ahead of what has been projected.
pub(crate) fn pending<'a>(
    journal: &Path,
    items: impl Iterator<Item = &'a HumanItem>,
) -> Vec<HumanItem> {
    let done = projected(journal);
    items
        .filter(|item| item.kind == HumanItemKind::Task)
        .filter(|item| done.get(&item.id) != Some(&Projected::of(item.state)))
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
            "state": Projected::of(item.state),
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
    done.insert(item.id.clone(), Projected::of(item.state));
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
