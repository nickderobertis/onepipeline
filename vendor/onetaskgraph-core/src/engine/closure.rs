//! A card brought in line with its issue's closure: `task status reconcile`, on demand over one
//! source, and `task status complete`, by the issue references a landing closed, over every
//! configured source that can hold an item for one.
//!
//! Both are for a source whose status is a card on a board beside an issue that closes on its
//! own. A source whose status *is* its closure — a Linear workflow state, a file's own
//! `status:`, a task held in memory — declares neither capability, is sent no request, and is
//! reported as having nothing to do.

use onetaskgraph_plugin_api::{
    Completion, HeldTask, IssueHolding, IssueRef, ItemKind, SourceError, SourceName, Status,
    StatusCategory, StatusContradiction,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::boundary::Exposure;
use super::delivery::{source_failed, targets};
use super::{ConfiguredSource, Delivered, Engine, EngineError};
use crate::resolve::ResolvedSource;
use crate::{Failure, GlobalId};

/// What `task status reconcile` answers with.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct StatusesReconciled {
    /// One entry per item whose card contradicted its closure, in the order the source read
    /// them. Empty for a source with nothing out of line, and for one that does not declare
    /// `reconcile_status`.
    pub items: Vec<StatusReconciled>,
}

/// One item `task status reconcile` brought in line, or would, or could not.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct StatusReconciled {
    /// The item, by its qualified id.
    pub source: GlobalId,
    /// What the item is: `task`, or `project` for a project's own card — a plan's home.
    pub kind: ItemKind,
    /// The option its card sat at, or `null` for a card with none.
    pub from: Option<String>,
    /// The status its closure implies, which its card was moved to.
    pub to: Status,
    /// What became of it: its `action`, and why when it failed.
    #[serde(flatten)]
    pub outcome: ReconcileOutcome,
}

/// What reconciling one item came to.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "action", rename_all = "kebab-case")]
pub enum ReconcileOutcome {
    /// Its card was moved.
    Updated,
    /// Its card would be moved; a dry run wrote nothing.
    WouldUpdate,
    /// Its card could not be moved.
    Failed {
        /// Why — the failure object a failure document carries.
        error: Failure,
    },
}

impl StatusesReconciled {
    /// Whether any item could not be brought in line, which is what exits `4`.
    #[must_use]
    pub fn failed(&self) -> bool {
        self.items
            .iter()
            .any(|item| matches!(item.outcome, ReconcileOutcome::Failed { .. }))
    }
}

/// What `task status complete` and [`Engine::complete_issues`] answer with.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct IssuesCompleted {
    /// One entry per distinct reference for each source asked, sources in configuration order
    /// and references in the order given.
    pub items: Vec<IssueCompleted>,
    /// Every source that would have been asked and holds no item by issue reference — its
    /// status is its closure, or its plugin does not declare `complete_by_issue` — each sent
    /// no request.
    pub unsupported: Vec<SourceName>,
}

impl IssuesCompleted {
    /// Whether any item failed, or a task one of them delivers could not be kept in step with
    /// it — which is what exits `4`.
    #[must_use]
    pub fn failed(&self) -> bool {
        self.items.iter().any(|item| {
            item.action == CompleteAction::Failed || item.delivered.iter().any(Delivered::failed)
        })
    }
}

/// What one source holds for one issue reference, and what moving it to `done` came to.
///
/// Its members are flat — `issue`, `source`, `task`, `kind`, `from`, `to`, `action`, `reason`,
/// `error` — because a landing's record of its closures and the read API consume exactly that shape;
/// [`Engine::complete_issues`] is the one writer, and it fills them together from one
/// [`Completion`].
// llmlint: ignore[invalid_states_unrepresentable] The contract for `IssueCompleted` names these nine flat members and their types, and a landing's closure record and the read API consume them, so folding them into tagged variants is a contract change only the contract's owner may make. `Engine::complete_issues` is the one constructor, and each arm of `completion` there fills the members its action owns and no other.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct IssueCompleted {
    /// The issue, as `owner/name#N`.
    pub issue: IssueRef,
    /// The source asked.
    pub source: SourceName,
    /// The item that source holds for the issue, by its qualified id whichever its kind —
    /// `null` when it holds none.
    pub task: Option<GlobalId>,
    /// What the held item is: `task`, or `project` for a project's own card — a plan's home.
    /// `task` when nothing is held, and for a reference the source could not be asked about.
    pub kind: ItemKind,
    /// The option the task's card sat at, or `null`.
    pub from: Option<String>,
    /// The status the task reads at once written, would read under a dry run, or already
    /// reads — `null` when nothing is held or the closure says otherwise.
    pub to: Option<Status>,
    /// What became of it.
    pub action: CompleteAction,
    /// Why it was skipped — `closed as <reason>` — present only then.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// Why it failed — the failure object a failure document carries — present only then.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<Failure>,
    /// Every task the written task delivers, kept in step with it exactly as `task status set`
    /// keeps them — left out when it delivers none, which a project never does.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[schemars(!skip_serializing_if)]
    pub delivered: Vec<Delivered>,
}

