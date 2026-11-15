//! Relations between tasks: a task's parent, and typed links between two items.
//!
//! A **parent** is a task of the child's own source, many-to-one and multi-level, held where
//! the source holds it — a GitHub sub-issue under `sub_issues: parent`, a Linear `parentId`, a
//! stored field on a folder of Markdown or an in-memory source — and written through
//! [`TaskSource::set_parents`], one answer per edge.
//!
//! A **link** is a type name over a directed pair of items. Its authority on every plugin is
//! the store's own metadata on both endpoints, under [`TaskLink::METADATA_KEY`], beside a
//! `## Links` section rendered into the item's content and stripped on every read; all of one
//! item's links in a call go in one targeted update ([`TaskSource::update_tasks`]), held to the
//! version this call read it at. A type a source's `link_types` mirrors natively is also
//! written as the destination's own relation, between two tasks of that source, and removed
//! only with the last mirrored link between that pair.
//!
//! The documents read and answered here are declared once, in this module, and published as
//! roots of the schema bundle, so both SDKs model them from these types.

use std::collections::{BTreeMap, BTreeSet};

use onetaskgraph_plugin_api::{
    BatchAnswer, BatchUpdate, LinkDirection, LinkTypes, Location, MetadataKey, NativeId,
    NativeLink, ParentAnswer, ParentWrite, Relations, SourceError, SourceName, TaskLink,
    TaskUpdate, with_links_section,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::batch::one_each;
use super::delivery::source_failed;
use super::{Engine, EngineError};
use crate::GlobalId;

/// The one version of every relation document this module reads and answers.
pub const RELATIONS_SCHEMA_VERSION: u32 = 1;

/// What `task parent set-many` reads: many parent writes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ParentBatch {
    /// `1`.
    pub schema_version: u32,
    /// The edges, each written on its own.
    pub edges: Vec<ParentEdge>,
}

/// One edge of a [`ParentBatch`]: `child`'s parent set to `parent`, or removed for `null`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ParentEdge {
    /// The child's qualified id, `<source>:<native-id>`.
    // llmlint: ignore[invalid_states_unrepresentable] Read as text so a malformed id is that edge's own `error`, naming it, rather than a refusal of the whole batch; `Engine::set_parents` parses it as a `GlobalId` before anything is sent.
    pub child: String,
    /// The parent's qualified id, a task of the child's own source, or `null` to remove the
    /// child's parent. Required: an edge leaving it out is refused rather than read as a removal.
    // llmlint: ignore[invalid_states_unrepresentable] As `child`.
    #[serde(deserialize_with = "nullable")]
    #[schemars(with = "RequiredNullable")]
    pub parent: Option<String>,
    /// The version the child must be at for anything to be written, as a read reported it.
    #[serde(default)]
    pub if_version: Option<String>,
}

/// The schema of [`ParentEdge::parent`]: a string or `null`, and required — which a derive over
/// `Option<String>` would publish as optional, so both SDKs' models would read an edge leaving
/// it out as a removal the deserializer refuses.
struct RequiredNullable;

impl JsonSchema for RequiredNullable {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "RequiredNullable".into()
    }

    fn inline_schema() -> bool {
        true
    }

    fn json_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
        schemars::json_schema!({"type": ["string", "null"]})
    }
}

/// What `task parent set-many` answers: one result per edge, in the order given.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ParentsSet {
    /// `1`.
    pub schema_version: u32,
    /// One per edge.
    pub results: Vec<ParentResult>,
}

/// What became of one edge of a [`ParentBatch`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ParentResult {
    // llmlint: ignore-block[invalid_states_unrepresentable] The shape is the contract the task fixes literally — `{child, parent, outcome, version?, reason?}` with `outcome` a tag beside flat members — which other nodes of the plan build against; every result is made in this module, each setting the members its outcome carries.
    /// The child as the batch named it.
    pub child: String,
    /// The parent as the batch named it, `null` for a removal.
    pub parent: Option<String>,
    /// What became of it.
    pub outcome: ParentOutcome,
    /// The child's version: after the write when `written` and the write's answer says, as read
    /// when `unchanged`, and the version it was found at when `version-moved`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    /// Why it is answered `refused` or `error`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    // llmlint: ignore-end[invalid_states_unrepresentable]
}

/// What became of one parent edge.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum ParentOutcome {
    /// It was written.
    Written,
    /// The child already had that parent; nothing was written.
    Unchanged,
    /// The child was at another version than the one it was held to; nothing was written.
    VersionMoved,
    /// The edge was refused on its own — a cycle, a parent in another source, or a limit of
    /// the destination on children or depth — and nothing was written.
    Refused,
    /// It could not be written; `reason` says why.
    Error,
}

impl ParentOutcome {
    /// Whether this edge ended as its caller asked.
    #[must_use]
    pub const fn settled(self) -> bool {
        matches!(self, Self::Written | Self::Unchanged)
    }
}

/// What `task link add-many` and `task link remove-many` read: many typed links.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LinkBatch {
    /// `1`.
    pub schema_version: u32,
    /// The links, each added or removed on its own.
    pub links: Vec<LinkItem>,
}

