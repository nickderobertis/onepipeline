//! Recording and checking attestations: the engine's half of [`crate::attestation`].
//!
//! Recording one is one read of the item — what its digest is computed over, and the version
//! `if_version` is held to — then one narrow metadata write of the whole
//! `onetaskgraph.attestations` entry, through the same seam `metadata set` writes through, so
//! a public write is screened exactly as that one is. A document's write takes the read rather
//! than reading again. Checking one is the read alone. A task's or a project's own forward
//! edges are read beside it, and each far end read once for the root of its chain of copies,
//! which the digest names it by. Where a source is declared private, a far end in a source
//! that is not, recording no root a copy out of a private source could have withheld, also
//! costs a query of each source declared private for the item linking to it
//! (`withheld_root`).

use onetaskgraph_plugin_api::{
    DependencyEdge, Document, ItemKind, MetadataKey, MetadataMatch, MetadataRecord, Repository,
    SourceName,
};
use serde_json::Value;

use super::copy::{Level, forward_edges};
use super::delivery::source_failed;
use super::dependents::could_not_answer;
use super::{Engine, EngineError, Filters, Paging, ProjectRequest, ProjectSelector, TaskRequest};
use crate::GlobalId;
use crate::attestation::{
    Attestation, AttestationCheck, AttestationKind, AttestationLabel, AttestationNote, Attested,
    Authored, AuthoredEdge, FarEnd, ProjectAttestationCheck, TaskAttestationCheck, check,
    content_digest, now, record_of, records,
};
use crate::config::SourceVisibility;
use crate::resolve::ResolvedSource;
use crate::template::Sha256Digest;

/// One attestation to record.
#[derive(Debug, Clone, PartialEq)]
pub struct AttestRequest {
    /// The item to record it on.
    pub id: GlobalId,
    /// Its kind: a record of this kind the item already holds is replaced.
    pub kind: AttestationKind,
    /// Who attests, as the caller labels them.
    pub by: AttestationLabel,
    /// The fingerprint of the bar the content is cleared against.
    pub bar: AttestationLabel,
    /// Caller-owned metadata keys to leave out of the digest.
    pub excludes: Vec<MetadataKey>,
    /// What the attester adds, if anything.
    pub note: Option<AttestationNote>,
    /// Record it only while the item is at this version: the read it is composed from is
    /// refused, writing nothing, when it answers another.
    pub if_version: Option<String>,
}

/// What a task, a project or a document holds that the digest reads.
struct Held {
    title: String,
    content: Option<String>,
    repositories: Vec<Repository>,
    metadata: std::collections::BTreeMap<String, Value>,
    version: Option<String>,
}

/// One item as the digest needs it: a task or a project with its own forward edges, or a
/// document — which has none, and whose write takes this read rather than reading again.
enum Read {
    /// A task or a project.
    Work {
        held: Held,
        edges: Vec<AuthoredEdge>,
    },
    /// A document, as it was read.
    Document(Document),
}

impl Read {
    fn metadata(&self) -> &std::collections::BTreeMap<String, Value> {
        match self {
            Self::Work { held, .. } => &held.metadata,
            Self::Document(document) => &document.metadata,
        }
    }

    fn version(&self) -> Option<&str> {
        match self {
            Self::Work { held, .. } => held.version.as_deref(),
            Self::Document(document) => document.version.as_deref(),
        }
    }

    fn digest(&self, excludes: &[MetadataKey]) -> Sha256Digest {
        let authored = match self {
            Self::Work { held, edges } => Authored {
                title: &held.title,
                content: held.content.as_deref(),
                repositories: &held.repositories,
                metadata: &held.metadata,
                depends_on: edges.clone(),
            },
            Self::Document(document) => Authored {
                title: &document.title,
                content: document.content.as_deref(),
                repositories: &document.repositories,
                metadata: &document.metadata,
                depends_on: Vec::new(),
            },
        };
        content_digest(&authored, excludes)
    }
}

