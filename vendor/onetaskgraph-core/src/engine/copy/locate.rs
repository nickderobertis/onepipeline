//! `task locate`: where a copy of each task would land, which item there it would update, and
//! which repository a create would file its issue in — answered by the copy's own placement
//! and counterpart lookup, writing nothing.
//!
//! Nothing here restates a rule of the copy. Each task is read and classified by
//! [`Engine::placement_of`], which every copy places through; its counterpart is found by
//! [`Engine::target`] and its misrouting refused by [`Engine::misrouted`], which every copy
//! looks up through; a private task bound for a source not declared private is refused by
//! [`Engine::refuse_private_landing`], which every write is held to; the project it would be
//! filed under is [`Engine::counterpart`]'s; and the repository is the destination's own
//! [`TaskSource::creation_repository`](onetaskgraph_plugin_api::TaskSource::creation_repository),
//! which the GitHub Projects source answers from the rule its create applies. So a locate and
//! the copy after it cannot disagree.
//!
//! What it never does is what a copy does only to write: the caller's boundary check, a
//! destination's live visibility, an asset or a forward edge read.

use onetaskgraph_plugin_api::{Classification, NativeId, Repository, SourceName, WriteTarget};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::GlobalId;
use crate::config::Placement;

use super::{
    CopyItems, CopyRequest, CopyScope, CopyVia, Item, Journal, Level, Planned, Read, Running,
    Spent, StatusPolicy, Target, classification_of, readings, refused, spent_between,
};
use crate::engine::{Engine, EngineError};

/// A request to locate tasks against one configured destination — `task locate`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocateRequest {
    /// The qualified tasks to locate, in the order they were named.
    pub items: Vec<GlobalId>,
    /// The configured source a copy would be made into, as `task copy --to` names it.
    pub destination: SourceName,
    /// How each task's counterpart is looked for.
    pub lookup: LocateLookup,
}

/// How `task locate` looks for each task's counterpart at the destination it is placed in.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum LocateLookup {
    /// By the copy's own rules, in its order: the task's link, its own origin, then the
    /// destination's origin query read to its last page.
    #[default]
    Counterpart,
    /// Not at all — `--no-counterpart`: no link, origin or origin-query read at the
    /// destination, and no `counterpart` or `carriers` in the answer.
    NoCounterpart,
    /// The caller knows it — `--target-native`: the item with this native id where the one
    /// task named is placed, read by id and verified by its origin naming that task back, as
    /// `task copy --target-task` verifies its target. The link, the origin and the origin query
    /// are not followed.
    TargetNative(NativeId),
}

/// What `task locate` answers: one entry per task, in the order they were named.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct LocateReport {
    /// One entry per task named, in the order named.
    pub items: Vec<Located>,
    /// What the command spent at the sources that meter their own requests, as a copy reports
    /// it — absent when none of them does.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spent: Option<Spent>,
}

/// Where one task would land, what it would update there, and where a create would file it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Located {
    /// The qualified id the task was read from.
    pub source: GlobalId,
    /// The class it was placed as, as a copy classifies it: its own declaration and, while the
    /// store's boundary is active, its repositories' visibility. A source declared private does
    /// not by itself make it private; it withholds the task's provenance instead.
    pub classification: Classification,
    /// Where a copy to the destination named would write it, and the index of the route entry
    /// that sent it there — `null` when none matched, exactly as a copy's report says it.
    #[serde(flatten)]
    pub placement: Placement,
    /// The item a copy would update, and the rule that found it — `null` when a copy would
    /// create one, and absent under `--no-counterpart`, which looks for none.
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[schemars(with = "Option<LocatedCounterpart>")]
    pub counterpart: Option<Option<LocatedCounterpart>>,
    /// Every destination item the origin query found carrying this task's origin, in listing
    /// order — `[]` when the query did not run, and absent under `--no-counterpart`. When it ran,
    /// `counterpart` is the first.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub carriers: Option<Vec<GlobalId>>,
    /// The repository a create there would file the task's issue in, or `null` where the
    /// destination files no issue in a repository. Where a create would file, whether or not a
    /// counterpart exists: an existing issue is never moved.
    pub creates_in: Option<CreatesIn>,
}

/// A member present in a document, `null` included: what tells `counterpart: null` — a copy
/// would create — from no `counterpart` at all.
fn present<'de, D, T>(deserializer: D) -> Result<Option<Option<T>>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(deserializer).map(Some)
}

/// The destination item a copy would update.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct LocatedCounterpart {
    /// Its qualified id at the destination the task is placed in.
    pub id: GlobalId,
    /// Which rule found it, as a copy's report spells it.
    pub via: CopyVia,
}

/// What an id `task locate` was named holds instead of a task.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotATask {
    /// A project.
    Project,
    /// A document.
    Document,
}

impl std::fmt::Display for NotATask {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::Project => "project",
            Self::Document => "document",
        })
    }
}