/// One link of a [`LinkBatch`]: a type name over a directed pair of items.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LinkItem {
    /// The link's `from`, a qualified task id.
    // llmlint: ignore[invalid_states_unrepresentable] As `ParentEdge::child`: read as text so a malformed id is that link's own `error`.
    pub from: String,
    /// The link's `to`, a qualified task id of any configured source.
    // llmlint: ignore[invalid_states_unrepresentable] As `from`.
    pub to: String,
    /// The link's type name.
    // llmlint: ignore[invalid_states_unrepresentable] As `from`: read as text so an empty name is that link's own `error`; `checked_link` turns it into a `LinkTypeName` before anything is read.
    #[serde(rename = "type")]
    pub link_type: String,
}

/// What `task link add-many` and `task link remove-many` answer: one result per link.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct LinksWritten {
    /// `1`.
    pub schema_version: u32,
    /// One per link, in the order given.
    pub results: Vec<LinkResult>,
}

/// What became of one link of a [`LinkBatch`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct LinkResult {
    // llmlint: ignore-block[invalid_states_unrepresentable] The shape is the contract the task fixes literally — `{from, to, type, outcome, from_version?, to_version?, reason?}` — which other nodes of the plan build against; every result is made in this module.
    /// The `from` as the batch named it.
    pub from: String,
    /// The `to` as the batch named it.
    pub to: String,
    /// The link's type name.
    #[serde(rename = "type")]
    pub link_type: String,
    /// What became of it.
    pub outcome: LinkOutcome,
    /// The `from` endpoint's version — after this call's write of it, else as read.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from_version: Option<String>,
    /// The `to` endpoint's version — after this call's write of it, else as read.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to_version: Option<String>,
    /// Why it is answered `partial` — naming the endpoint that landed — or `error`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    // llmlint: ignore-end[invalid_states_unrepresentable]
}

/// What became of one link.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum LinkOutcome {
    /// Every endpoint it needed was written.
    Written,
    /// An add of a link both endpoints already hold; nothing was written.
    Exists,
    /// A removal of a link neither endpoint holds; nothing was written.
    Absent,
    /// One endpoint landed and the other did not; `reason` names the one that landed, and
    /// repeating the call completes the other.
    Partial,
    /// Nothing of it was written; `reason` says why.
    Error,
}

impl LinkOutcome {
    /// Whether this link ended as its caller asked.
    #[must_use]
    pub const fn settled(self) -> bool {
        matches!(self, Self::Written | Self::Exists | Self::Absent)
    }
}

/// What `task relations` answers: one item per id asked, in the order asked.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct TaskRelations {
    /// `1`.
    pub schema_version: u32,
    /// One per id.
    pub items: Vec<RelationsItem>,
}

/// One task's relations.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct RelationsItem {
    /// The task's qualified id.
    pub item: GlobalId,
    /// Its parent's qualified id, or `null`.
    pub parent: Option<GlobalId>,
    /// Its children's qualified ids.
    pub children: Vec<GlobalId>,
    /// Whether `children` is every child.
    pub children_complete: bool,
    /// Its links, as it holds them.
    pub links: Vec<ReadLink>,
    /// Whether `links` is every link it holds.
    pub links_complete: bool,
}

/// One link as one endpoint holds it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ReadLink {
    /// The link's type name.
    // llmlint: ignore[invalid_states_unrepresentable] The type name as the endpoint stores it under the contract's `TaskLink`, whose `type` is the caller's own word; a stored name this engine never wrote is reported as stored rather than refused, because a read writes nothing and answers what each end holds.
    #[serde(rename = "type")]
    pub link_type: String,
    /// Which end of the link this item is.
    pub direction: LinkDirection,
    /// The other item's qualified id.
    pub other: GlobalId,
    /// Whether the other end holds this link too: `present` or `missing` when the other item
    /// is among the ids asked, and `unread` otherwise.
    pub other_end: OtherEnd,
}

/// Whether a link's other end holds it too.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum OtherEnd {
    /// The other end holds the same link.
    Present,
    /// The other end was read and does not hold it — a whole-body rewrite there lost it.
    Missing,
    /// The other end is not among the ids asked, so it was not read.
    Unread,
}

/// The relation calls' schema roots, in the order the bundle publishes them.
pub(crate) fn schemas() -> [(&'static str, schemars::Schema); 11] {
    use crate::schema::write_answer_schema;
    use schemars::schema_for;
    [
        ("ParentBatch", schema_for!(ParentBatch)),
        ("ParentsSet", write_answer_schema::<ParentsSet>()),
        ("ParentOutcome", schema_for!(ParentOutcome)),
        ("LinkBatch", schema_for!(LinkBatch)),
        ("LinksWritten", write_answer_schema::<LinksWritten>()),
        ("LinkOutcome", schema_for!(LinkOutcome)),
        ("TaskRelations", schema_for!(TaskRelations)),
        ("RelationsItem", schema_for!(RelationsItem)),
        ("ReadLink", schema_for!(ReadLink)),
        ("OtherEnd", schema_for!(OtherEnd)),
        ("TaskLink", schema_for!(TaskLink)),
    ]
}

/// A relation document of another schema version than this engine reads, refused whole.
fn check_version(what: &str, found: u32) -> Result<(), EngineError> {
    if found == RELATIONS_SCHEMA_VERSION {
        return Ok(());
    }
    Err(EngineError::BatchDocument {
        what: what.to_owned(),
        why: format!(
            "its schema_version is {found}, and this release reads {RELATIONS_SCHEMA_VERSION}"
        ),
    })
}

/// `text` as a qualified task id, or why it is not one.
fn qualified(text: &str, what: &str) -> Result<GlobalId, String> {
    text.parse::<GlobalId>().map_err(|error| {
        format!("{what} {text:?} is not a qualified task id, `<source>:<native-id>`: {error}")
    })
}

/// An unordered pair of tasks, spelled one way.
type Pair = (GlobalId, GlobalId);

/// The native word a pair is mirrored as, and the links of a call that need it.
type Mirrored = (onetaskgraph_plugin_api::NativeWord, Vec<usize>);

/// What one read of endpoints answered for one id.
type Read = Result<Relations, String>;

/// One endpoint of a link call: as read, and as this call leaves its links.
struct Endpoint {
    read: Relations,
    before: Vec<TaskLink>,
    after: Vec<TaskLink>,
}

/// Whether a link call adds its links or removes them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkOp {
    /// `task link add-many`.
    Add,
    /// `task link remove-many`.
    Remove,
}

