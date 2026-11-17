//! A task's priority and a task's content, each set on its own.
//!
//! Both verbs are one call to one source, with no compare-and-set: what lands is what was
//! asked for, whatever the task held a moment before. Neither moves the task's status, so no
//! delivered task is re-evaluated after one of these writes.
//!
//! A priority is refused here, before the source is asked, when the source declares it holds
//! none: a source that cannot hold a priority has nowhere to put one, and asking it anyway
//! would leave the refusal to each plugin's own wording — or, for a plugin written before
//! priorities existed, to a write that drops it in silence.

use onetaskgraph_plugin_api::{MetadataRecord, Priority};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::boundary::{Exposure, Held};
use super::delivery::source_failed;
use super::{Engine, EngineError, PriorityCondition, Spent};
use crate::GlobalId;
use crate::cache::{WriteFallback, WriteResult};
use crate::resolve::ResolvedSource;

/// What `task priority set` answers with.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct TaskPrioritySet {
    /// The task whose priority was set.
    pub id: GlobalId,
    /// Its priority as its source reads it back after the write.
    pub priority: Priority,
    /// What each mutation an expected write sent came to — present when the write was made
    /// with `--expect` on a source that reports its writes' parts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<WriteResult>,
    /// That an expected write read its target first, and why — absent when it sent only its
    /// mutations, and for a write made without `--expect`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fallback: Option<WriteFallback>,
    /// What an expected write spent at the sources that meter their own requests, its
    /// fallback read included — absent for a write made without `--expect`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spent: Option<Spent>,
    /// What a guard by `--when-priority` came to — its `outcome` and its `priority_condition`
    /// beside the other members — absent for a write made without the guard.
    #[serde(flatten, default, skip_serializing_if = "Option::is_none")]
    pub guard: Option<PriorityGuard>,
}

/// What a write guarded by a condition on the item's own value came to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum GuardOutcome {
    /// The item held a value the guard names, and the write was made.
    Written,
    /// The item held a value the guard does not name, and nothing was written.
    Held,
}

/// What a write guarded by the priority its task holds came to: whether it was `written` or
/// `held`, and the condition that decided it. Made only from the condition — read back off the
/// wire too, whatever `outcome` it was written with — so the outcome is always the one its
/// `met` says, and never stands without it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(try_from = "PriorityGuardWire")]
pub struct PriorityGuard {
    /// `written` when the task held a priority the guard names, and `held` when it did not.
    outcome: GuardOutcome,
    /// What the guard found the task's priority at before the write, and whether that is one it
    /// names — the shape `task copy --when-destination-priority` reports.
    priority_condition: PriorityCondition,
}

impl PriorityGuard {
    /// The guard `condition` came to.
    #[must_use]
    pub const fn of(condition: PriorityCondition) -> Self {
        Self {
            outcome: if condition.met {
                GuardOutcome::Written
            } else {
                GuardOutcome::Held
            },
            priority_condition: condition,
        }
    }

    /// Whether the write was made.
    #[must_use]
    pub const fn outcome(&self) -> GuardOutcome {
        self.outcome
    }

    /// The condition that decided it.
    #[must_use]
    pub const fn condition(&self) -> PriorityCondition {
        self.priority_condition
    }
}

/// [`PriorityGuard`] as it reads off the wire: its condition decides its outcome, and with no
/// condition there is no guard. Each member is optional here because the members around them
/// are: an answer written without a guard carries neither.
#[derive(Deserialize, JsonSchema)]
#[schemars(rename = "PriorityGuard")]
struct PriorityGuardWire {
    /// `written` when the task held a priority the guard names, and `held` when it did not —
    /// absent for a write made without the guard.
    #[allow(
        dead_code,
        reason = "read for the schema; the condition decides the outcome"
    )]
    #[serde(default)]
    outcome: Option<GuardOutcome>,
    /// What the guard found the task's priority at before the write, and whether that is one it
    /// names — the shape `task copy --when-destination-priority` reports — absent for a write
    /// made without the guard.
    #[serde(default)]
    priority_condition: Option<PriorityCondition>,
}