/// What moving one issue's item to `done` came to at one source.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum CompleteAction {
    /// The task was written `done`.
    Updated,
    /// The task would be written `done`; a dry run wrote nothing.
    WouldUpdate,
    /// The issue is closed as completed and its card already reads `done`.
    Unchanged,
    /// The issue is closed some other way, which a landing does not overrule.
    Skipped,
    /// The source holds no item for the issue, or cannot see it.
    Absent,
    /// The task could not be written, or the source could not be read; `error` says why.
    Failed,
}

impl Engine {
    /// Bring every item of `source` whose card contradicts its issue's closure in line, writing
    /// only its card through the source's own status write — or, under `dry_run`, nothing.
    ///
    /// A source that does not declare
    /// [`reconcile_status`](onetaskgraph_plugin_api::Capabilities::reconcile_status) — its
    /// status read as its closure — is sent no request and answers `None`.
    ///
    /// # Errors
    ///
    /// Returns [`EngineError::UnknownSource`] for a source nothing configures,
    /// [`EngineError::SourceUnavailable`] for one that did not build, and
    /// [`EngineError::SourceFailed`] when its read of what is out of line fails. One item whose
    /// write fails is not an error: it is reported `failed` beside the others.
    pub async fn reconcile_statuses(
        &self,
        source: &SourceName,
        dry_run: bool,
    ) -> Result<Option<StatusesReconciled>, EngineError> {
        let built = self.built(source)?;
        if !built.source().capabilities().reconcile_status.is_native() {
            return Ok(None);
        }
        let contradictions = built
            .source()
            .status_contradictions()
            .await
            .map_err(|error| source_failed(built, error))?;
        // A plugin's answer is held to what the method promises before any of it is written:
        // each task once, so no card is written twice in one run.
        let mut seen = std::collections::BTreeSet::new();
        for contradiction in &contradictions {
            let kind = contradiction.item.kind();
            if !seen.insert((kind.marker(), contradiction.item.id().clone())) {
                return Err(source_failed(
                    built,
                    SourceError::Malformed {
                        message: format!(
                            "status_contradictions answered {} {} more than once",
                            kind.marker(),
                            contradiction.item.id()
                        ),
                    },
                ));
            }
        }
        let mut items = Vec::with_capacity(contradictions.len());
        for contradiction in contradictions {
            items.push(self.reconciled(built, contradiction, dry_run).await);
        }
        Ok(Some(StatusesReconciled { items }))
    }

    /// One contradiction brought in line, or reported as it would be.
    async fn reconciled(
        &self,
        source: &ResolvedSource,
        contradiction: StatusContradiction,
        dry_run: bool,
    ) -> StatusReconciled {
        let StatusContradiction {
            item,
            option,
            closed_as,
            to,
        } = contradiction;
        let kind = item.kind();
        let id = GlobalId::new(source.name().clone(), item.id().clone());
        let planned = Status {
            category: closed_as.category(),
            name: to,
        };
        let entry = |outcome, to| StatusReconciled {
            source: id.clone(),
            kind,
            from: option.clone(),
            to,
            outcome,
        };
        if dry_run {
            return entry(ReconcileOutcome::WouldUpdate, planned);
        }
        let written = self
            .write_category(
                source,
                &id,
                kind,
                item.classification(),
                item.repositories(),
                closed_as.category(),
            )
            .await;
        match written {
            Ok(status) => entry(ReconcileOutcome::Updated, status),
            Err(error) => entry(
                ReconcileOutcome::Failed {
                    error: Failure::from(&error),
                },
                planned,
            ),
        }
    }