/// A link type name a call has checked: never empty, without surrounding whitespace, and
/// holding no line break or other control character — the `## Links` section lists each link
/// on one line, and its reader recognises only those lines, so a name spanning two would leave
/// part of the section in the content every read answers.
#[derive(Debug, Clone, PartialEq, Eq)]
struct LinkTypeName(String);

impl LinkTypeName {
    /// `name` checked by [`LinkTypes::type_name`], the rule a source's `link_types` is held to
    /// as well, or why it names no type a link can carry.
    fn new(name: &str) -> Result<Self, &'static str> {
        LinkTypes::type_name(name).map(|name| Self(name.to_owned()))
    }
}

/// One link of a call, checked.
struct Planned {
    from: GlobalId,
    to: GlobalId,
    link_type: LinkTypeName,
}

/// What a link call decided before anything is written.
enum Decision {
    /// Answered without a write.
    Answered(LinkOutcome, Option<String>),
    /// Writes these endpoints.
    Writes(Vec<GlobalId>),
}

impl Engine {
    /// The relations of each task `ids` names, in the order asked: its parent, its children
    /// and its links — each link's `other_end` compared with the other item when that item is
    /// among `ids`, and `unread` otherwise, so the comparison costs no request.
    ///
    /// # Errors
    ///
    /// [`EngineError::UnknownSource`] and its siblings for a source not usable here,
    /// [`EngineError::SourceFailed`] when a source could not read, or holds no relations,
    /// [`EngineError::NoSuchTask`] for an id naming no task, and
    /// [`EngineError::RelationsIncomplete`] under `complete` when any item's children or links
    /// were not read whole.
    pub async fn task_relations(
        &self,
        ids: &[GlobalId],
        complete: bool,
    ) -> Result<TaskRelations, EngineError> {
        let wanted: BTreeSet<GlobalId> = ids.iter().cloned().collect();
        let mut read: BTreeMap<GlobalId, Relations> = BTreeMap::new();
        for (name, natives) in by_source(&wanted) {
            let source = self.built(&name)?;
            let answers = source.source().read_relations(&natives).await;
            let answers = answers_each(source, &natives, answers)?;
            for (native, answer) in natives.into_iter().zip(answers) {
                let id = GlobalId::new(name.clone(), native);
                let relations = answer
                    .map_err(|error| source_failed(source, error))?
                    .ok_or_else(|| EngineError::NoSuchTask { id: id.to_string() })?;
                read.insert(id, relations);
            }
        }
        let mut items = Vec::with_capacity(ids.len());
        let mut incomplete = Vec::new();
        for id in ids {
            let relations = &read[id];
            let held = stored_links(id, &relations.task.metadata).map_err(|why| {
                EngineError::SourceFailed {
                    name: id.source.to_string(),
                    error: SourceError::Malformed { message: why },
                }
            })?;
            let links = held
                .into_iter()
                .map(|(link, other)| {
                    let other_end = match read.get(&other) {
                        None => OtherEnd::Unread,
                        Some(other) => {
                            let mirror = TaskLink {
                                link_type: link.link_type.clone(),
                                direction: opposite(link.direction),
                                other: id.to_string(),
                            };
                            if TaskLink::held(&other.task.metadata)
                                .unwrap_or_default()
                                .contains(&mirror)
                            {
                                OtherEnd::Present
                            } else {
                                OtherEnd::Missing
                            }
                        }
                    };
                    ReadLink {
                        link_type: link.link_type,
                        direction: link.direction,
                        other,
                        other_end,
                    }
                })
                .collect();
            if !relations.children_complete {
                incomplete.push(id.to_string());
            }
            items.push(RelationsItem {
                item: id.clone(),
                parent: relations
                    .parent
                    .clone()
                    .map(|parent| GlobalId::new(id.source.clone(), parent)),
                children: relations
                    .children
                    .iter()
                    .map(|child| GlobalId::new(id.source.clone(), child.clone()))
                    .collect(),
                children_complete: relations.children_complete,
                links,
                links_complete: true,
            });
        }
        if complete && !incomplete.is_empty() {
            return Err(EngineError::RelationsIncomplete {
                items: incomplete.join(", "),
            });
        }
        Ok(TaskRelations {
            schema_version: RELATIONS_SCHEMA_VERSION,
            items,
        })
    }

