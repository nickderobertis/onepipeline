//! The engine's half of a human task projection: routing, screening and the tracker writes,
//! each held to the producer's durable intent (`crate::task_projection`).
//!
//! The intent is the authority, not this file: a create is sent only once the intent says one
//! may be, a pending create is bound only by one verified origin match, and a terminal write
//! is committed before it is sent. What this file owns is everything the existing write path
//! already decides — where an item routes, whether it may be written there, and what the
//! source is asked — with the reads a projection does not need left out: no whole-project
//! classification, no read back of what was written, and no read of a task this producer
//! wrote before when the ids it kept say where it is.

use std::collections::BTreeMap;

use onetaskgraph_plugin_api::{
    Classification, ItemWrite, MetadataKey, MetadataRecord, NativeId, PageRequest, Status,
    StatusCategory, Task, TaskQuery, TaskUpdate, WriteTarget,
};
use serde_json::Value;

use super::boundary::{Exposure, Outbound};
use super::delivery::source_failed;
use super::{Engine, EngineError, ResolvedSource};
use crate::GlobalId;
use crate::task_projection::{
    HumanTaskProjected, HumanTaskProjection, IntentPhase, IntentStore, PlacementCache,
    ProjectedTaskId, ProjectionClaim, ProjectionError, ProjectionPlacement, ProjectionProjectId,
    ProjectionState, TrackerUrl, route_purpose,
};

/// The metadata keys a projected task carries, read back by recovery.
const ITEM_ID_KEY: &str = "human.item_id";
const OWNER_KEY: &str = "human.owner";
const DIGEST_KEY: &str = "human.payload_digest";
const HOST_KEY: &str = "human.host";
/// A dismissal's reason, recorded only when the request gives one.
const REASON_KEY: &str = "human.reason";

/// The most origin matches one recovery reads; a page holding more is ambiguous.
const RECOVERY_PAGE: u32 = 10;