    /// Move to `done` every task the configured sources hold for `issues`, by issue reference,
    /// and keep every task each one delivers in step — or, under `dry_run`, say which would
    /// move and write nothing.
    ///
    /// The sources asked are every one the configuration declares, in configuration order —
    /// not only the default selection — or, when `only` names some, only those. Each that
    /// declares [`complete_by_issue`](onetaskgraph_plugin_api::Capabilities::complete_by_issue)
    /// is asked once per fifty references, one after another, so an issue held on two boards is
    /// closed by the first source's write and read closed by the second, which moves only its
    /// card; every other source is sent no request and listed in
    /// [`IssuesCompleted::unsupported`]. An empty `issues` sends no request at all.
    ///
    /// # Errors
    ///
    /// Returns [`EngineError::UnknownSource`] when `only` names a source nothing configures,
    /// before anything is asked. A source that fails to read, and a task whose write fails, are
    /// not errors: each is reported `failed` beside the others.
    pub async fn complete_issues(
        &self,
        issues: &[IssueRef],
        only: &[SourceName],
        dry_run: bool,
    ) -> Result<IssuesCompleted, EngineError> {
        for name in only {
            self.known(name)?;
        }
        let mut distinct: Vec<IssueRef> = Vec::with_capacity(issues.len());
        for issue in issues {
            if !distinct.contains(issue) {
                distinct.push(issue.clone());
            }
        }
        let mut asked: Vec<&ConfiguredSource> = self
            .sources
            .iter()
            .filter(|source| only.is_empty() || only.contains(source.name()))
            .collect();
        asked.sort_by(|a, b| a.name().cmp(b.name()));
        let mut completed = IssuesCompleted {
            items: Vec::new(),
            unsupported: Vec::new(),
        };
        for configured in asked {
            let source = match configured {
                ConfiguredSource::Ready(source) => source,
                ConfiguredSource::Unavailable(_) => {
                    let error = self
                        .built(configured.name())
                        .err()
                        .unwrap_or(EngineError::NoSources);
                    completed.items.extend(
                        distinct
                            .iter()
                            .map(|issue| failed_completion(issue, configured.name(), &error)),
                    );
                    continue;
                }
            };
            if !source.source().capabilities().complete_by_issue.is_native() {
                completed.unsupported.push(source.name().clone());
                continue;
            }
            if distinct.is_empty() {
                continue;
            }
            let holdings = match source
                .source()
                .issue_holdings(&distinct)
                .await
                .and_then(|holdings| answered_each(&distinct, holdings))
            {
                Ok(holdings) => holdings,
                Err(error) => {
                    let error = source_failed(source, error);
                    completed.items.extend(
                        distinct
                            .iter()
                            .map(|issue| failed_completion(issue, source.name(), &error)),
                    );
                    continue;
                }
            };
            for holding in holdings {
                completed
                    .items
                    .push(self.completion(source, holding, dry_run).await);
            }
        }
        Ok(completed)
    }

    /// What one holding came to: written `done` when it is due and this is not a dry run.
    async fn completion(
        &self,
        source: &ResolvedSource,
        holding: IssueHolding,
        dry_run: bool,
    ) -> IssueCompleted {
        let named = |task: &HeldTask| GlobalId::new(source.name().clone(), task.id.clone());
        let entry = |task: Option<&HeldTask>, action| IssueCompleted {
            issue: holding.issue.clone(),
            source: source.name().clone(),
            task: task.map(named),
            kind: task.map_or(ItemKind::Task, |task| task.kind),
            from: task.and_then(|task| task.option.clone()),
            to: None,
            action,
            reason: None,
            error: None,
            delivered: Vec::new(),
        };
        match &holding.completion {
            Completion::Absent => entry(None, CompleteAction::Absent),
            Completion::Skipped { task, reason } => IssueCompleted {
                reason: Some(reason.clone()),
                ..entry(Some(task), CompleteAction::Skipped)
            },
            Completion::Unchanged { task } => IssueCompleted {
                to: task.option.clone().map(|name| Status {
                    category: StatusCategory::Done,
                    name,
                }),
                ..entry(Some(task), CompleteAction::Unchanged)
            },
            Completion::Due { task, to } => {
                let planned = Status {
                    category: StatusCategory::Done,
                    name: to.clone(),
                };
                if dry_run {
                    return IssueCompleted {
                        to: Some(planned),
                        ..entry(Some(task), CompleteAction::WouldUpdate)
                    };
                }
                let id = named(task);
                match self
                    .write_category(
                        source,
                        &id,
                        task.kind,
                        task.classification,
                        &task.repositories,
                        StatusCategory::Done,
                    )
                    .await
                {
                    // A project delivers no task, so there is nothing to keep in step with it.
                    Ok(status) if task.kind == ItemKind::Project => IssueCompleted {
                        to: Some(status),
                        ..entry(Some(task), CompleteAction::Updated)
                    },
                    Ok(status) => {
                        let delivers = targets(&task.delivers, &id.source);
                        let delivered = self
                            .deliver(
                                &id,
                                task.classification,
                                status.category,
                                &delivers,
                                &delivers,
                            )
                            .await;
                        IssueCompleted {
                            to: Some(status),
                            delivered,
                            ..entry(Some(task), CompleteAction::Updated)
                        }
                    }
                    Err(error) => IssueCompleted {
                        to: Some(planned),
                        error: Some(Failure::from(&error)),
                        ..entry(Some(task), CompleteAction::Failed)
                    },
                }
            }
        }
    }