impl Engine {
    /// Record one attestation on one task, project or document, replacing the record of its
    /// kind the item holds; every other kind's record is left as it was.
    ///
    /// # Errors
    ///
    /// Returns [`EngineError::UnknownSource`] for a source nothing configures,
    /// [`EngineError::MetadataNotWritable`] for one with no write side,
    /// [`EngineError::NoSuchTask`], [`EngineError::NoSuchProject`] or
    /// [`EngineError::NoSuchDocument`] when the item is not there,
    /// [`EngineError::VersionMoved`] when `if_version` is not the version the item is at, and
    /// every refusal a narrow metadata write can answer — the public boundary's among them.
    pub async fn attest(
        &self,
        record: MetadataRecord,
        request: &AttestRequest,
    ) -> Result<Attested, EngineError> {
        let id = &request.id;
        let source = self.metadata_writable(id, record)?;
        let mut read = self.read_authored(source, record, id).await?;
        if let Some(expected) = &request.if_version {
            let at = read.version().unwrap_or_default().to_owned();
            if source.source().order_versions(&at, expected) != Some(std::cmp::Ordering::Equal) {
                return Err(EngineError::VersionMoved {
                    id: id.to_string(),
                    expected: expected.clone(),
                    held: Box::new(crate::cache::WriteResult {
                        version: Some(at.clone()),
                        parts: None,
                        status: None,
                        priority: None,
                    }),
                    found: at,
                });
            }
        }
        // Held to its version first, so a stale one is refused before anything is written. A
        // write that sanitises the task as stored would otherwise replace the very text the
        // digest is taken over, leaving the record void the moment it lands: that is done
        // first, and the task read again as it then stands.
        if record == MetadataRecord::Task && self.sanitise_stored_task(source, id).await? {
            read = self.read_authored(source, record, id).await?;
        }
        let mut excludes = request.excludes.clone();
        excludes.sort();
        excludes.dedup();
        let attestation = Attestation {
            kind: request.kind.clone(),
            by: request.by.clone(),
            at: now(),
            content_digest: read.digest(&excludes),
            bar: request.bar.clone(),
            note: request.note.clone(),
            excludes,
        };
        let mut entries: serde_json::Map<String, Value> =
            records(read.metadata()).into_iter().collect();
        entries.insert(request.kind.as_str().to_owned(), attestation.to_value());
        let key = MetadataKey::attestations();
        let value = Value::Object(entries);
        let set = match read {
            Read::Document(document) => {
                self.set_read_document_metadata(source, id, &key, &value, &document)
                    .await?
            }
            Read::Work { .. } if record == MetadataRecord::Project => {
                self.set_project_metadata(id, &key, &value).await?
            }
            Read::Work { .. } => self.set_task_metadata(id, &key, &value).await?,
        };
        // The record is what the source holds after the write — screening may have rewritten
        // a label on its way — and a write answering a record of another kind, or none this
        // product can read, is the source misanswering.
        let mut held = std::collections::BTreeMap::new();
        held.insert(MetadataKey::ATTESTATIONS_KEY.to_owned(), set.value);
        let record = record_of(&held, &request.kind)
            // Screening may rewrite a label — who attested, the bar, the note — and nothing
            // else: every other member must read back as it was written.
            .filter(|held| {
                held.content_digest == attestation.content_digest
                    && held.at == attestation.at
                    && held.excludes == attestation.excludes
            })
            .ok_or_else(|| {
                source_failed(
                    source,
                    onetaskgraph_plugin_api::SourceError::Malformed {
                        message: format!(
                            "the record {id} it answered the write with does not hold the {} \
                             attestation it was just written",
                            request.kind
                        ),
                    },
                )
            })?;
        Ok(Attested {
            id: id.clone(),
            record,
        })
    }

    /// Check one kind of attestation on one task, project or document: `valid` when its
    /// record's content is the item's and — when `bar` is given — its bar is `bar`.
    ///
    /// # Errors
    ///
    /// Returns [`EngineError::UnknownSource`] for a source nothing configures,
    /// [`EngineError::NoDocuments`] for a document of a source holding none,
    /// [`EngineError::NoSuchTask`], [`EngineError::NoSuchProject`] or
    /// [`EngineError::NoSuchDocument`] when the item is not there, and
    /// [`EngineError::SourceFailed`] when a read fails.
    pub async fn check_attestation(
        &self,
        record: MetadataRecord,
        id: &GlobalId,
        kind: &AttestationKind,
        bar: Option<&AttestationLabel>,
    ) -> Result<AttestationCheck, EngineError> {
        let source = self.built(&id.source)?;
        if record == MetadataRecord::Document
            && !source.source().capabilities().documents.is_native()
        {
            return Err(EngineError::NoDocuments {
                name: source.name().to_string(),
                kind: source.kind().to_owned(),
            });
        }
        let read = self.read_authored(source, record, id).await?;
        Ok(check(
            kind,
            record_of(read.metadata(), kind),
            |excludes| read.digest(excludes),
            bar,
        ))
    }