/// Where a create would file a task's issue.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CreatesIn {
    /// The repository, as a normalized origin `host/owner/name`.
    pub repository: Repository,
    /// Why the destination's own policy on where it creates — a GitHub Projects source's
    /// `creation_owners` — would refuse a create there, as the copy's refusal says it; absent
    /// when it would not. The locate itself never fails for it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refused: Option<String>,
}

impl Engine {
    /// Locate every task `request` names against its destination, writing nothing anywhere.
    ///
    /// # Errors
    ///
    /// [`EngineError::TargetNativeNeedsOne`] for a `--target-native` beside more or fewer than
    /// one task, before anything is read; [`EngineError::NotATask`] for an id naming a project
    /// or a document; and every refusal a copy of the same task makes before its first write
    /// while it places the task and looks its counterpart up — a destination nothing
    /// configures, a stale link or origin, a misrouted counterpart, a private task bound for a
    /// source not declared private, a `--target-native` item missing or not naming the task
    /// back — with the copy's own error.
    pub async fn locate(&self, request: &LocateRequest) -> Result<LocateReport, EngineError> {
        let mut result = None;
        let mut operation: std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + '_>> =
            Box::pin(async { result = Some(self.locate_in_scope(request).await) });
        for source in self.ready() {
            operation = source.source().scope_visibility(operation);
        }
        operation.await;
        result.expect("the scoped operation completed")
    }

    async fn locate_in_scope(&self, request: &LocateRequest) -> Result<LocateReport, EngineError> {
        // A source declared private reads a named repository to place a create, once per
        // destination for the whole locate.
        let _held = self.hold_visibility();
        if matches!(request.lookup, LocateLookup::TargetNative(_)) && request.items.len() != 1 {
            return Err(EngineError::TargetNativeNeedsOne {
                count: request.items.len(),
            });
        }
        let Some(items) = CopyItems::new(request.items.clone()) else {
            return Ok(LocateReport {
                items: Vec::new(),
                spent: None,
            });
        };
        // The copy this answers for: a dry run of these tasks, which is what makes the filing
        // lookup below create nothing.
        let copy = CopyRequest {
            items,
            scope: CopyScope::Tasks,
            status_policy: StatusPolicy::Source,
            destination: request.destination.clone(),
            match_by: None,
            target_task: None,
            recreate: false,
            create: false,
            dry_run: true,
            comments: false,
            no_link: true,
            expect: None,
            statuses: Vec::new(),
            conditions: Default::default(),
        };
        let destination = self.writable(&copy.destination)?;
        let metered = self.metered(&copy, destination)?;
        let before = readings(&metered).await;
        let mut running = Running::default();
        let mut located = Vec::with_capacity(request.items.len());
        for id in &request.items {
            located.push(
                self.locate_one(&copy, &request.lookup, id, &mut running)
                    .await?,
            );
        }
        Ok(LocateReport {
            items: located,
            spent: spent_between(&before, &readings(&metered).await),
        })
    }

