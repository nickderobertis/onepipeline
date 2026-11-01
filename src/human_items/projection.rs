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
// llmlint: ignore-block[invalid_states_unrepresentable] a private record of what the store accepted: its title, body and priority are copied from a HumanItem whose decoder refused a blank title and `none`, and the store's own ProjectionTitle refused them again before anything was recorded.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Record {
    state: Projected,
    title: String,
    body: String,
    priority: super::Priority,
    /// The repositories it was first projected with: a plan-owned task's plan's, which the
    /// store routes and classifies it by. Absent for a task no plan owns, and in a record an
    /// earlier build wrote, which projected none.
    // llmlint: ignore[changed_behavior_has_e2e] a record an earlier build wrote is one the store accepted with no repositories — a planless task, or one whose task source was its plan's own — so reading it as none replays exactly what was accepted; `a_record_an_earlier_build_wrote_reads_as_projected_with_no_repositories` holds the decoding, and the replay journeys drive the reread.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    repositories: Vec<onetaskgraph_plugin_api::Repository>,
}
// llmlint: ignore-end[invalid_states_unrepresentable]

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

/// The tasks whose projection is owed.
pub(crate) fn pending<'a>(
    journal: &Path,
    items: impl Iterator<Item = &'a HumanItem>,
) -> Vec<HumanItem> {
    let done = projected(journal);
    items
        .filter(|item| item.kind == HumanItemKind::Task)
        // Ahead of what was projected — or projected open with no tracker bound on
        // the item, whose binding the store answers again rather than filing anew.
        .filter(|item| {
            done.get(&item.id).map(|r| r.state) != Some(Projected::of(item.state))
                || (item.state == HumanItemState::Open && item.tracker.is_none())
        })
        .cloned()
        .collect()
}

/// The content `item` is projected with: what it was first projected as — the
/// record's copy — else the item as the journal recorded it when its tracker was bound,
/// or as it stands, with its plan's reach added when a plan owns it.
fn content(dir: &Path, journal: &Path, item: &HumanItem) -> Result<Content, String> {
    if let Some(r) = projected(journal).remove(&item.id) {
        return Ok(Content {
            title: title(r.title)?,
            body: r.body,
            priority: r.priority,
            repositories: r.repositories,
        });
    }
    let bound = super::live::first_tracked(journal, &item.id);
    let first = bound.as_ref().unwrap_or(item);
    let mut content = Content {
        title: title(first.title.clone())?,
        body: first.body.clone(),
        priority: first.priority,
        repositories: Vec::new(),
    };
    if let Some(plan) = &item.owner.plan {
        let reach = plan_reach(dir, plan)?;
        content.body = with_plan_link(&content.body, plan, reach.url.as_deref());
        content.repositories = reach.repositories;
    }
    Ok(content)
}

/// What a projected task carries, fixed at its first projection: the store binds its
/// intent to it, so every later state is projected with this same content.
struct Content {
    title: onetaskgraph_core::task_projection::ProjectionTitle,
    body: String,
    priority: super::Priority,
    repositories: Vec<onetaskgraph_plugin_api::Repository>,
}

/// `spelled` as the store's own title, refused as the store refuses it.
fn title(spelled: String) -> Result<onetaskgraph_core::task_projection::ProjectionTitle, String> {
    onetaskgraph_core::task_projection::ProjectionTitle::new(spelled)
        .map_err(|e| format!("the task's title is not one the store accepts: {e}"))
}

/// The plan `plan` names, read where it is held. Refused rather than projected without
/// it: a task projected with no repositories is classified public, so a private plan's
/// task would land on a public board.
fn plan_reach(dir: &Path, plan: &str) -> Result<crate::taskgraph::PlanReach, String> {
    let id: crate::taskgraph::QualifiedId = plan
        .parse()
        .map_err(|e: crate::error::Error| format!("its plan {plan} is not one to read: {e}"))?;
    crate::taskgraph::Store::at(dir)
        .plan_reach(&id)
        .map_err(|e| format!("its plan {plan} could not be read: {e}"))
}

// llmlint: ignore[changed_behavior_has_e2e] the qualified-id fallback is for a source that answers a project with no address, which no tracker source a human board is does — GitHub and Linear both answer one, which `human_board_projection` drives end to end; the fallback's spelling is held by `a_plan_link_follows_the_body_and_names_the_plan_by_its_address_or_its_id`.
/// `body` with a closing line linking the plan that owns it: its address where the plan's
/// source gives one, else its qualified id.
fn with_plan_link(body: &str, plan: &str, url: Option<&str>) -> String {
    let link = url.map_or_else(|| format!("`{plan}`"), |url| format!("[{plan}]({url})"));
    let body = body.trim_end();
    if body.is_empty() {
        format!("Plan: {link}")
    } else {
        format!("{body}\n\nPlan: {link}")
    }
}

/// A task's tracker binding, and the record that it was projected.
pub(crate) struct Bound {
    pub(crate) tracker: HumanItemTracker,
    record: Record,
}

/// Project one task into `source`, as the store configured from `dir` places it.
/// Answers its tracker binding.
pub(crate) fn project(
    dir: &Path,
    journal: &Path,
    source: &str,
    item: &HumanItem,
) -> Result<Bound, String> {
    let resolution = item.resolution.as_ref();
    let content = content(dir, journal, item)?;
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
        title: content.title.clone(),
        body: content.body.clone(),
        priority: content.priority,
        repositories: content.repositories.clone(),
        // Placed by the task source's own configured project wherever it routes: the
        // plan's project is in the plans source, which a human task never lands in.
        project: None,
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
    Ok(Bound {
        tracker: HumanItemTracker {
            task: bound.task().to_string(),
            url: bound.url().to_string(),
        },
        record: Record {
            state: Projected::of(item.state),
            title: content.title.as_str().to_owned(),
            body: content.body,
            priority: content.priority,
            repositories: content.repositories,
        },
    })
}

/// Record that `item` has been projected as `bound` says.
pub(crate) fn record(journal: &Path, item: &HumanItem, bound: Bound) {
    let mut done = projected(journal);
    done.insert(item.id.clone(), bound.record);
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

#[cfg(test)]
mod tests {
    use super::with_plan_link;

    #[test]
    fn a_plan_link_follows_the_body_and_names_the_plan_by_its_address_or_its_id() {
        assert_eq!(
            with_plan_link("Approve it.\n", "plans:I_1", Some("https://example.test/1")),
            "Approve it.\n\nPlan: [plans:I_1](https://example.test/1)"
        );
        assert_eq!(
            with_plan_link("Approve it.", "local:plan", None),
            "Approve it.\n\nPlan: `local:plan`"
        );
        assert_eq!(
            with_plan_link("  \n", "local:plan", None),
            "Plan: `local:plan`"
        );
    }

    #[test]
    fn a_record_an_earlier_build_wrote_reads_as_projected_with_no_repositories() {
        let earlier = r#"{"state":"open","title":"Decide","body":"Why.","priority":"high"}"#;
        let record: super::Record = serde_json::from_str(earlier).unwrap();
        assert!(record.repositories.is_empty());
        assert_eq!(serde_json::to_string(&record).unwrap(), earlier);
    }
}