    /// Check one kind of attestation on every task filed under one project, in the order the
    /// source lists them.
    ///
    /// # Errors
    ///
    /// As [`check_attestation`](Self::check_attestation), and
    /// [`EngineError::SourceRefused`] when the project's tasks cannot be listed whole.
    pub async fn check_project_attestations(
        &self,
        project: &GlobalId,
        kind: &AttestationKind,
        bar: Option<&AttestationLabel>,
    ) -> Result<ProjectAttestationCheck, EngineError> {
        let source = self.built(&project.source)?;
        source
            .source()
            .get_project(&project.native)
            .await
            .map_err(|error| source_failed(source, error))?
            .ok_or_else(|| EngineError::NoSuchProject {
                id: project.to_string(),
            })?;
        let listed = self
            .project_member_tasks(project, "for an attestation check")
            .await?;
        let mut tasks = Vec::with_capacity(listed.len());
        for task in listed {
            let read = self
                .authored_from(
                    source,
                    &task.id,
                    ItemKind::Task,
                    Held {
                        title: task.item.title,
                        content: task.item.content,
                        repositories: task.item.repositories,
                        metadata: task.item.metadata,
                        version: task.item.version,
                    },
                )
                .await?;
            tasks.push(TaskAttestationCheck {
                id: task.id,
                check: check(
                    kind,
                    record_of(read.metadata(), kind),
                    |excludes| read.digest(excludes),
                    bar,
                ),
            });
        }
        Ok(ProjectAttestationCheck {
            project: project.clone(),
            kind: kind.clone(),
            tasks,
        })
    }

    /// Read the item `id` names and its own forward edges, as the digest reads them.
    async fn read_authored(
        &self,
        source: &ResolvedSource,
        record: MetadataRecord,
        id: &GlobalId,
    ) -> Result<Read, EngineError> {
        let missing = || match record {
            MetadataRecord::Task => EngineError::NoSuchTask { id: id.to_string() },
            MetadataRecord::Project => EngineError::NoSuchProject { id: id.to_string() },
            MetadataRecord::Document => EngineError::NoSuchDocument { id: id.to_string() },
        };
        let (kind, authoring) = match record {
            MetadataRecord::Task => {
                let task = source
                    .source()
                    .get_task(&id.native)
                    .await
                    .map_err(|error| source_failed(source, error))?
                    .ok_or_else(missing)?;
                (
                    ItemKind::Task,
                    Held {
                        title: task.title,
                        content: task.content,
                        repositories: task.repositories,
                        metadata: task.metadata,
                        version: task.version,
                    },
                )
            }
            MetadataRecord::Project => {
                let project = source
                    .source()
                    .get_project(&id.native)
                    .await
                    .map_err(|error| source_failed(source, error))?
                    .ok_or_else(missing)?;
                (
                    ItemKind::Project,
                    Held {
                        title: project.title,
                        content: project.content,
                        repositories: project.repositories,
                        metadata: project.metadata,
                        version: project.version,
                    },
                )
            }
            MetadataRecord::Document => {
                let document = source
                    .source()
                    .get_document(&id.native)
                    .await
                    .map_err(|error| source_failed(source, error))?
                    .ok_or_else(missing)?;
                return Ok(Read::Document(document));
            }
        };
        self.authored_from(source, id, kind, authoring).await
    }

    /// `authoring`, with its forward edges read and each far end named.
    async fn authored_from(
        &self,
        source: &ResolvedSource,
        id: &GlobalId,
        kind: ItemKind,
        authoring: Held,
    ) -> Result<Read, EngineError> {
        let level = match kind {
            ItemKind::Task => Level::Task,
            ItemKind::Project => Level::Project,
        };
        let mut edges = Vec::new();
        for edge in forward_edges(source, &id.native, level).await? {
            edges.push(self.edge_entry(source.name(), &edge).await?);
        }
        Ok(Read::Work {
            held: authoring,
            edges,
        })
    }