impl TryFrom<PriorityGuardWire> for PriorityGuard {
    type Error = &'static str;

    fn try_from(wire: PriorityGuardWire) -> Result<Self, Self::Error> {
        wire.priority_condition
            .map(Self::of)
            .ok_or("no guard: no priority condition")
    }
}

/// What `task content set` answers with.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct TaskContentSet {
    /// The task whose content was replaced.
    pub id: GlobalId,
    /// The content the task holds after the write: what its source's backend stores, as the
    /// write's own answer reported it, where the source reports one — a backend may normalize
    /// the spelling of the Markdown it is written while keeping its rendered meaning — and
    /// otherwise exactly the bytes sent. An answer, never stored.
    pub content: String,
    /// What each mutation an expected write sent came to — present when the write was made
    /// with `--expect` on a source that reports its writes' parts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<WriteResult>,
    /// That an expected write read its target first, and why — absent when it sent only its
    /// mutations, and for a write made without `--expect`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fallback: Option<WriteFallback>,
    /// What an expected write spent at the sources that meter their own requests, its
    /// fallback read included — absent for a write made without `--expect`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spent: Option<Spent>,
}

impl Engine {
    /// Set one task's priority, and nothing else about it.
    ///
    /// # Errors
    ///
    /// Returns [`EngineError::UnknownSource`] for a source nothing configures,
    /// [`EngineError::PriorityNotWritable`] for one with no write side,
    /// [`EngineError::NoPriority`] for one declaring it holds no priority — neither of which
    /// is asked — [`EngineError::NoSuchTask`] when the task is not there, and
    /// [`EngineError::SourceFailed`] when the source refuses, a board with no option for the
    /// priority included.
    pub async fn set_task_priority(
        &self,
        id: &GlobalId,
        priority: Priority,
    ) -> Result<TaskPrioritySet, EngineError> {
        let source = self.built(&id.source)?;
        if !source.source().writes().is_supported() {
            return Err(EngineError::PriorityNotWritable {
                name: source.name().to_string(),
                kind: source.kind().to_owned(),
            });
        }
        // Every value, `none` included: a source that holds no priority has none to clear,
        // and the verb asks it for a write it has no field for.
        if !source.source().capabilities().priority.is_native() {
            return Err(no_priority(source, &id.to_string(), priority));
        }
        self.admit_existing(
            source,
            &id.to_string(),
            &id.native,
            Held::Task,
            &Exposure::metadata([priority.as_str().to_owned()]),
        )
        .await?;
        let read = source
            .source()
            .set_task_priority(&id.native, priority)
            .await
            .map_err(|error| source_failed(source, error))?
            .ok_or_else(|| EngineError::NoSuchTask { id: id.to_string() })?;
        let (result, fallback, spent) = self.expected_parts(id);
        let mut result = self.verb_result(source, id, None, result).await;
        // The priority this verb's write answered with is what the task holds, whatever else
        // the source could say.
        if let Some(result) = result.as_mut() {
            result.priority = Some(read);
        }
        Ok(TaskPrioritySet {
            id: id.clone(),
            priority: read,
            result,
            fallback,
            spent,
            guard: None,
        })
    }