    /// Every parent edge `batch` names, each written on its own and answered in the order
    /// given.
    ///
    /// A malformed id, a parent in another source than its child's and a child named as its
    /// own parent are each that edge's answer, decided before anything is sent; every other
    /// edge goes to its source in one call per source ([`TaskSource::set_parents`]), whose
    /// answer for one edge — a cycle or a destination limit refused, a version moved — never
    /// writes or refuses another.
    ///
    /// # Errors
    ///
    /// [`EngineError::BatchDocument`] for a batch of another schema version.
    ///
    /// [`TaskSource::set_parents`]: onetaskgraph_plugin_api::TaskSource::set_parents
    pub async fn set_parents(&self, batch: &ParentBatch) -> Result<ParentsSet, EngineError> {
        check_version("the parent batch", batch.schema_version)?;
        let mut results: Vec<Option<ParentResult>> = batch.edges.iter().map(|_| None).collect();
        let mut by_name: BTreeMap<SourceName, Vec<(usize, ParentWrite)>> = BTreeMap::new();
        for (at, edge) in batch.edges.iter().enumerate() {
            match checked_edge(edge) {
                Ok((source, write)) => by_name.entry(source).or_default().push((at, write)),
                Err((outcome, reason)) => {
                    results[at] = Some(parent_result(edge, outcome, None, Some(reason)))
                }
            }
        }
        for (name, writes) in by_name {
            let source = match self.built(&name) {
                Ok(source) => source,
                Err(error) => {
                    for (at, _) in &writes {
                        results[*at] = Some(parent_result(
                            &batch.edges[*at],
                            ParentOutcome::Error,
                            None,
                            Some(error.to_string()),
                        ));
                    }
                    continue;
                }
            };
            let sent: Vec<ParentWrite> = writes.iter().map(|(_, write)| write.clone()).collect();
            let answers = one_each(&name, sent.len(), source.source().set_parents(&sent).await);
            for ((at, write), answer) in writes.into_iter().zip(answers) {
                let edge = &batch.edges[at];
                results[at] = Some(match answer {
                    Ok(ParentAnswer::Written { version }) => {
                        parent_result(edge, ParentOutcome::Written, version, None)
                    }
                    Ok(ParentAnswer::Unchanged { version }) => {
                        parent_result(edge, ParentOutcome::Unchanged, version, None)
                    }
                    Ok(ParentAnswer::VersionMoved { found }) => {
                        parent_result(edge, ParentOutcome::VersionMoved, found, None)
                    }
                    Ok(ParentAnswer::Refused { reason }) => {
                        parent_result(edge, ParentOutcome::Refused, None, Some(reason))
                    }
                    Ok(ParentAnswer::Missing { what }) => parent_result(
                        edge,
                        ParentOutcome::Error,
                        None,
                        Some(format!(
                            "{name} holds no task {}, so nothing was written for {}",
                            GlobalId::new(name.clone(), what),
                            GlobalId::new(name.clone(), write.child.clone())
                        )),
                    ),
                    Err(error) => parent_result(
                        edge,
                        ParentOutcome::Error,
                        None,
                        Some(format!(
                            "{name} refused the parent of {}: {error}",
                            edge.child
                        )),
                    ),
                });
            }
        }
        Ok(ParentsSet {
            schema_version: RELATIONS_SCHEMA_VERSION,
            results: results.into_iter().flatten().collect(),
        })
    }

    /// Add every link `batch` names, each answered on its own in the order given.
    ///
    /// # Errors
    ///
    /// [`EngineError::BatchDocument`] for a batch of another schema version.
    pub async fn add_links(&self, batch: &LinkBatch) -> Result<LinksWritten, EngineError> {
        check_version("the link batch", batch.schema_version)?;
        self.write_links(batch, LinkOp::Add).await
    }

    /// Remove every link `batch` names, each answered on its own in the order given.
    ///
    /// # Errors
    ///
    /// [`EngineError::BatchDocument`] for a batch of another schema version.
    pub async fn remove_links(&self, batch: &LinkBatch) -> Result<LinksWritten, EngineError> {
        check_version("the link batch", batch.schema_version)?;
        self.write_links(batch, LinkOp::Remove).await
    }

    /// The relations of every id in `ids`, read in one call per source; an id a source cannot
    /// answer for carries why.
    async fn read_endpoints(&self, ids: &BTreeSet<GlobalId>) -> BTreeMap<GlobalId, Read> {
        let mut read = BTreeMap::new();
        for (name, natives) in by_source(ids) {
            let answers: Vec<Read> = match self.built(&name) {
                Err(error) => natives.iter().map(|_| Err(error.to_string())).collect(),
                // Each id is answered on its own, so one the source could not read is that
                // id's error and never another's.
                Ok(source) => match answers_each(
                    source,
                    &natives,
                    source.source().read_relations(&natives).await,
                ) {
                    Err(error) => natives.iter().map(|_| Err(error.to_string())).collect(),
                    Ok(answers) => natives
                        .iter()
                        .zip(answers)
                        .map(|(native, answer)| match answer {
                            Err(error) => Err(format!("{name} could not read it: {error}")),
                            Ok(None) => Err(format!(
                                "{name} holds no task {}",
                                GlobalId::new(name.clone(), native.clone())
                            )),
                            Ok(Some(relations)) => Ok(relations),
                        })
                        .collect(),
                },
            };
            for (native, answer) in natives.into_iter().zip(answers) {
                read.insert(GlobalId::new(name.clone(), native), answer);
            }
        }
        read
    }