    /// One edge as the digest names it: its kind, its far end's kind, and its far end by its
    /// lineage root — see [`lineage_root`](Self::lineage_root).
    async fn edge_entry(
        &self,
        near: &SourceName,
        edge: &DependencyEdge,
    ) -> Result<AuthoredEdge, EngineError> {
        let entry = |end: FarEnd| AuthoredEdge {
            kind: edge.kind,
            far: edge.to.kind,
            end,
        };
        if let Some(designator) = edge.to.designator() {
            // An opaque far end names no configured source this read can follow: it is named by
            // its tracker and its id there, since the same id in another tracker is another item.
            return Ok(entry(FarEnd::Opaque {
                designator: designator.clone(),
                id: edge.to.id().to_owned(),
            }));
        }
        let start = if edge.to.is_native() {
            format!("{near}:{}", edge.to.id())
        } else {
            edge.to.id().to_owned()
        };
        Ok(entry(self.lineage_root(&start, edge.to.kind).await?))
    }

    /// The item `start` names, by the root of the chain of copies it is one of — what it
    /// records at [`GlobalId::LINEAGE_KEY`], else at [`GlobalId::ORIGIN_KEY`], else its own id:
    /// the one id every copy of it, however many hops away, records or is.
    ///
    /// One read of the far end itself and nothing past it, so no item between it and its root
    /// has to be there: a copy deleted between two others leaves the root, and every record
    /// over an edge to it, as they were. Two items sharing a title never share a root. An id
    /// this configuration names no source for, and an item its source does not hold, are named
    /// by that id: the edge still names it, so it is still the same edge.
    ///
    /// # Errors
    ///
    /// [`EngineError::SourceFailed`], with the source answering it, for a lineage or an origin
    /// that is not a qualified id — it has no root every copy would agree on, so none is
    /// guessed — and for a far end's source failing to answer; and the refusal a configured
    /// source that cannot be built answers with.
    async fn lineage_root(&self, start: &str, kind: ItemKind) -> Result<FarEnd, EngineError> {
        let Ok(id) = start.parse::<GlobalId>() else {
            // An edge spelled with no source is named as it is spelled.
            return Ok(FarEnd::Id(start.to_owned()));
        };
        let far = match self.built(&id.source) {
            Ok(far) => far,
            Err(EngineError::UnknownSource { .. }) => return Ok(FarEnd::Lineage(id)),
            Err(error) => return Err(error),
        };
        let metadata = match kind {
            ItemKind::Task => far
                .source()
                .get_task(&id.native)
                .await
                .map_err(|error| source_failed(far, error))?
                .map(|task| task.metadata),
            ItemKind::Project => far
                .source()
                .get_project(&id.native)
                .await
                .map_err(|error| source_failed(far, error))?
                .map(|project| project.metadata),
        };
        let Some(metadata) = metadata else {
            return Ok(FarEnd::Lineage(id));
        };
        let root = match recorded_root(&metadata, start) {
            Some(Ok(root)) => Root::Recorded(root),
            Some(Err(error)) => return Err(source_failed(far, error)),
            None => Root::Read(id),
        };
        let fallback = root.id().clone();
        Ok(FarEnd::Lineage(
            self.withheld_root(root, kind).await?.unwrap_or(fallback),
        ))
    }

