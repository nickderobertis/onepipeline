//! A plan graph created in one call: a project, the member tasks its template declares — each
//! rendered from its own task template — and the `blocks` edges between them.
//!
//! The members arrive rendered and ordered ([`crate::template::Template::prepare_members`]), so
//! every answer a member's template refuses was refused before this is called and nothing was
//! written. Here the project is created first, then each member under it in the order given,
//! each as [`Engine::create_task`] creates one — its provenance recorded exactly as that verb
//! records it — offered its declared id, and depending on the members it names by the ids their
//! source actually gave them.

use std::collections::{BTreeMap, HashMap};

use onetaskgraph_plugin_api::{MetadataKey, NativeId, Repository, SourceName};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::rendered::{Body, ProjectCreate, TaskCreate};
use super::{Engine, EngineError};
use crate::GlobalId;
use crate::template::TemplateError;

/// One member task of a plan graph, ready to create.
#[derive(Debug, Clone, PartialEq)]
pub struct PlanMember {
    /// The id it is offered to its source under, and how later members' `depends_on` name it.
    pub id: NativeId,
    /// Its title.
    pub title: String,
    /// Its body: a rendering of its own template.
    pub body: Body,
    /// The ids of earlier members it depends on, each a `blocks` edge.
    pub depends_on: Vec<NativeId>,
    /// The caller's own metadata keys.
    pub metadata: BTreeMap<MetadataKey, Value>,
    /// The repositories it concerns.
    pub repositories: Vec<Repository>,
}

/// What creating a plan graph came to: the project, and each member by the qualified id its
/// source gave it, in the order they were created.
#[derive(Debug, Clone, PartialEq)]
pub struct PlanCreated {
    /// The project.
    pub project: GlobalId,
    /// Each member, as its declared id beside the qualified id it was created under.
    pub members: Vec<(NativeId, GlobalId)>,
}

/// What `project create --dry-run` answers: the project and every member it would create, in
/// the order it would create them, and nothing written.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct PlannedProject {
    /// The source it would be created in.
    pub source: SourceName,
    /// The id it would be written under.
    pub id: NativeId,
    /// Its title.
    pub title: String,
    /// Its description, as it would be written.
    pub content: String,
    /// Each member, in the order it would be created: after every member it depends on.
    pub members: Vec<PlannedMember>,
}

/// One member `project create --dry-run` would create.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct PlannedMember {
    /// The id it would be offered to its source under.
    pub id: NativeId,
    /// Its title.
    pub title: String,
    /// The template its body is rendered from, as its provenance would record it; `null` for a
    /// body given as it is.
    pub template: Option<String>,
    /// Its body, as it would be written.
    pub content: String,
    /// The resolved answers its body was rendered with.
    pub answers: BTreeMap<String, Value>,
    /// The members it would depend on, each a `blocks` edge.
    pub depends_on: Vec<NativeId>,
    /// The caller's metadata keys it would carry.
    pub metadata: BTreeMap<String, Value>,
    /// The repositories it concerns.
    pub repositories: Vec<Repository>,
}

impl PlannedProject {
    /// What creating `project` and `members` would write, written nowhere.
    #[must_use]
    pub fn of(project: &ProjectCreate, members: &[PlanMember]) -> Self {
        Self {
            source: project.source.clone(),
            id: project.id.clone(),
            title: project.title.clone(),
            content: project.body.content().to_owned(),
            members: members
                .iter()
                .map(|member| PlannedMember {
                    id: member.id.clone(),
                    title: member.title.clone(),
                    template: member
                        .body
                        .provenance()
                        .map(|provenance| provenance.template.clone()),
                    content: member.body.content().to_owned(),
                    answers: member.body.answers().cloned().unwrap_or_default(),
                    depends_on: member.depends_on.clone(),
                    metadata: member
                        .metadata
                        .iter()
                        .map(|(key, value)| (key.as_str().to_owned(), value.clone()))
                        .collect(),
                    repositories: member.repositories.clone(),
                })
                .collect(),
        }
    }
}

impl Engine {
    /// Create `project`, then every one of `members` under it, in the order given.
    ///
    /// # Errors
    ///
    /// [`EngineError::Template`] — before anything is written — when a member depends on one
    /// that is not created before it; every refusal of [`Engine::create_project`]; and
    /// [`EngineError::PlanMember`], naming the member and every item already created, wrapping a
    /// member's refusal by [`Engine::create_task`] — nothing that landed is taken back.
    pub async fn create_plan(
        &self,
        project: &ProjectCreate,
        members: &[PlanMember],
    ) -> Result<PlanCreated, EngineError> {
        let mut earlier: Vec<&NativeId> = Vec::with_capacity(members.len());
        let refused = |message: String| EngineError::Template {
            error: TemplateError::Malformed {
                file: "members".to_owned(),
                key: Some("id".to_owned()),
                message,
            },
        };
        for member in members {
            if earlier.contains(&&member.id) {
                return Err(refused(format!("name the member {} twice", member.id)));
            }
            if let Some(later) = member
                .depends_on
                .iter()
                .find(|needed| !earlier.contains(needed))
            {
                return Err(EngineError::Template {
                    error: TemplateError::Malformed {
                        file: "members".to_owned(),
                        key: Some("depends_on".to_owned()),
                        message: format!(
                            "member {} depends on {later}, which is not a member created before \
                             it",
                            member.id
                        ),
                    },
                });
            }
            earlier.push(&member.id);
        }
        let created = self.create_project(project).await?;
        let mut landed: HashMap<&NativeId, GlobalId> = HashMap::new();
        let mut order = Vec::with_capacity(members.len());
        for member in members {
            let request = TaskCreate {
                source: project.source.clone(),
                project: created.id.native.clone(),
                title: member.title.clone(),
                body: member.body.clone(),
                status: None,
                labels: Vec::new(),
                repositories: member.repositories.clone(),
                depends_on: member
                    .depends_on
                    .iter()
                    .filter_map(|needed| landed.get(needed).cloned())
                    .collect(),
                delivers: Vec::new(),
                metadata: member.metadata.clone(),
                assets: Vec::new(),
                classification: onetaskgraph_plugin_api::Classification::default(),
                assignee: None,
            };
            let task = match self.create_task_as(&request, Some(&member.id)).await {
                Ok(task) => task,
                Err(error) => {
                    return Err(EngineError::PlanMember {
                        member: member.id.to_string(),
                        landed: std::iter::once(created.id.to_string())
                            .chain(
                                order
                                    .iter()
                                    .map(|(_, id): &(NativeId, GlobalId)| id.to_string()),
                            )
                            .collect(),
                        error: Box::new(error),
                    });
                }
            };
            landed.insert(&member.id, task.task.id.clone());
            order.push((member.id.clone(), task.task.id));
        }
        Ok(PlanCreated {
            project: created.id,
            members: order,
        })
    }
}