impl Engine {
    /// Project one human task: create it once, bind its canonical identity durably, and settle
    /// it at the terminal state the request names — or answer the binding already held.
    ///
    /// `current_host` is the host this process runs on; only the host a destination's
    /// `task_projection` names may project into it.
    ///
    /// # Errors
    ///
    /// [`EngineError::ProjectionRefused`] for a request its routing, its durable intent or the
    /// tracker's evidence does not authorize — a private item with no private route, a project
    /// of another source, another host, a changed payload, a conflicting terminal state, and a
    /// pending create with no single verified match — and the source's own refusal of a write.
    pub async fn project_human(
        &self,
        request: &HumanTaskProjection,
        current_host: &str,
    ) -> Result<HumanTaskProjected, EngineError> {
        let refused = |message: String, next: &str| EngineError::ProjectionRefused {
            item_id: request.item_id.to_string(),
            message,
            next: next.to_owned(),
        };
        self.built(&request.source)?;
        if request.reason.is_some() && request.state != ProjectionState::Dismissed {
            return Err(refused(
                "a reason is recorded only for a dismissal".to_owned(),
                "send the reason with state dismissed, or send it as null.",
            ));
        }
        // Who may read it is its repositories' answer while the boundary is active; it is
        // routed by that and by its repositories through the human-task entries alone.
        let classification = self
            .classify(Classification::Public, &request.repositories)
            .await;
        let destination = self
            .routes
            .place_for(
                &request.source,
                classification,
                &request.repositories,
                Some(&route_purpose()),
            )
            .destination;
        let item = format!("human task {:?}", request.item_id.as_str());
        // A private item with no private route stays where it was sent, and is refused there:
        // nothing is ever written publicly in its place.
        self.refuse_private_landing(&destination, &item, classification)?;
        let settings = match self.projections.get(&destination) {
            Some(Ok(settings)) => settings,
            Some(Err(why)) => return Err(refused(why.clone(), "project into a tracker source.")),
            None => {
                return Err(refused(
                    format!("source {destination} declares no task_projection"),
                    &format!(
                        "declare sources.{destination}.task_projection with its host and \
                         intent directory."
                    ),
                ));
            }
        };
        let project = match &request.project {
            Some(project) => match project.as_str().split_once(':') {
                Some((source, native)) if source == destination.as_str() => {
                    ProjectionProjectId::new(native)
                        .map_err(|why| refused(why, "name a project."))?
                }
                Some((source, _)) => {
                    return Err(refused(
                        format!(
                            "project {project} is in source {source}, but this item routes to \
                             {destination}"
                        ),
                        "name the plan's project resolved in the source the item routes to; \
                         nothing was written anywhere else.",
                    ));
                }
                None => project.clone(),
            },
            None => settings.project.clone().ok_or_else(|| {
                refused(
                    format!(
                        "the request names no project and source {destination} configures none"
                    ),
                    &format!("set sources.{destination}.task_projection.project."),
                )
            })?,
        };
        let source = self.creatable(&destination, MetadataRecord::Task)?;
        let store = IntentStore::new(
            settings.directory.clone(),
            settings.host.as_str().to_owned(),
            current_host.to_owned(),
        )
        .map_err(|error| projection_refused(request, &error))?;
        let placement = ProjectionPlacement {
            identity: settings.identity.clone(),
            source: destination.clone(),
            project: project.clone(),
            classification,
        };
        let native_project = project.native_id();
        // Held to the destination's declaration and — while the boundary is active and the
        // destination is not declared private — to the caller's check, before the intent can
        // authorize anything.
        let task = projected_task(request, &project, classification, None);
        let rewrite = self
            .admit(
                source,
                &Outbound {
                    item: item.clone(),
                    classification,
                    target: WriteTarget::New {
                        repositories: &request.repositories,
                        project: Some(&native_project),
                    },
                    exposure: &Exposure::of_task(&task),
                },
            )
            .await?;
        let mut claim = store
            .claim(request, &placement)
            .map_err(|error| projection_refused(request, &error))?;
        let mut task = projected_task(request, &project, classification, Some(&claim));
        rewrite.task(&mut task);
        match claim.phase() {
            IntentPhase::Bound => {}
            IntentPhase::Creating => {
                let matches = self.origin_matches(source, claim.origin()).await?;
                claim
                    .reconcile(request, &matches)
                    .map_err(|error| projection_refused(request, &error))?;
                // The match this process just read is what a later terminal write takes instead
                // of reading it again, exactly as a create's own answer is.
                if let Some(binding) = claim.binding().cloned() {
                    let written = source
                        .source()
                        .written_context(&binding.task().global_id().native);
                    claim
                        .bind_written(binding, written)
                        .map_err(|error| projection_refused(request, &error))?;
                }
            }
            IntentPhase::Ready | IntentPhase::Uncertain => {
                self.create_projection(source, &mut claim, request, &task)
                    .await?;
            }
        }
        if request.state != ProjectionState::Open {
            self.settle_projection(source, &item, &mut claim, request, &task)
                .await?;
        }
        claim
            .binding()
            .cloned()
            .ok_or_else(|| refused("no binding was committed".to_owned(), "project it again."))
    }