    /// The root of the chain of copies a copy out of a source declared private leads back to,
    /// where `root` is such a copy — `None` where it is not one, or where which item it was
    /// copied from cannot be told.
    ///
    /// A copy out of a source declared private into one that is not records no origin naming
    /// it (journey 89): its provenance stays on the private side, as the
    /// [`MetadataKey::COPIES_KEY`] link the private item records to it. So an item that records
    /// no root of its own and sits in a source not declared private may be such a copy, and its
    /// root is found where its correspondence is kept: the one item of a source declared private
    /// whose link to `root`'s source names `root`, by that item's own root. Asked only of a
    /// configured source declared private other than `root`'s own, and only when `root`'s own
    /// source is configured and is not declared private — so a store declaring nothing private
    /// sends exactly the requests it did, and a private far end costs none. A source declared
    /// private that could not be asked (its credential absent, unreachable, rate limited) is
    /// passed over, and two items linking to `root` are not a confident answer: either leaves
    /// `root` naming itself, which can void a record but never makes one read `valid`.
    ///
    /// A [`Root::Recorded`] — what the far end records — is read once first, because a copy out
    /// of an item that was itself copied out of a source declared private records that item as
    /// its origin and hands on no lineage naming the private source: what it records is then
    /// the root, and nothing past it is read. A [`Root::Read`] was read already.
    ///
    /// # Errors
    ///
    /// [`EngineError::SourceFailed`] for `root`'s source or a private source failing otherwise,
    /// and for `root` or the item found recording a lineage or an origin that is not a
    /// qualified id.
    async fn withheld_root(
        &self,
        root: Root,
        kind: ItemKind,
    ) -> Result<Option<GlobalId>, EngineError> {
        let (root, read) = match root {
            Root::Read(id) => (id, true),
            Root::Recorded(id) => (id, false),
        };
        if !self
            .sources
            .iter()
            .any(|source| source.name() == &root.source)
            || self.declared(&root.source) == SourceVisibility::Private
        {
            return Ok(None);
        }
        let private: Vec<SourceName> = self
            .sources
            .iter()
            .map(|source| source.name().clone())
            .filter(|name| name != &root.source && self.declared(name) == SourceVisibility::Private)
            .collect();
        if private.is_empty() {
            return Ok(None);
        }
        if !read {
            // A root a copy recorded, read for the root it records in turn: a copy out of an
            // item that was itself copied out of a source declared private hands on no
            // lineage naming that source, so its copy records this item as its root.
            let far = self.built(&root.source)?;
            let metadata = match kind {
                ItemKind::Task => far
                    .source()
                    .get_task(&root.native)
                    .await
                    .map_err(|error| source_failed(far, error))?
                    .map(|task| task.metadata),
                ItemKind::Project => far
                    .source()
                    .get_project(&root.native)
                    .await
                    .map_err(|error| source_failed(far, error))?
                    .map(|project| project.metadata),
            };
            match metadata
                .as_ref()
                .and_then(|held| recorded_root(held, &root.to_string()))
            {
                Some(Ok(recorded)) => return Ok(Some(recorded)),
                Some(Err(error)) => return Err(source_failed(far, error)),
                None => {}
            }
        }
        let mut found = Linked::Nothing;
        for source in private {
            let linked = match kind {
                ItemKind::Task => self.tasks_linking(&source, &root).await,
                ItemKind::Project => self.projects_linking(&source, &root).await,
            };
            found = match linked {
                Ok(linked) => found.and(linked),
                Err(EngineError::SourceFailed { name, error })
                    if name == source.as_str() && could_not_answer(&error) =>
                {
                    found
                }
                Err(error) => return Err(error),
            };
            if matches!(found, Linked::Several) {
                return Ok(None);
            }
        }
        let Linked::One(private, metadata) = found else {
            return Ok(None);
        };
        match recorded_root(&metadata, &private.to_string()) {
            Some(Ok(lineage)) => Ok(Some(lineage)),
            Some(Err(error)) => Err(EngineError::SourceFailed {
                name: private.source.to_string(),
                error,
            }),
            None => Ok(Some(private)),
        }
    }

    /// The tasks of `source` whose [`MetadataKey::COPIES_KEY`] link names `copy`, through the
    /// engine's own task query, which narrows a source that cannot apply the predicate itself
    /// and fills a page before it says more follow — so its first page answers: one with more
    /// to come already holds a page of links, which is several.
    async fn tasks_linking(
        &self,
        source: &SourceName,
        copy: &GlobalId,
    ) -> Result<Linked, EngineError> {
        let wanted = MetadataMatch::new(
            MetadataKey::COPIES_KEY,
            vec![copy.source.to_string()],
            copy.to_string(),
        )
        .expect("the key, a source name and a qualified id are never empty");
        let request = TaskRequest {
            sources: vec![source.clone()],
            filters: Filters::default(),
            project: ProjectSelector::Any,
            priorities: Vec::new(),
            commented_since: None,
            metadata: vec![wanted],
            origin: None,
            include_members: false,
            paging: Paging {
                limit: super::copy::PROJECT_PAGE,
                token: None,
            },
        };
        let response = self.tasks(&request).await?;
        if let Some(failure) = response.errors.into_iter().next() {
            return Err(EngineError::SourceFailed {
                name: failure.source.to_string(),
                error: failure.error,
            });
        }
        Ok(response
            .items
            .into_iter()
            .filter(|task| links(&task.item.metadata, copy))
            .fold(Linked::Nothing, |found, task| {
                found.and(Linked::One(task.id, task.item.metadata))
            }))
    }

