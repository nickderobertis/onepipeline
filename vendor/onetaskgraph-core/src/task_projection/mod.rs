//! Producer-owned human task projection: the public wire declaration lives here.
//!
//! Requests identify an action item rather than a question. The same declaration is used
//! by the CLI and exported schemas so consumers need not maintain a parallel contract.

// llmlint: ignore-file[code_lands_in_the_domain_that_owns_it] Core domains here are crate submodules: config, cache, template and engine each live within onetaskgraph-core's single Nx project. Task projection follows that convention, owning its declaration and producer intent here while sharing the existing engine write machinery. A separate project for this one domain would impose a boundary the existing core domains do not have.
mod identity;
mod intent;

pub use identity::{
    ExternalItemId, HostIdentity, PlanId, ProjectedTaskId, ProjectionProjectId, ProjectionTitle,
    RunId, SessionId, SourceIdentityDigest, TrackerUrl,
};

pub use intent::{
    IntentPhase, IntentStore, PlacementCache, ProjectionClaim, ProjectionError, ProjectionPlacement,
};

use onetaskgraph_plugin_api::{Priority, Repository, SourceName};
use schemars::{JsonSchema, Schema, SchemaGenerator};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// The sole supported projection request version.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ProjectionVersion;

impl Serialize for ProjectionVersion {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_u32(1)
    }
}

impl<'de> Deserialize<'de> for ProjectionVersion {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        match u32::deserialize(deserializer)? {
            1 => Ok(Self),
            other => Err(serde::de::Error::custom(format!(
                "unsupported task projection schema_version {other}; expected 1"
            ))),
        }
    }
}

impl JsonSchema for ProjectionVersion {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "ProjectionVersion".into()
    }

    fn json_schema(_generator: &mut SchemaGenerator) -> Schema {
        serde_json::json!({"type": "integer", "const": 1})
            .try_into()
            .expect("a literal schema")
    }
}

/// The producer that owns the intent; nullable run and plan are explicit on the wire.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ProjectionOwner {
    /// The owning session.
    pub session: SessionId,
    /// The owning run, when applicable.
    #[serde(deserialize_with = "nullable")]
    #[schemars(required, schema_with = "nullable_string_schema")]
    pub run: Option<RunId>,
    /// The owning plan, when applicable.
    #[serde(deserialize_with = "nullable")]
    #[schemars(required, schema_with = "nullable_string_schema")]
    pub plan: Option<PlanId>,
}

/// The requested tracker lifecycle. Questions have no representation here.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum ProjectionState {
    /// The operator has work to do.
    Open,
    /// The operator completed the work.
    Completed,
    /// The operator dismissed the work, optionally recording a reason.
    Dismissed,
}

/// Version 1 of the general human task projection request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct HumanTaskProjection {
    /// The wire contract version, always 1.
    pub schema_version: ProjectionVersion,
    /// The external idempotency identity, stable across producer retries.
    pub item_id: ExternalItemId,
    /// The producer that owns the intent.
    pub owner: ProjectionOwner,
    /// The visible task title.
    pub title: ProjectionTitle,
    /// The visible task content.
    pub body: String,
    /// Its priority, using the existing plugin API serialization.
    pub priority: Priority,
    /// Normalized repository origins whose work this item concerns.
    pub repositories: Vec<Repository>,
    /// The plan's resolved destination project, or null for a configured human project.
    #[serde(deserialize_with = "nullable")]
    #[schemars(required, schema_with = "nullable_string_schema")]
    pub project: Option<ProjectionProjectId>,
    /// The configured source through which placement is resolved.
    pub source: SourceName,
    /// The desired lifecycle.
    pub state: ProjectionState,
    /// An optional dismissal reason. Omission and null both preserve absence.
    // llmlint: ignore[invalid_states_unrepresentable] The v1 wire declares `state` and `reason` as independent fields, and the exported schema both SDKs generate from is frozen; `Engine::project_human` refuses a reason beside any state but `dismissed` before anything is written, and a journey proves it.
    #[serde(default)]
    pub reason: Option<String>,
}

/// A durable projection's bound tracker identity, suitable for a directly clickable card.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct HumanTaskProjected {
    /// The qualified task identity.
    task: ProjectedTaskId,
    /// The canonical tracker issue URL.
    url: TrackerUrl,
    /// The resolved destination project.
    project: ProjectionProjectId,
    /// The resolved configured source.
    source: SourceName,
}

impl HumanTaskProjected {
    /// Bind a canonical issue URL and existing project to a qualified task identity.
    #[must_use]
    pub fn new(task: ProjectedTaskId, url: TrackerUrl, project: ProjectionProjectId) -> Self {
        let source = task.global_id().source;
        Self {
            task,
            url,
            project,
            source,
        }
    }