    /// One item's status written through its source's own status write for its kind, after the
    /// public boundary has admitted it, answering the status as the source reads it back — which
    /// has to be the category written, or the source is refused as malformed rather than
    /// reported as having written it.
    async fn write_category(
        &self,
        source: &ResolvedSource,
        id: &GlobalId,
        kind: ItemKind,
        classification: onetaskgraph_plugin_api::Classification,
        repositories: &[onetaskgraph_plugin_api::Repository],
        category: StatusCategory,
    ) -> Result<Status, EngineError> {
        self.admit_known(
            source,
            &id.to_string(),
            &id.native,
            classification,
            repositories,
            &Exposure::metadata([category_word(category)]),
        )
        .await?;
        let written = match kind {
            ItemKind::Task => source.source().set_task_status(&id.native, category).await,
            ItemKind::Project => {
                source
                    .source()
                    .set_project_status(&id.native, category)
                    .await
            }
        }
        .map_err(|error| source_failed(source, error))?
        .ok_or_else(|| match kind {
            ItemKind::Task => EngineError::NoSuchTask { id: id.to_string() },
            ItemKind::Project => EngineError::NoSuchProject { id: id.to_string() },
        })?;
        if written.category != category {
            return Err(source_failed(
                source,
                SourceError::Malformed {
                    message: format!(
                        "a write of {id} to {} read back as {}",
                        category_word(category),
                        category_word(written.category)
                    ),
                },
            ));
        }
        Ok(written)
    }
}

/// A source's holdings, held to what `issue_holdings` promises: one per reference it was asked
/// about, in the order asked, each about that very reference, and no project naming a task it
/// delivers — a project delivers none. Anything else is a malformed answer, refused for the
/// whole source rather than read as some references holding nothing.
fn answered_each(
    asked: &[IssueRef],
    holdings: Vec<IssueHolding>,
) -> Result<Vec<IssueHolding>, SourceError> {
    if holdings.len() != asked.len() {
        return Err(SourceError::Malformed {
            message: format!(
                "issue_holdings answered {} holding(s) for {} reference(s)",
                holdings.len(),
                asked.len()
            ),
        });
    }
    for (holding, issue) in holdings.iter().zip(asked) {
        if holding.issue != *issue {
            return Err(SourceError::Malformed {
                message: format!(
                    "issue_holdings answered {} where {issue} was asked about",
                    holding.issue
                ),
            });
        }
        let held = match &holding.completion {
            Completion::Absent => None,
            Completion::Skipped { task, .. }
            | Completion::Unchanged { task }
            | Completion::Due { task, .. } => Some(task),
        };
        if let Some(project) =
            held.filter(|held| held.kind == ItemKind::Project && !held.delivers.is_empty())
        {
            return Err(SourceError::Malformed {
                message: format!(
                    "issue_holdings answered project {} for {issue} as delivering tasks, and a \
                     project delivers none",
                    project.id
                ),
            });
        }
    }
    Ok(holdings)
}

/// A reference a source could not be asked about, reported failed with why.
fn failed_completion(issue: &IssueRef, source: &SourceName, error: &EngineError) -> IssueCompleted {
    IssueCompleted {
        issue: issue.clone(),
        source: source.clone(),
        task: None,
        kind: ItemKind::Task,
        from: None,
        to: None,
        action: CompleteAction::Failed,
        reason: None,
        error: Some(Failure::from(error)),
        delivered: Vec::new(),
    }
}

/// A category as the wire spells it, for the boundary's check of what a status write carries.
fn category_word(category: StatusCategory) -> String {
    serde_json::to_value(category)
        .ok()
        .and_then(|value| value.as_str().map(str::to_owned))
        .unwrap_or_default()
}