    async fn write_links(
        &self,
        batch: &LinkBatch,
        op: LinkOp,
    ) -> Result<LinksWritten, EngineError> {
        let mut results: Vec<Option<LinkResult>> = batch.links.iter().map(|_| None).collect();
        let mut planned: Vec<(usize, Planned)> = Vec::new();
        for (at, item) in batch.links.iter().enumerate() {
            match checked_link(item) {
                Ok(link) => planned.push((at, link)),
                Err(reason) => {
                    results[at] = Some(link_result(
                        item,
                        LinkOutcome::Error,
                        None,
                        None,
                        Some(reason),
                    ));
                }
            }
        }
        let wanted: BTreeSet<GlobalId> = planned
            .iter()
            .flat_map(|(_, link)| [link.from.clone(), link.to.clone()])
            .collect();
        let mut read = self.read_endpoints(&wanted).await;
        // Each endpoint read, with the links it holds; one whose links cannot be read is an
        // error of every link naming it.
        let mut endpoints: BTreeMap<GlobalId, Endpoint> = BTreeMap::new();
        let mut unusable: BTreeMap<GlobalId, String> = BTreeMap::new();
        for (id, answer) in std::mem::take(&mut read) {
            match answer.and_then(|relations| {
                let held = stored_links(&id, &relations.task.metadata)
                    .map_err(|why| format!("{id} cannot be linked: {why}"))?
                    .into_iter()
                    .map(|(link, _)| link)
                    .collect::<Vec<_>>();
                Ok((relations, held))
            }) {
                Ok((relations, held)) => {
                    endpoints.insert(
                        id,
                        Endpoint {
                            read: relations,
                            before: held.clone(),
                            after: held,
                        },
                    );
                }
                Err(why) => {
                    unusable.insert(id, why);
                }
            }
        }
        let mut usable: Vec<(usize, Planned)> = Vec::new();
        for (at, link) in planned {
            let failed: Vec<String> = [&link.from, &link.to]
                .into_iter()
                .filter_map(|id| unusable.get(id).cloned())
                .collect();
            if failed.is_empty() {
                usable.push((at, link));
            } else {
                results[at] = Some(link_result(
                    &batch.links[at],
                    LinkOutcome::Error,
                    None,
                    None,
                    Some(failed.join("; ")),
                ));
            }
        }
        // Mirror first: a native relation the destination refuses leaves that link unwritten
        // in metadata too, so repeating the call tries it again.
        let mut decisions = plan(&mut endpoints, &usable, op);
        let native_failed = self.mirror(&endpoints, &usable, &decisions, op).await;
        if !native_failed.is_empty() {
            for endpoint in endpoints.values_mut() {
                endpoint.after = endpoint.before.clone();
            }
            let kept: Vec<(usize, Planned)> = usable
                .into_iter()
                .filter(|(at, _)| !native_failed.contains_key(at))
                .collect();
            decisions = plan(&mut endpoints, &kept, op);
            for (at, why) in &native_failed {
                results[*at] = Some(link_result(
                    &batch.links[*at],
                    LinkOutcome::Error,
                    None,
                    None,
                    Some(why.clone()),
                ));
            }
            usable = kept;
        }
        let written = self.write_endpoints(&mut endpoints).await;
        for (at, link) in usable {
            let version = |id: &GlobalId| match written.get(id) {
                Some(Ok(version)) => version.clone(),
                _ => endpoints
                    .get(id)
                    .and_then(|end| end.read.task.version.clone()),
            };
            let (from_version, to_version) = (version(&link.from), version(&link.to));
            let item = &batch.links[at];
            results[at] = Some(match decisions.remove(&at) {
                Some(Decision::Answered(outcome, reason)) => {
                    link_result(item, outcome, from_version, to_version, reason)
                }
                Some(Decision::Writes(needed)) => {
                    let landed: Vec<&GlobalId> = needed
                        .iter()
                        .filter(|id| matches!(written.get(*id), Some(Ok(_))))
                        .collect();
                    let failures: Vec<String> = needed
                        .iter()
                        .filter_map(|id| match written.get(id) {
                            Some(Err(why)) => Some(why.clone()),
                            Some(Ok(_)) => None,
                            None => Some(format!("{id} was not written")),
                        })
                        .collect();
                    if failures.is_empty() {
                        link_result(item, LinkOutcome::Written, from_version, to_version, None)
                    } else if landed.is_empty() {
                        link_result(
                            item,
                            LinkOutcome::Error,
                            from_version,
                            to_version,
                            Some(failures.join("; ")),
                        )
                    } else {
                        let landed = landed
                            .iter()
                            .map(ToString::to_string)
                            .collect::<Vec<_>>()
                            .join(", ");
                        link_result(
                            item,
                            LinkOutcome::Partial,
                            from_version,
                            to_version,
                            Some(format!(
                                "{landed} landed, and the other end did not: {}; repeat the \
                                 call to complete it",
                                failures.join("; ")
                            )),
                        )
                    }
                }
                None => link_result(item, LinkOutcome::Error, from_version, to_version, None),
            });
        }
        Ok(LinksWritten {
            schema_version: RELATIONS_SCHEMA_VERSION,
            results: results.into_iter().flatten().collect(),
        })
    }

