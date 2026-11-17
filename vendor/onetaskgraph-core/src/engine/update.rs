//! One targeted update of one existing task: every field the caller names, and nothing else.
//!
//! One call to one source. The engine reads nothing of its own around it: the source's answer
//! carries the task as it reads back, the fields it wrote and the `delivers` it held before,
//! and every figure this reports is read off that answer. On a hosted backend that is the
//! difference between one read of the item and two, on every update — which is the whole of
//! why this exists beside a copy and the narrow verbs.
//!
//! `delivers` is kept exactly as `task status set` keeps it: whenever `status` or `delivers` is
//! named, every task the list names now, and every task it dropped, is re-evaluated.

use std::collections::BTreeSet;

use onetaskgraph_plugin_api::{
    DependencyEdge, DependencyEndpoint, ItemKind, NativeId, SourceName, Task, TaskUpdate,
    UpdatedField,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::boundary::{Exposure, Held};
use super::copy::{Spent, readings, spent_between};
use super::delivery::{Delivered, qualified_task, source_failed, targets};
use super::narrow::holds_priority;
use super::{Engine, EngineError};
use crate::GlobalId;
use crate::cache::{WriteFallback, WriteResult};

/// What `task update` answers with.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct TaskUpdated {
    /// The task updated.
    pub id: GlobalId,
    /// The task as its source reads it back, with every entry of its `delivers` and
    /// `delivered_by` qualified.
    pub task: Task,
    /// The fields the source actually wrote — empty when nothing differed.
    pub written: BTreeSet<UpdatedField>,
    /// As `set_task_status` reports it; re-evaluated whenever `status` or `delivers` was
    /// named, empty otherwise.
    pub delivered: Vec<Delivered>,
    /// What the source's meter says this call spent, as `CopyReport::spent` — and absent,
    /// never zero, when the source does not meter its own requests.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spent: Option<Spent>,
    /// What each mutation an expected write sent came to — present when the write was made
    /// with `--expect` on a source that reports its writes' parts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<WriteResult>,
    /// That an expected write read its target first, and why — absent when it sent only its
    /// mutations, and for a write made without `--expect`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fallback: Option<WriteFallback>,
    /// The public projection a written status category was mirrored to — absent when the
    /// update wrote no status, and for a task with no projection.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub projection: Option<GlobalId>,
    /// What `--when-priority` found the task's priority at, and whether the update was written
    /// or held — absent for an update given no priority guard.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub guard: Option<super::PriorityGuard>,
}

impl Engine {
    /// Apply a targeted update to one task — every field `update` names, and nothing else —
    /// then keep every task it delivers in step with it when `status` or `delivers` was named.
    ///
    /// The source is asked once, and nothing else of it is read: a field already holding the
    /// requested value is sent no write, and an update in which nothing differs writes nothing
    /// at all — an update naming no field included, which answers the task as its source reads
    /// it and re-evaluates nothing; the CLI refuses that one as a usage error before it gets
    /// here, a library caller is answered. `update.depends_on` may name its far ends qualified; one in this task's own
    /// source reaches the source as its native id, and each edge's `from` is this task.
    ///
    /// # Errors
    ///
    /// Returns [`EngineError::UnknownSource`] for a source nothing configures,
    /// [`EngineError::UpdateNotWritable`] for one with no write side,
    /// [`EngineError::NoPriority`] for a priority other than `none` to a source that holds
    /// none — neither of which is asked — [`EngineError::NoSuchTask`] when the task is not
    /// there, and [`EngineError::SourceFailed`] carrying the source's own [`SourceError`] when
    /// it refuses or fails, so its class and its `retry_after_seconds` are what a caller
    /// reads. An update both setting and removing one metadata key is refused that way too,
    /// before anything is sent, in the words the source itself refuses it with. A delivered
    /// task that cannot be kept in step is not an error: it is reported `failed` beside the
    /// update that landed.
    ///
    /// [`SourceError`]: onetaskgraph_plugin_api::SourceError
    pub async fn update_task(
        &self,
        id: &GlobalId,
        update: &TaskUpdate,
    ) -> Result<TaskUpdated, EngineError> {
        let source = self.updatable(&id.source)?;
        let update = self.prepared_update(source, id, update).await?;
        let before = readings(&[source]).await;
        let outcome = source
            .source()
            .update_task(&id.native, &update)
            .await
            .map_err(|error| source_failed(source, error))?
            .ok_or_else(|| EngineError::NoSuchTask { id: id.to_string() })?;
        self.updated(source, id, &update, outcome, &before).await
    }

    /// The source `name` names, refused as [`EngineError::UpdateNotWritable`] when it has no
    /// write side.
    pub(super) fn updatable(
        &self,
        name: &SourceName,
    ) -> Result<&super::ResolvedSource, EngineError> {
        let source = self.built(name)?;
        if !source.source().writes().is_supported() {
            return Err(EngineError::UpdateNotWritable {
                name: source.name().to_string(),
                kind: source.kind().to_owned(),
            });
        }
        Ok(source)
    }