    /// Place one task, look its counterpart up as `lookup` says, and ask where a create of it
    /// would file its issue.
    async fn locate_one(
        &self,
        copy: &CopyRequest,
        lookup: &LocateLookup,
        id: &GlobalId,
        running: &mut Running,
    ) -> Result<Located, EngineError> {
        let read = self.read_task(id).await?;
        let (classification, placement) = self.placement_of(copy, &read, running).await?;
        let to = self.writable(&placement.destination)?;
        let withheld = self.withholds_provenance(&id.source, &placement.destination);
        let (target, held, carriers) = match lookup {
            LocateLookup::NoCounterpart => (Target::Create, None, None),
            LocateLookup::Counterpart => {
                if self.routes.routes(&copy.destination) {
                    self.misrouted(copy, id, &read.item, &placement.destination)
                        .await?;
                }
                let mut carriers = Vec::new();
                let (target, held) = self
                    .target(to, copy, id, &read.item, withheld, Some(&mut carriers))
                    .await?;
                (target, held, Some(carriers))
            }
            LocateLookup::TargetNative(native) => {
                let mut targeted = copy.clone();
                targeted.target_task =
                    Some(GlobalId::new(placement.destination.clone(), native.clone()));
                let (target, held) = self
                    .target(to, &targeted, id, &read.item, withheld, None)
                    .await?;
                (target, held, Some(Vec::new()))
            }
        };
        // As a copy's preflight holds every write: what it updates is as private as it is.
        let landing = classification.strictest(
            held.as_ref()
                .map_or(Classification::Public, |held| classification_of(&held.item)),
        );
        self.refuse_private_landing(&placement.destination, &id.to_string(), landing)?;
        let counterpart = carriers.as_ref().map(|_| match &target {
            Target::Update { id: native, found } => Some(LocatedCounterpart {
                id: GlobalId::new(placement.destination.clone(), native.clone()),
                via: found.via(),
            }),
            Target::Create => None,
        });
        let carriers = carriers.map(|carriers| {
            carriers
                .into_iter()
                .map(|native| GlobalId::new(placement.destination.clone(), native))
                .collect()
        });
        let Item::Task(task) = &read.item else {
            unreachable!("read_task reads tasks alone");
        };
        let repositories = task.repositories.clone();
        let project = task.project.clone();
        // Asked first without a project, which reads nothing: a destination filing no issue in
        // a repository answers `None` then too, and the filing lookup is spared.
        let unfiled = to
            .source()
            .creation_repository(&WriteTarget::New {
                repositories: &repositories,
                project: None,
            })
            .await
            .map_err(|error| refused(to, error))?;
        let (repository, filed) = match (unfiled, project) {
            (Some(_), Some(project)) => {
                let planned = Planned {
                    status_policy: copy.status_policy,
                    source: id.clone(),
                    source_status: read.item.status().cloned(),
                    item: read.item,
                    edges: Vec::new(),
                    target,
                    held,
                    to: placement.destination.clone(),
                    placed: None,
                    assets: Vec::new(),
                    classification: landing,
                    withheld,
                    // A lookup writes nothing, so it records no lineage.
                    lineage: None,
                    carried: None,
                    comment_plan: None,
                    skipped: false,
                    status_condition: None,
                    priority_condition: None,
                    assignee_condition: None,
                    assignees: None,
                    intent: None,
                    relist: false,
                    links: None,
                    pending: Vec::new(),
                    comment_links: Vec::new(),
                    comment_sections: super::sections::SectionCounts::default(),
                    rewrite: super::Rewrite::default(),
                };
                // A dry run's filing lookup, so it creates no member project; what it would have
                // written is nothing, and the journal is dropped with nothing in it to undo.
                let mut journal = Journal::default();
                let filed = self
                    .counterpart(copy, &planned, Some(&project), running, &mut journal)
                    .await?;
                let repository = to
                    .source()
                    .creation_repository(&WriteTarget::New {
                        repositories: &repositories,
                        project: filed.as_ref(),
                    })
                    .await
                    .map_err(|error| refused(to, error))?;
                (repository, filed)
            }
            (unfiled, _) => (unfiled, None),
        };
        // What the destination's own policy on where it creates says of that repository, which
        // a locate reports rather than fails on: it answers where a copy would land.
        let refused_here = match &repository {
            Some(_) => to
                .source()
                .creation_refusal(&WriteTarget::New {
                    repositories: &repositories,
                    project: filed.as_ref(),
                })
                .await
                .map_err(|error| refused(to, error))?,
            None => None,
        };
        Ok(Located {
            source: id.clone(),
            classification,
            placement,
            counterpart,
            carriers,
            creates_in: repository.map(|repository| CreatesIn {
                repository,
                refused: refused_here,
            }),
        })
    }

    /// The task `id` names, or — for an id naming no task — the refusal saying it names a
    /// project or a document, when it does. A probe the source cannot answer fails as itself:
    /// whether the id names something else is then not known.
    async fn read_task(&self, id: &GlobalId) -> Result<Read, EngineError> {
        let missing = match self.read(Level::Task, id).await {
            Err(missing @ EngineError::NoSuchItem { .. }) => missing,
            other => return other,
        };
        let source = self.readable(&id.source)?;
        let capabilities = source.source().capabilities();
        if capabilities.projects.is_native()
            && source
                .source()
                .get_project(&id.native)
                .await
                .map_err(|error| refused(source, error))?
                .is_some()
        {
            return Err(EngineError::NotATask {
                id: id.clone(),
                kind: NotATask::Project,
            });
        }
        if capabilities.documents.is_native()
            && source
                .source()
                .get_document(&id.native)
                .await
                .map_err(|error| refused(source, error))?
                .is_some()
        {
            return Err(EngineError::NotATask {
                id: id.clone(),
                kind: NotATask::Document,
            });
        }
        Err(missing)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A located task's counterpart reads back as it was written: looked up and found, looked up
    /// and not found — `null` — and not looked up at all, which writes no member.
    #[test]
    fn a_counterpart_round_trips_found_absent_and_not_looked_up() {
        let located = |counterpart, carriers| Located {
            source: "drafts:t".parse().unwrap(),
            classification: Classification::Public,
            placement: Placement {
                destination: SourceName::new("site".to_owned()).unwrap(),
                route: None,
            },
            counterpart,
            carriers,
            creates_in: None,
        };
        let found = LocatedCounterpart {
            id: "site:t".parse().unwrap(),
            via: CopyVia::Link,
        };
        for (item, members) in [
            (located(Some(Some(found)), Some(Vec::new())), 2),
            (located(Some(None), Some(Vec::new())), 2),
            (located(None, None), 0),
        ] {
            let written = serde_json::to_value(&item).unwrap();
            let present = ["counterpart", "carriers"]
                .iter()
                .filter(|key| written.get(**key).is_some())
                .count();
            assert_eq!(present, members, "{written}");
            assert_eq!(serde_json::from_value::<Located>(written).unwrap(), item);
        }
    }
}
