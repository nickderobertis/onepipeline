//! A task's assignees: read as every person the item holds, written as one person or none.
//!
//! A source holds assignees only once its configuration opts in (`config.assignees: true`),
//! which its plugin declares as [`Capabilities::assignees`]. Every write naming an assignee
//! to any other source is refused here, before that source is asked anything, naming the
//! source and the setting; and every write names at most one person, which replaces whatever
//! the item holds — so a source is never handed a list it would have to merge.
//!
//! [`Capabilities::assignees`]: onetaskgraph_plugin_api::Capabilities::assignees

use onetaskgraph_plugin_api::TaskUpdate;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::{Engine, EngineError, TaskUpdated};
use crate::GlobalId;
use crate::cache::WriteResult;
use crate::resolve::ResolvedSource;

/// What `task assignee set` answers with.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct TaskAssigneeSet {
    /// The task whose assignee was set.
    pub id: GlobalId,
    /// Every person the task is assigned to as its source reads it back after the write —
    /// the one person named, or none.
    pub assignees: Vec<String>,
    /// What the task holds after the write — its version where the write's own answers
    /// determine it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<WriteResult>,
}

/// What a guarded copy found a counterpart's assignees at, and whether that met the condition.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AssigneeCondition {
    /// Every person the counterpart was assigned to, as the copy's own pre-write read held
    /// them — empty when it was unassigned.
    pub held: Vec<String>,
    /// Whether those are exactly the people the condition names.
    pub met: bool,
}

impl Engine {
    /// Assign one task to `person`, replacing every person it held — or, with `None`, to
    /// nobody — and nothing else about it.
    ///
    /// # Errors
    ///
    /// Returns [`EngineError::UnknownSource`] for a source nothing configures,
    /// [`EngineError::UpdateNotWritable`] for one with no write side and
    /// [`EngineError::NoAssignees`] for one that has not opted in to assignees — neither of
    /// which is asked — [`EngineError::NoSuchTask`] when the task is not there, and
    /// [`EngineError::SourceFailed`] when the source refuses, a person it cannot assign
    /// included.
    pub async fn set_task_assignee(
        &self,
        id: &GlobalId,
        person: Option<&str>,
    ) -> Result<TaskAssigneeSet, EngineError> {
        let source = self.updatable(&id.source)?;
        holds_assignees(source, &id.to_string())?;
        let update = TaskUpdate {
            assignees: Some(person.map(str::to_owned).into_iter().collect()),
            ..TaskUpdate::default()
        };
        let TaskUpdated { task, result, .. } = self.update_task(id, &update).await?;
        Ok(TaskAssigneeSet {
            id: id.clone(),
            assignees: task.assignees.unwrap_or_default(),
            result,
        })
    }
}

/// Refuse a write naming an assignee to `task` of `source` when that source has not opted in
/// to assignees.
pub(crate) fn holds_assignees(source: &ResolvedSource, task: &str) -> Result<(), EngineError> {
    if source.source().capabilities().assignees.is_native() {
        return Ok(());
    }
    Err(EngineError::NoAssignees {
        name: source.name().to_string(),
        kind: source.kind().to_owned(),
        task: task.to_owned(),
    })
}

/// Refuse writing `people` as `item`'s assignees when they are more than the one person a
/// write names.
pub(crate) fn one_person(item: &str, people: &[String]) -> Result<(), EngineError> {
    if people.len() <= 1 {
        return Ok(());
    }
    Err(EngineError::SeveralAssignees {
        item: item.to_owned(),
        count: people.len(),
    })
}

/// Whether two lists of assignees name the same people, whatever their order.
pub(crate) fn same_people(left: &[String], right: &[String]) -> bool {
    let left: std::collections::BTreeSet<&str> = left.iter().map(String::as_str).collect();
    let right: std::collections::BTreeSet<&str> = right.iter().map(String::as_str).collect();
    left == right
}