    /// The projects of `source` whose [`MetadataKey::COPIES_KEY`] link names `copy`: a project
    /// query has no metadata predicate, so the source's projects are listed one page at a time
    /// and narrowed here, as far as a second link.
    async fn projects_linking(
        &self,
        source: &SourceName,
        copy: &GlobalId,
    ) -> Result<Linked, EngineError> {
        let mut request = ProjectRequest {
            sources: vec![source.clone()],
            filters: Filters::default(),
            paging: Paging {
                limit: super::copy::PROJECT_PAGE,
                token: None,
            },
        };
        let mut found = Linked::Nothing;
        loop {
            let response = self.projects(&request).await?;
            if let Some(failure) = response.errors.into_iter().next() {
                return Err(EngineError::SourceFailed {
                    name: failure.source.to_string(),
                    error: failure.error,
                });
            }
            for project in response.items {
                if links(&project.item.metadata, copy) {
                    found = found.and(Linked::One(project.id, project.item.metadata));
                }
            }
            match response.next {
                Some(token) if !matches!(found, Linked::Several) => {
                    request.paging.token = Some(token);
                }
                _ => return Ok(found),
            }
        }
    }
}

/// What the sources declared private hold that links to a copy.
enum Linked {
    Nothing,
    /// One item, with its metadata, which says what its root is.
    One(GlobalId, std::collections::BTreeMap<String, Value>),
    /// More than one, which is no confident answer.
    Several,
}

impl Linked {
    /// What `self` and `other` hold together.
    fn and(self, other: Self) -> Self {
        match (self, other) {
            (Self::Nothing, held) | (held, Self::Nothing) => held,
            _ => Self::Several,
        }
    }
}

/// The root of a far end's chain of copies, as [`Engine::lineage_root`] first finds it.
enum Root {
    /// The far end itself, read already and recording no root.
    Read(GlobalId),
    /// The root the far end records, not read yet.
    Recorded(GlobalId),
}

impl Root {
    fn id(&self) -> &GlobalId {
        match self {
            Self::Read(id) | Self::Recorded(id) => id,
        }
    }
}

/// Whether `metadata`'s [`MetadataKey::COPIES_KEY`] link to `copy`'s source names `copy`.
fn links(metadata: &std::collections::BTreeMap<String, Value>, copy: &GlobalId) -> bool {
    metadata
        .get(MetadataKey::COPIES_KEY)
        .and_then(|links| links.get(copy.source.as_str()))
        .and_then(Value::as_str)
        == Some(copy.to_string().as_str())
}

/// The root an item recording `metadata` names for its chain of copies — its
/// [`GlobalId::LINEAGE_KEY`], else its [`GlobalId::ORIGIN_KEY`] — `None` when it records
/// neither, where the item is its own root; and a refusal naming `item` when what it records
/// is not a qualified id, which has no root every copy would agree on.
fn recorded_root(
    metadata: &std::collections::BTreeMap<String, Value>,
    item: &str,
) -> Option<Result<GlobalId, onetaskgraph_plugin_api::SourceError>> {
    [GlobalId::LINEAGE_KEY, GlobalId::ORIGIN_KEY]
        .into_iter()
        .find_map(|key| metadata.get(key).map(|recorded| (key, recorded)))
        .map(
            |(key, recorded)| match recorded.as_str().map(str::parse::<GlobalId>) {
                Some(Ok(root)) => Ok(root),
                _ => Err(onetaskgraph_plugin_api::SourceError::Malformed {
                    message: format!(
                        "{item} records the {key} {recorded}, which is not a qualified id, so \
                         the dependency on it cannot be named for an attestation"
                    ),
                }),
            },
        )
}