    /// `update` of the task `id` as its source is to be sent it: refused when it contradicts
    /// itself, names a priority the source holds none of or a private reference it may not
    /// carry, and with every far end named as its source names it and every value the write
    /// policy rewrites rewritten — everything [`update_task`](Self::update_task) checks before
    /// it sends anything.
    pub(super) async fn prepared_update(
        &self,
        source: &super::ResolvedSource,
        id: &GlobalId,
        update: &TaskUpdate,
    ) -> Result<TaskUpdate, EngineError> {
        update
            .consistent()
            .map_err(|error| source_failed(source, error))?;
        if let Some(priority) = update.priority {
            holds_priority(source, &id.to_string(), priority)?;
        }
        if let Some(people) = &update.assignees {
            super::assignees::holds_assignees(source, &id.to_string())?;
            super::assignees::one_person(&id.to_string(), people)?;
        }
        let depends_on = match &update.depends_on {
            Some(edges) => Some(
                self.publicly_named(
                    &id.source,
                    &id.to_string(),
                    near_edges(edges, &id.native, &id.source),
                )
                .await?,
            ),
            None => None,
        };
        let update = TaskUpdate {
            depends_on,
            ..update.clone()
        };
        // Each reference as the qualified id it names: a native one names a task of this source.
        let qualified = |named: String| {
            named
                .parse::<GlobalId>()
                .map_err(|_| EngineError::ReferenceUnclassified {
                    item: id.to_string(),
                    reference: named.clone(),
                    destination: id.source.to_string(),
                    why: "it is not a qualified id".to_owned(),
                })
        };
        let mut references = Vec::new();
        // An opaque far end conveys nothing, so it names nothing this write has to refuse.
        for edge in update
            .depends_on
            .iter()
            .flatten()
            .filter(|edge| edge.to.designator().is_none())
        {
            references.push((
                if edge.to.is_qualified() {
                    qualified(edge.to.id().to_owned())?
                } else {
                    GlobalId::new(id.source.clone(), NativeId(edge.to.id().to_owned()))
                },
                edge.to.kind,
            ));
        }
        for entry in update.delivers.iter().flatten() {
            references.push((
                qualified(entry.in_source(&id.source).as_str().to_owned())?,
                onetaskgraph_plugin_api::ItemKind::Task,
            ));
        }
        self.refuse_private_references(&id.source, &id.to_string(), &references)
            .await?;
        let rewrite = self
            .admit_existing(
                source,
                &id.to_string(),
                &id.native,
                Held::Task,
                &Exposure::of_update(&update),
            )
            .await?;
        // The repositories it is to name classify it as a read would after the write: a list
        // naming a private one is refused onto a source that is not private, as a create of it
        // would be, before anything is sent.
        if let Some(repositories) = update.repositories.as_ref().filter(|list| !list.is_empty())
            && self.boundary_active()
        {
            self.admit_known(
                source,
                &id.to_string(),
                &id.native,
                onetaskgraph_plugin_api::Classification::Public,
                repositories,
                &Exposure::default(),
            )
            .await?;
        }
        let mut update = update;
        rewrite.update(&mut update);
        Ok(update)
    }

    /// What `update` of the task `id`, which its source answered with `outcome`, came to: every
    /// task it delivers kept in step when it named a status or `delivers`, its status mirrored
    /// to its public projection when it wrote one, and what the call spent since `before`.
    pub(super) async fn updated(
        &self,
        source: &super::ResolvedSource,
        id: &GlobalId,
        update: &TaskUpdate,
        outcome: onetaskgraph_plugin_api::TaskUpdateOutcome,
        before: &[Option<onetaskgraph_plugin_api::Metering>],
    ) -> Result<TaskUpdated, EngineError> {
        let delivered = if update.status.is_some() || update.delivers.is_some() {
            let now = targets(&outcome.task.delivers, &id.source);
            let dropped = targets(&outcome.delivers_before, &id.source);
            self.deliver(
                id,
                outcome.task.classification,
                outcome.task.status.category,
                &now,
                &dropped,
            )
            .await
        } else {
            Vec::new()
        };
        let spent = spent_between(before, &readings(&[source]).await);
        let (result, fallback, _) = self.expected_parts(id);
        let result = self
            .verb_result(source, id, Some(&outcome.task), result)
            .await;
        let projection = if outcome.written.contains(&UpdatedField::Status) {
            self.mirror_status(id, &outcome.task, outcome.task.status.category)
                .await?
        } else {
            None
        };
        Ok(TaskUpdated {
            id: id.clone(),
            task: qualified_task(id.clone(), outcome.task).item,
            written: outcome.written,
            delivered,
            spent,
            result,
            fallback,
            projection,
            guard: None,
        })
    }
}

/// `edges` as the task `near` of `source` holds them: each starting at `near`, and each far
/// end in `source` itself named by its native id, which is how a source names one of its own.
fn near_edges(
    edges: &[DependencyEdge],
    near: &NativeId,
    source: &SourceName,
) -> Vec<DependencyEdge> {
    edges
        .iter()
        .map(|edge| {
            let qualified = edge
                .to
                .is_qualified()
                .then(|| edge.to.id().parse::<GlobalId>());
            let to = match qualified {
                Some(Ok(far)) if &far.source == source => {
                    DependencyEndpoint::from_native(far.native, edge.to.kind)
                }
                _ => edge.to.clone(),
            };
            DependencyEdge {
                from: DependencyEndpoint::from_native(near.clone(), ItemKind::Task),
                to,
                kind: edge.kind,
            }
        })
        .collect()
}