    /// The qualified task identity.
    #[must_use]
    pub fn task(&self) -> &ProjectedTaskId {
        &self.task
    }

    /// The canonical URL used directly by a tracker card.
    #[must_use]
    pub fn url(&self) -> &TrackerUrl {
        &self.url
    }

    /// The resolved existing project.
    #[must_use]
    pub fn project(&self) -> &ProjectionProjectId {
        &self.project
    }

    /// The source derived from the qualified task identity.
    #[must_use]
    pub fn source(&self) -> &SourceName {
        &self.source
    }
}

impl<'de> Deserialize<'de> for HumanTaskProjected {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let mut fields = serde_json::Map::<String, serde_json::Value>::deserialize(deserializer)?;
        fn take<T: serde::de::DeserializeOwned>(
            fields: &mut serde_json::Map<String, serde_json::Value>,
            name: &str,
        ) -> Result<T, String> {
            serde_json::from_value(
                fields
                    .remove(name)
                    .ok_or_else(|| format!("missing field {name}"))?,
            )
            .map_err(|error| format!("{name}: {error}"))
        }
        let task: ProjectedTaskId = take(&mut fields, "task").map_err(serde::de::Error::custom)?;
        let url = take(&mut fields, "url").map_err(serde::de::Error::custom)?;
        let project = take(&mut fields, "project").map_err(serde::de::Error::custom)?;
        let source: SourceName = take(&mut fields, "source").map_err(serde::de::Error::custom)?;
        if !fields.is_empty() {
            return Err(serde::de::Error::custom("unknown projection binding field"));
        }
        let bound = Self::new(task, url, project);
        if bound.source != source {
            return Err(serde::de::Error::custom(
                "binding source disagrees with qualified task",
            ));
        }
        Ok(bound)
    }
}

/// Require a nullable key to be present, distinguishing it from an optional reason.
fn nullable<'de, D: Deserializer<'de>, T: Deserialize<'de>>(
    deserializer: D,
) -> Result<Option<T>, D::Error> {
    Option::<T>::deserialize(deserializer)
}

/// One source's projection authority, as its `task_projection` configures it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectionSource {
    /// The digest naming the source's remote, shared with its local cache.
    pub identity: SourceIdentityDigest,
    /// The authoritative host.
    pub host: HostIdentity,
    /// The absolute directory holding its intents.
    pub directory: std::path::PathBuf,
    /// The project a planless item is filed under.
    pub project: Option<ProjectionProjectId>,
    /// Where its kept placement lives — beside the source's local cache — or `None` when this
    /// host names no cache home, so every create reads its placement.
    pub placement: Option<PlacementCache>,
}

/// Every configured source's projection authority, or why it cannot hold one.
pub(crate) fn configured(
    config: &crate::config::Config,
) -> std::collections::BTreeMap<SourceName, Result<ProjectionSource, String>> {
    config
        .sources()
        .iter()
        .filter_map(|(name, source)| {
            let declared = source.task_projection()?;
            let home = config
                .cache_home()
                .map(std::path::Path::to_path_buf)
                .or_else(crate::cache::process_home);
            let resolved = (|| {
                let identity = crate::cache::remote_digest(source).ok_or_else(|| {
                    format!(
                        "source {name} names no remote tracker, so its projections have no \
                         stable identity"
                    )
                })?;
                Ok(ProjectionSource {
                    identity: SourceIdentityDigest::new(identity)?,
                    host: declared.host.clone(),
                    directory: declared.directory.as_path().to_path_buf(),
                    project: declared.project.clone(),
                    placement: crate::cache::designator_location(source, home.as_deref()).map(
                        |(designator, _)| {
                            PlacementCache::at(designator.with_file_name("placement.json"))
                        },
                    ),
                })
            })();
            Some((name.clone(), resolved))
        })
        .collect()
}

/// The purpose selected by human task projection writers on existing route declarations.
#[must_use]
pub fn route_purpose() -> crate::config::RoutePurpose {
    crate::config::RoutePurpose::new("human-task").expect("a literal routing identity")
}

fn nullable_string_schema(_generator: &mut SchemaGenerator) -> Schema {
    serde_json::json!({"type": ["string", "null"]})
        .try_into()
        .expect("a literal nullable string schema")
}

/// The schema bundle registers this domain once; new roots stay beside their declaration.
pub(crate) fn schemas() -> [(&'static str, Schema); 2] {
    [
        (
            "HumanTaskProjection",
            schemars::schema_for!(HumanTaskProjection),
        ),
        (
            "HumanTaskProjected",
            crate::schema::write_answer_schema::<HumanTaskProjected>(),
        ),
    ]
}