    /// Send the one create the intent authorizes, and bind what it returned.
    async fn create_projection(
        &self,
        source: &ResolvedSource,
        claim: &mut ProjectionClaim,
        request: &HumanTaskProjection,
        task: &Task,
    ) -> Result<(), EngineError> {
        let kept = self
            .projections
            .get(&claim.placement().source)
            .and_then(|settings| settings.as_ref().ok())
            .and_then(|settings| settings.placement.clone());
        if let Some(context) = kept
            .as_ref()
            .map(PlacementCache::read)
            .transpose()
            .map_err(|error| projection_refused(request, &error))?
            .flatten()
        {
            source.source().prime_placement(&context);
        }
        claim
            .begin_create()
            .map_err(|error| projection_refused(request, &error))?;
        let write = ItemWrite {
            target: None,
            item: task.clone(),
            depends_on: Vec::new(),
        };
        let id = match source.source().write_task(&write).await {
            Ok(id) => id,
            Err(error) => {
                // What it held may be what the tracker refused; the next create reads afresh.
                if let Some(kept) = &kept {
                    let _ = kept.forget();
                }
                return Err(source_failed(source, error));
            }
        };
        if let (Some(kept), Some(context)) = (&kept, source.source().placement_context()) {
            let _ = kept.keep(&context);
        }
        let written = source.source().written_context(&id);
        let url = match source
            .source()
            .written_location(MetadataRecord::Task, &id)
            .and_then(|location| match location {
                onetaskgraph_plugin_api::Location::Url(url) => Some(url),
                onetaskgraph_plugin_api::Location::Path(_) => None,
            }) {
            Some(url) => url,
            None => {
                // The create's answer named no address, so the tracker is asked: what this process
                // remembers of its own write knows no more than that answer did.
                source
                    .source()
                    .end_command()
                    .await
                    .map_err(|error| source_failed(source, error))?;
                source
                    .source()
                    .get_task(&id)
                    .await
                    .map_err(|error| source_failed(source, error))?
                    .and_then(|task| task.url)
                    .unwrap_or_default()
            }
        };
        let placement = claim.placement().clone();
        let binding = HumanTaskProjected::new(
            ProjectedTaskId::try_from(GlobalId::new(placement.source.clone(), id.clone()))
                .map_err(|why| refused_by(request, why, "inspect the tracker."))?,
            TrackerUrl::new(url).map_err(|why| {
                refused_by(
                    request,
                    format!(
                        "the tracker answered the create of {id} without a canonical URL: {why}"
                    ),
                    "inspect the tracker; the created task is reconciled by its origin.",
                )
            })?,
            placement.project,
        );
        claim
            .bind_written(binding, written)
            .map_err(|error| projection_refused(request, &error))
    }

    /// Every task of `source` carrying `origin`, read once and held to the origin exactly.
    async fn origin_matches(
        &self,
        source: &ResolvedSource,
        origin: &str,
    ) -> Result<Vec<Task>, EngineError> {
        let page = source
            .source()
            .query_tasks(
                &TaskQuery {
                    origin: Some(origin.to_owned()),
                    ..TaskQuery::default()
                },
                &PageRequest {
                    cursor: None,
                    limit: RECOVERY_PAGE,
                },
            )
            .await
            .map_err(|error| source_failed(source, error))?;
        let mut matches: Vec<Task> = page
            .items
            .into_iter()
            .filter(|task| {
                task.metadata
                    .get(GlobalId::ORIGIN_KEY)
                    .and_then(Value::as_str)
                    == Some(origin)
            })
            .collect();
        // A page with more behind it is not a whole answer, so it cannot be one match.
        if page.next.is_some() && matches.len() == 1 {
            matches.push(matches[0].clone());
        }
        Ok(matches)
    }

    /// Write the terminal state the request names, once, after committing it.
    async fn settle_projection(
        &self,
        source: &ResolvedSource,
        item: &str,
        claim: &mut ProjectionClaim,
        request: &HumanTaskProjection,
        task: &Task,
    ) -> Result<(), EngineError> {
        let resumed = claim.terminal_committed();
        claim
            .begin_terminal(request.state, request.reason.clone())
            .map_err(|error| projection_refused(request, &error))?;
        if claim.terminal_acknowledged() {
            return Ok(());
        }
        let binding = claim
            .binding()
            .cloned()
            .ok_or_else(|| refused_by(request, "no task is bound".to_owned(), "project it."))?;
        let id = binding.task().global_id().native;
        let mut update = terminal_update(request);
        let rewrite = self.admit_update(source, item, &id, &update).await?;
        rewrite.update(&mut update);
        // A terminal write whose answer was lost may have landed whole, in part or not at all:
        // the update then reads the task as it stands and writes only what still differs.
        if !resumed && let Some(context) = claim.written_context() {
            // The terminal state is the issue's own — its status and its closure — so the ids this
            // producer kept are all the write needs. A reason is recorded against the body as it
            // stands now, which the source reads before composing it — never against a copy of the
            // body this producer wrote.
            let mut held = task.clone();
            held.id.clone_from(&id);
            source.source().prime_written(&id, context, &held);
        }
        source
            .source()
            .update_task(&id, &update)
            .await
            .map_err(|error| source_failed(source, error))?
            .ok_or_else(|| {
                refused_by(
                    request,
                    format!("bound task {} is no longer in the tracker", binding.task()),
                    "inspect the tracker and reconcile the projection by hand.",
                )
            })?;
        claim
            .acknowledge_terminal()
            .map_err(|error| projection_refused(request, &error))
    }