    /// Send the native relations `decisions` call for — each mirrored link written between two
    /// tasks of one source adds its pair's relation, and each pair a removal leaves with no
    /// mirrored link loses it — answering the links whose relation the destination refused,
    /// by position.
    async fn mirror(
        &self,
        endpoints: &BTreeMap<GlobalId, Endpoint>,
        usable: &[(usize, Planned)],
        decisions: &BTreeMap<usize, Decision>,
        op: LinkOp,
    ) -> BTreeMap<usize, String> {
        // By source: each unordered pair once, with the word it is mirrored as and the links
        // of this call that need it.
        let mut wanted: BTreeMap<SourceName, BTreeMap<Pair, Mirrored>> = BTreeMap::new();
        for (at, link) in usable {
            if link.from.source != link.to.source {
                continue;
            }
            if !matches!(decisions.get(at), Some(Decision::Writes(_))) {
                continue;
            }
            let Ok(source) = self.built(&link.from.source) else {
                continue;
            };
            let types = source.source().link_types();
            let Some(word) = types.native(&link.link_type.0) else {
                continue;
            };
            let pair = ordered(&link.from, &link.to);
            if op == LinkOp::Remove {
                // Removed only with the last mirrored link between the pair.
                let remains = [&link.from, &link.to].into_iter().any(|end| {
                    endpoints.get(end).is_some_and(|endpoint| {
                        endpoint.after.iter().any(|held| {
                            types.native(&held.link_type).is_some()
                                && held
                                    .other
                                    .parse::<GlobalId>()
                                    .is_ok_and(|other| ordered(end, &other) == pair)
                        })
                    })
                });
                if remains {
                    continue;
                }
            }
            wanted
                .entry(link.from.source.clone())
                .or_default()
                .entry(pair)
                .or_insert_with(|| (word.clone(), Vec::new()))
                .1
                .push(*at);
        }
        let mut failed: BTreeMap<usize, String> = BTreeMap::new();
        for (name, pairs) in wanted {
            let Ok(source) = self.built(&name) else {
                continue;
            };
            let entries: Vec<(Pair, Mirrored)> = pairs.into_iter().collect();
            let sent: Vec<NativeLink> = entries
                .iter()
                .map(|((a, b), (word, ats))| {
                    // The relation follows the first link of the call naming the pair.
                    let first = &usable
                        .iter()
                        .find(|(at, _)| *at == ats[0])
                        .map(|(_, link)| link);
                    let (from, to) = first.map_or((a, b), |link| (&link.from, &link.to));
                    NativeLink {
                        from: from.native.clone(),
                        to: to.native.clone(),
                        native: word.clone(),
                    }
                })
                .collect();
            let answers = if op == LinkOp::Add {
                source.source().add_native_links(&sent).await
            } else {
                source.source().remove_native_links(&sent).await
            };
            let answers = one_each(&name, sent.len(), answers);
            for ((pair, (word, ats)), answer) in entries.into_iter().zip(answers) {
                if let Err(error) = answer {
                    let why = format!(
                        "{name} refused the native `{word}` relation between {} and {}: {error}",
                        pair.0, pair.1
                    );
                    for at in ats {
                        failed.insert(at, why.clone());
                    }
                }
            }
        }
        failed
    }

    /// Write every endpoint whose links this call changed — each in one targeted update, held
    /// to the version this call read it at, in one call per source — answering each written
    /// endpoint's version after the write, or why it was not written.
    async fn write_endpoints(
        &self,
        endpoints: &mut BTreeMap<GlobalId, Endpoint>,
    ) -> BTreeMap<GlobalId, Result<Option<String>, String>> {
        let mut answers: BTreeMap<GlobalId, Result<Option<String>, String>> = BTreeMap::new();
        let changed: Vec<GlobalId> = endpoints
            .iter()
            .filter(|(_, endpoint)| endpoint.after != endpoint.before)
            .map(|(id, _)| id.clone())
            .collect();
        if changed.is_empty() {
            return answers;
        }
        // The other end of every link a changed endpoint will hold, for its `## Links` line.
        let others: BTreeSet<GlobalId> = changed
            .iter()
            .flat_map(|id| endpoints[id].after.iter())
            .filter_map(|link| link.other.parse::<GlobalId>().ok())
            .filter(|other| !endpoints.contains_key(other))
            .collect();
        let far: BTreeMap<GlobalId, Relations> = if others.is_empty() {
            BTreeMap::new()
        } else {
            self.read_endpoints(&others)
                .await
                .into_iter()
                .filter_map(|(id, read)| read.ok().map(|read| (id, read)))
                .collect()
        };
        let mut by_name: BTreeMap<SourceName, Vec<(GlobalId, BatchUpdate)>> = BTreeMap::new();
        for id in changed {
            let endpoint = &endpoints[&id];
            let bullets: Vec<String> = endpoint
                .after
                .iter()
                .map(|link| {
                    let other = link.other.parse::<GlobalId>().ok();
                    let shown = other.as_ref().and_then(|other| {
                        endpoints
                            .get(other)
                            .map(|end| &end.read)
                            .or_else(|| far.get(other))
                    });
                    bullet(link, shown)
                })
                .collect();
            let content = endpoint.read.task.content.clone().unwrap_or_default();
            let mut update = TaskUpdate {
                content: Some(with_links_section(&content, &bullets)),
                ..TaskUpdate::default()
            };
            if endpoint.after.is_empty() {
                update.metadata_remove.insert(MetadataKey::links());
            } else {
                update.metadata_set.insert(
                    MetadataKey::links(),
                    serde_json::to_value(&endpoint.after).unwrap_or_default(),
                );
            }
            let source = match self.built(&id.source) {
                Ok(source) => source,
                Err(error) => {
                    answers.insert(id, Err(error.to_string()));
                    continue;
                }
            };
            let update = match self.prepared_update(source, &id, &update).await {
                Ok(update) => update,
                Err(error) => {
                    answers.insert(id, Err(error.to_string()));
                    continue;
                }
            };
            by_name.entry(id.source.clone()).or_default().push((
                id.clone(),
                BatchUpdate {
                    id: id.native.clone(),
                    update,
                    if_version: endpoint.read.task.version.clone(),
                    when_priority: Vec::new(),
                    when_assignees: None,
                },
            ));
        }
        for (name, writes) in by_name {
            let Ok(source) = self.built(&name) else {
                continue;
            };
            let sent: Vec<BatchUpdate> = writes.iter().map(|(_, each)| each.clone()).collect();
            let written = one_each(&name, sent.len(), source.source().update_tasks(&sent).await);
            for ((id, _), answer) in writes.into_iter().zip(written) {
                let answer = match answer {
                    // Written only when the answer is about the task this end sent.
                    Ok(BatchAnswer::Written(outcome)) if outcome.task.id == id.native => {
                        Ok(outcome.task.version)
                    }
                    Ok(BatchAnswer::Written(outcome)) => Err(format!(
                        "{name} answered the write of {id} with the task {}, so whether its end \
                         was written is unknown",
                        outcome.task.id
                    )),
                    Ok(BatchAnswer::VersionMoved { found }) => Err(format!(
                        "{id} moved to version {} after it was read, so its end was not written",
                        found.unwrap_or_else(|| "unknown".to_owned())
                    )),
                    Ok(BatchAnswer::Missing) => Err(format!("{name} holds no task {id}")),
                    Ok(BatchAnswer::PriorityHeld(_) | BatchAnswer::AssigneesHeld(_)) => {
                        Err(format!("{id} was held by a guard this call never sets"))
                    }
                    Err(error) => Err(format!("{name} refused the write of {id}: {error}")),
                };
                answers.insert(id, answer);
            }
        }
        answers
    }
}