    /// [`set_task_priority`](Self::set_task_priority), only while the task holds one of the
    /// priorities `when` names — [`Priority::None`] matching a task with none: the task is read
    /// first, and one at any other priority is answered [`GuardOutcome::Held`] with the priority
    /// it holds and nothing written. The semantics of `task copy --when-destination-priority`,
    /// on one task. An empty `when` sets no guard.
    ///
    /// The read and the write are two calls; beside [`Engine::held_write`] a change landing
    /// between them is refused as the version having moved.
    ///
    /// # Errors
    ///
    /// As [`set_task_priority`](Self::set_task_priority).
    pub async fn set_task_priority_when(
        &self,
        id: &GlobalId,
        priority: Priority,
        when: &[Priority],
    ) -> Result<TaskPrioritySet, EngineError> {
        if when.is_empty() {
            return self.set_task_priority(id, priority).await;
        }
        let source = self.built(&id.source)?;
        if !source.source().writes().is_supported() {
            return Err(EngineError::PriorityNotWritable {
                name: source.name().to_string(),
                kind: source.kind().to_owned(),
            });
        }
        if !source.source().capabilities().priority.is_native() {
            return Err(no_priority(source, &id.to_string(), priority));
        }
        let task = source
            .source()
            .get_task(&id.native)
            .await
            .map_err(|error| source_failed(source, error))?
            .ok_or_else(|| EngineError::NoSuchTask { id: id.to_string() })?;
        let condition = PriorityCondition {
            held: task.priority,
            met: when.contains(&task.priority),
        };
        if !condition.met {
            return Ok(TaskPrioritySet {
                id: id.clone(),
                priority: task.priority,
                result: Some(WriteResult::of_task(&task, task.version.clone())),
                fallback: None,
                spent: None,
                guard: Some(PriorityGuard::of(condition)),
            });
        }
        let mut set = self.set_task_priority(id, priority).await?;
        set.guard = Some(PriorityGuard::of(condition));
        Ok(set)
    }

    /// Replace one task's content with `content`, byte for byte, and nothing else about it.
    ///
    /// Answers with the content the task then holds, taken from the write's own answer where
    /// the source reports what its backend stores, with no read of the task after the write.
    ///
    /// # Errors
    ///
    /// Returns [`EngineError::UnknownSource`] for a source nothing configures,
    /// [`EngineError::ContentNotWritable`] for one with no write side, which is not asked,
    /// [`EngineError::NoSuchTask`] when the task is not there, and
    /// [`EngineError::SourceFailed`] when the source refuses.
    pub async fn set_task_content(
        &self,
        id: &GlobalId,
        content: &str,
    ) -> Result<TaskContentSet, EngineError> {
        let source = self.built(&id.source)?;
        if !source.source().writes().is_supported() {
            return Err(EngineError::ContentNotWritable {
                name: source.name().to_string(),
                kind: source.kind().to_owned(),
            });
        }
        let rewrite = self
            .admit_existing(
                source,
                &id.to_string(),
                &id.native,
                Held::Task,
                &Exposure::text(content),
            )
            .await?;
        let content = &rewrite.text(content);
        super::stored::forget(source, MetadataRecord::Task, &id.native);
        source
            .source()
            .set_task_content(&id.native, content)
            .await
            .map_err(|error| source_failed(source, error))?
            .ok_or_else(|| EngineError::NoSuchTask { id: id.to_string() })?;
        let held = source
            .source()
            .stored_content(MetadataRecord::Task, &id.native)
            .unwrap_or_else(|| content.to_owned());
        let (result, fallback, spent) = self.expected_parts(id);
        let result = self.verb_result(source, id, None, result).await;
        Ok(TaskContentSet {
            id: id.clone(),
            content: held,
            result,
            fallback,
            spent,
        })
    }
}

/// Refuse `priority` for `task` when `source` declares it holds no priority at all.
///
/// `none` always passes: it is what such a source already reports for every task, so a write
/// carrying it asks the source for nothing it cannot hold. That is what lets a copy carrying
/// `none` into such a source write exactly as it did before priorities existed.
pub(super) fn holds_priority(
    source: &ResolvedSource,
    task: &str,
    priority: Priority,
) -> Result<(), EngineError> {
    if priority == Priority::None || source.source().capabilities().priority.is_native() {
        return Ok(());
    }
    Err(no_priority(source, task, priority))
}

/// The refusal of `priority` for `task` at a source that holds none.
fn no_priority(source: &ResolvedSource, task: &str, priority: Priority) -> EngineError {
    EngineError::NoPriority {
        name: source.name().to_string(),
        kind: source.kind().to_owned(),
        task: task.to_owned(),
        priority,
    }
}
