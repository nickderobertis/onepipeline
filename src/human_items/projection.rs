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

/// What a task was projected as: the state it last reached, and the content it
/// was first projected with — which the store binds its projection to, so every
/// later state is projected with that same content.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Record {
    state: Projected,
    title: String,
    body: String,
    priority: super::Priority,
}

/// Each task's last projected state, by item id. A record that is not one is
/// said and read as empty: every task is then projected again, which the store's
/// intent answers without a second ticket.
fn projected(journal: &Path) -> BTreeMap<String, Record> {
    let path = projected_path(journal);
    let Ok(bytes) = std::fs::read(&path) else {
        return BTreeMap::new();
    };
    // llmlint: ignore[changed_behavior_has_e2e] no build writes a partial record: `record` replaces the file whole through `write_atomic`, so a garbled one is a hand edit; reading it as empty replays every task, the path `tasks_project_once_into_the_task_source_and_questions_never_do` drives after a record the host refused.
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
        .filter(|item| done.get(&item.id).map(|r| r.state) != Some(Projected::of(item.state)))
        .cloned()
        .collect()
}

/// The content `item` is projected with: what it was first projected as — the
/// record's copy, or, where that is lost, the journal's own record of the item as
/// it stood when its tracker was bound — else its own.
fn content(journal: &Path, item: &HumanItem) -> (String, String, super::Priority) {
    if let Some(r) = projected(journal).remove(&item.id) {
        return (r.title, r.body, r.priority);
    }
    let bound = super::live::first_tracked(journal, &item.id);
    let first = bound.as_ref().unwrap_or(item);
    (first.title.clone(), first.body.clone(), first.priority)
}

/// Project one task into `source`, as the store configured from `dir` places it.
/// Answers its tracker binding.
pub(crate) fn project(
    dir: &Path,
    journal: &Path,
    source: &str,
    item: &HumanItem,
) -> Result<HumanItemTracker, String> {
    let resolution = item.resolution.as_ref();
    let (title, body, priority) = content(journal, item);
    use onetaskgraph_core::task_projection as wire;
    let refused =
        |what: &str, why: String| format!("the task's {what} is not one the store accepts: {why}");
    let request = wire::HumanTaskProjection {
        schema_version: wire::ProjectionVersion,
        item_id: wire::ExternalItemId::new(item.id.clone()).map_err(|e| refused("id", e))?,
        owner: wire::ProjectionOwner {
            session: wire::SessionId::new(item.owner.session.clone())
                .map_err(|e| refused("owner session", e))?,
            run: item
                .owner
                .run
                .clone()
                .map(wire::RunId::new)
                .transpose()
                .map_err(|e| refused("owner run", e))?,
            plan: item
                .owner
                .plan
                .clone()
                .map(wire::PlanId::new)
                .transpose()
                .map_err(|e| refused("owner plan", e))?,
        },
        title: wire::ProjectionTitle::new(title).map_err(|e| refused("title", e))?,
        body,
        priority,
        repositories: Vec::new(),
        project: item
            .owner
            .plan
            .clone()
            .map(wire::ProjectionProjectId::new)
            .transpose()
            .map_err(|e| refused("plan", e))?,
        source: onetaskgraph_plugin_api::SourceName::new(source)
            .map_err(|e| refused("task_source", e.to_string()))?,
        state: match Projected::of(item.state) {
            Projected::Open => wire::ProjectionState::Open,
            Projected::Completed => wire::ProjectionState::Completed,
            Projected::Dismissed => wire::ProjectionState::Dismissed,
        },
        reason: if item.state == HumanItemState::Dismissed {
            resolution.and_then(|r| r.reason.clone())
        } else {
            None
        },
    };
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
    let (title, body, priority) = content(journal, item);
    let mut done = projected(journal);
    done.insert(
        item.id.clone(),
        Record {
            state: Projected::of(item.state),
            title,
            body,
            priority,
        },
    );
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