/// Plan `links` over `endpoints`, applying each to the links its endpoints will hold, and
/// deciding what each link comes to before anything is written.
fn plan(
    endpoints: &mut BTreeMap<GlobalId, Endpoint>,
    links: &[(usize, Planned)],
    op: LinkOp,
) -> BTreeMap<usize, Decision> {
    let mut decisions = BTreeMap::new();
    for (at, link) in links {
        let out = TaskLink {
            link_type: link.link_type.0.clone(),
            direction: LinkDirection::Out,
            other: link.to.to_string(),
        };
        let inward = TaskLink {
            link_type: link.link_type.0.clone(),
            direction: LinkDirection::In,
            other: link.from.to_string(),
        };
        let mut needed = Vec::new();
        for (id, entry) in [(&link.from, &out), (&link.to, &inward)] {
            let Some(endpoint) = endpoints.get_mut(id) else {
                continue;
            };
            let holds = endpoint.after.contains(entry);
            if op == LinkOp::Add && !holds {
                endpoint.after.push(entry.clone());
                needed.push(id.clone());
            } else if op == LinkOp::Remove && holds {
                endpoint.after.retain(|held| held != entry);
                needed.push(id.clone());
            } else if endpoint.before.contains(entry) != holds {
                // An earlier link of this call already wrote this entry.
                needed.push(id.clone());
            }
        }
        let decision = if needed.is_empty() {
            Decision::Answered(
                if op == LinkOp::Add {
                    LinkOutcome::Exists
                } else {
                    LinkOutcome::Absent
                },
                None,
            )
        } else {
            Decision::Writes(needed)
        };
        decisions.insert(*at, decision);
    }
    decisions
}

/// One `## Links` line: the type, the direction, and the other item as a link where its
/// source gives it an address.
fn bullet(link: &TaskLink, other: Option<&Relations>) -> String {
    let arrow = match link.direction {
        LinkDirection::Out => "→",
        LinkDirection::In => "←",
    };
    // An address spanning lines could not stay on the bullet's one line, so the bullet names
    // the other end without it.
    let address = other
        .and_then(|other| {
            other.task.url.clone().or_else(|| {
                other.task.location.as_ref().map(|location| match location {
                    Location::Url(url) => url.clone(),
                    Location::Path(path) => path.clone(),
                })
            })
        })
        .filter(|address| !address.chars().any(char::is_control));
    let title = other.map_or_else(
        || link.other.clone(),
        |other| {
            // One line, as the section's reader needs: a title's line breaks become spaces.
            other
                .task
                .title
                .replace(['[', ']'], "")
                .replace(|c: char| c.is_control(), " ")
        },
    );
    match address {
        Some(address) => format!(
            "**{}** {arrow} [{title}]({address}) `{}`",
            link.link_type, link.other
        ),
        None => format!("**{}** {arrow} {title} `{}`", link.link_type, link.other),
    }
}