    /// Hold a terminal update of a task already written to the destination's declaration and
    /// the caller's check, as every other narrow write is held.
    async fn admit_update(
        &self,
        source: &ResolvedSource,
        item: &str,
        id: &NativeId,
        update: &TaskUpdate,
    ) -> Result<super::boundary::Rewrite, EngineError> {
        let exposure = Exposure::of_update(update);
        self.preflight(
            source,
            &Outbound {
                item: item.to_owned(),
                classification: Classification::Public,
                target: WriteTarget::Existing(id),
                exposure: &exposure,
            },
        )
        .await
    }
}

/// The task a projection creates: open, with its identity, owner and origin in its metadata
/// so a recovery can verify it from the tracker alone.
fn projected_task(
    request: &HumanTaskProjection,
    project: &ProjectionProjectId,
    classification: Classification,
    claim: Option<&ProjectionClaim>,
) -> Task {
    let mut metadata = BTreeMap::new();
    metadata.insert(
        ITEM_ID_KEY.to_owned(),
        Value::from(request.item_id.as_str()),
    );
    metadata.insert(
        OWNER_KEY.to_owned(),
        serde_json::to_value(&request.owner).unwrap_or(Value::Null),
    );
    if let Some(claim) = claim {
        metadata.insert(GlobalId::ORIGIN_KEY.to_owned(), Value::from(claim.origin()));
        metadata.insert(DIGEST_KEY.to_owned(), Value::from(claim.payload_digest()));
        metadata.insert(HOST_KEY.to_owned(), Value::from(claim.host()));
    }
    Task {
        version: None,
        id: NativeId::from("human-task"),
        key: None,
        title: request.title.as_str().to_owned(),
        content: Some(request.body.clone()),
        status: Status {
            category: StatusCategory::Todo,
            name: "todo".to_owned(),
        },
        priority: request.priority,
        labels: Vec::new(),
        project: Some(project.native_id()),
        url: None,
        location: None,
        created_at: None,
        updated_at: None,
        metadata,
        repositories: request.repositories.clone(),
        delivers: Vec::new(),
        delivered_by: Vec::new(),
        classification,
        assignees: None,
    }
}

/// The category a terminal state lands in.
fn terminal_category(state: ProjectionState) -> StatusCategory {
    match state {
        ProjectionState::Completed => StatusCategory::Done,
        ProjectionState::Dismissed | ProjectionState::Open => StatusCategory::Cancelled,
    }
}

/// The one update a terminal projection writes: its status — which closes the issue as its
/// category says — and, only when the request gave one, its reason. An absent reason stays
/// absent.
fn terminal_update(request: &HumanTaskProjection) -> TaskUpdate {
    let category = terminal_category(request.state);
    let mut metadata_set = BTreeMap::new();
    if let Some(reason) = &request.reason {
        metadata_set.insert(
            MetadataKey::new(REASON_KEY.to_owned()).expect("a literal human key"),
            Value::from(reason.as_str()),
        );
    }
    TaskUpdate {
        status: Some(Status {
            category,
            name: serde_json::to_value(category)
                .ok()
                .and_then(|word| word.as_str().map(str::to_owned))
                .unwrap_or_default(),
        }),
        metadata_set,
        ..TaskUpdate::default()
    }
}

fn projection_refused(request: &HumanTaskProjection, error: &ProjectionError) -> EngineError {
    refused_by(
        request,
        error.to_string(),
        "inspect the tracker for the item's origin and reconcile its intent; no second create \
         was sent.",
    )
}

fn refused_by(request: &HumanTaskProjection, message: String, next: &str) -> EngineError {
    EngineError::ProjectionRefused {
        item_id: request.item_id.to_string(),
        message,
        next: next.to_owned(),
    }
}