/// The links `metadata` holds, each beside its other end as a qualified id — or why they cannot
/// be read: a value that is not a list of links, a link whose other end is no qualified id, or
/// one whose type is no name a link can carry, refused naming `id` rather than read as a link
/// to nothing.
fn stored_links(
    id: &GlobalId,
    metadata: &BTreeMap<String, serde_json::Value>,
) -> Result<Vec<(TaskLink, GlobalId)>, String> {
    TaskLink::held(metadata)
        .map_err(|why| format!("{id}: {why}"))?
        .into_iter()
        .map(|link| match link.other.parse::<GlobalId>() {
            Ok(other) => match LinkTypeName::new(&link.link_type) {
                Ok(_) => Ok((link, other)),
                Err(why) => Err(format!("{id} holds a link to {} that {why}", link.other)),
            },
            Err(error) => Err(format!(
                "{id} holds a `{}` link whose other end {:?} is not a qualified id: {error}",
                link.link_type, link.other
            )),
        })
        .collect()
}

/// The direction the other end of a link holds it in.
const fn opposite(direction: LinkDirection) -> LinkDirection {
    match direction {
        LinkDirection::Out => LinkDirection::In,
        LinkDirection::In => LinkDirection::Out,
    }
}

/// The unordered pair `a`–`b`, spelled one way.
fn ordered(a: &GlobalId, b: &GlobalId) -> (GlobalId, GlobalId) {
    if a <= b {
        (a.clone(), b.clone())
    } else {
        (b.clone(), a.clone())
    }
}

/// `ids` grouped by source, in name order, each source's native ids in order.
fn by_source(ids: &BTreeSet<GlobalId>) -> BTreeMap<SourceName, Vec<NativeId>> {
    let mut grouped: BTreeMap<SourceName, Vec<NativeId>> = BTreeMap::new();
    for id in ids {
        grouped
            .entry(id.source.clone())
            .or_default()
            .push(id.native.clone());
    }
    grouped
}

/// A relation read's answers held to one per id asked — a count that differs failing every
/// id, because no answer can then be told to be any one id's.
fn answers_each(
    source: &super::ResolvedSource,
    asked: &[NativeId],
    answers: Vec<Result<Option<Relations>, SourceError>>,
) -> Result<Vec<Result<Option<Relations>, SourceError>>, EngineError> {
    if answers.len() == asked.len() {
        // Each answer is held to the task it was asked about, so a read never lends one task's
        // content, links or version to another; one that names another task is that id's
        // error alone.
        return Ok(asked
            .iter()
            .zip(answers)
            .map(|(native, answer)| match answer {
                Ok(Some(relations)) if &relations.task.id != native => {
                    Err(SourceError::Malformed {
                        message: format!(
                            "asked for the relations of {native}, it answered with those of {}",
                            relations.task.id
                        ),
                    })
                }
                answer => answer,
            })
            .collect());
    }
    Err(source_failed(
        source,
        SourceError::Malformed {
            message: format!(
                "it answered {} relation reads for the {} tasks asked",
                answers.len(),
                asked.len()
            ),
        },
    ))
}

/// A member that must be present and may be `null`: serde reads an absent `Option` as `None`
/// unless the member names its own deserializer, so naming this one makes it required.
fn nullable<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<Option<String>, D::Error> {
    Option::<String>::deserialize(deserializer)
}

/// One parent edge checked: its source and the write, or what it is answered without one.
fn checked_edge(edge: &ParentEdge) -> Result<(SourceName, ParentWrite), (ParentOutcome, String)> {
    let child = qualified(&edge.child, "the child").map_err(|why| (ParentOutcome::Error, why))?;
    let parent = match &edge.parent {
        None => None,
        Some(parent) => {
            let parent =
                qualified(parent, "the parent").map_err(|why| (ParentOutcome::Error, why))?;
            if parent.source != child.source {
                return Err((
                    ParentOutcome::Refused,
                    format!(
                        "{child} is a task of {} and its parent {parent} a task of {}: a parent \
                         is a task of its child's own source, so nothing was written",
                        child.source, parent.source
                    ),
                ));
            }
            if parent == child {
                return Err((
                    ParentOutcome::Refused,
                    format!("{child} cannot be its own parent: that is a cycle"),
                ));
            }
            Some(parent.native)
        }
    };
    Ok((
        child.source.clone(),
        ParentWrite {
            child: child.native,
            parent,
            if_version: edge.if_version.clone(),
        },
    ))
}

/// One link checked.
fn checked_link(item: &LinkItem) -> Result<Planned, String> {
    let from = qualified(&item.from, "the link's from")?;
    let to = qualified(&item.to, "the link's to")?;
    let link_type = LinkTypeName::new(&item.link_type)
        .map_err(|why| format!("the link from {from} to {to} {why}"))?;
    if from == to {
        return Err(format!("{from} cannot be linked to itself"));
    }
    Ok(Planned {
        from,
        to,
        link_type,
    })
}

fn parent_result(
    edge: &ParentEdge,
    outcome: ParentOutcome,
    version: Option<String>,
    reason: Option<String>,
) -> ParentResult {
    ParentResult {
        child: edge.child.clone(),
        parent: edge.parent.clone(),
        outcome,
        version,
        reason,
    }
}

fn link_result(
    item: &LinkItem,
    outcome: LinkOutcome,
    from_version: Option<String>,
    to_version: Option<String>,
    reason: Option<String>,
) -> LinkResult {
    LinkResult {
        from: item.from.clone(),
        to: item.to.clone(),
        link_type: item.link_type.clone(),
        outcome,
        from_version,
        to_version,
        reason,
    }
}
