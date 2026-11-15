//! The entries the cache keeps beside a task's: a project's, a document's, a task's and a
//! project's forward and reverse edges each read on their own, and the verified bytes of an image
//! asset.
//!
//! **Each kind lives in a directory of its own, named with its schema** — `projects-v4/`,
//! `documents-v4/`, `edges-v4/`, `reverse-edges-v4/`, `project-edges-v4/`,
//! `project-reverse-edges-v4/`, `bytes-v4/` — so an id cannot name one kind's entry for
//! another's, and a release writing this schema never writes over a file a release of another
//! schema wrote: that one lives in its own directory. Each entry still carries its schema, its
//! kind and the remote it is from, and a reader holds it to all three, so a file another release
//! or another remote left where this one looks is a miss.
//!
//! **A byte entry is named by its own SHA-256 and holds nothing else**, under a directory named
//! for the remote it was read from. It is written only when the bytes hash to that name and
//! served only when they still do, so a file truncated, corrupted or replaced is a miss and is
//! never answered with. What decides whether bytes are *served* is not here: it is the manifest
//! the caching source served at a covered version, which names the digest.

use std::path::{Path, PathBuf};

use onetaskgraph_plugin_api::{Asset, DependencyEdge, Document, NativeId, Project};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

use super::{SourceCache, digest};

/// The schema every project, document, edge and byte entry this build writes is spelled in,
/// and the suffix of the directory each kind lives in.
///
/// Held apart from [`super::ENTRY_SCHEMA`], which a task's entries carry, so a change to these
/// kinds' shapes moves their directories — every one of them, since they share this number —
/// and leaves a task's entries served.
///
/// Version 2 added a task's reverse edges and a project's forward and reverse edges as kinds of
/// their own; what version 1 kept is left in its own directories, unread, for
/// `cache clear` of that release. Version 3 admits the opaque form of a dependency endpoint
/// among the edges they keep, which an earlier release would misread as a native id. Version 4
/// changes no byte an entry holds: the schema these shapes are digested from now describes a
/// task's classification as its own, never tightened by its project, and an entry version 3
/// wrote is read as a miss.
pub const KIND_ENTRY_SCHEMA: u32 = 4;

/// Which kind an entry is: written into it and held to the directory it is read from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum EntryKind {
    /// A project's fields.
    Project,
    /// A document's fields and the manifest of its image assets.
    Document,
    /// A task's forward dependency edges, read on their own.
    Edges,
    /// A task's reverse dependency edges — what depends on it — read on their own.
    ReverseEdges,
    /// A project's forward dependency edges, read on their own.
    ProjectEdges,
    /// A project's reverse dependency edges, read on their own.
    ProjectReverseEdges,
}

impl EntryKind {
    /// Every kind, in the order `cache clear` walks them.
    const ALL: [Self; 6] = [
        Self::Project,
        Self::Document,
        Self::Edges,
        Self::ReverseEdges,
        Self::ProjectEdges,
        Self::ProjectReverseEdges,
    ];

    /// The directory this kind's entries live in.
    fn directory(self) -> String {
        let name = match self {
            Self::Project => "projects",
            Self::Document => "documents",
            Self::Edges => "edges",
            Self::ReverseEdges => "reverse-edges",
            Self::ProjectEdges => "project-edges",
            Self::ProjectReverseEdges => "project-reverse-edges",
        };
        format!("{name}-v{KIND_ENTRY_SCHEMA}")
    }
}

/// Which edges an edges entry holds: one of the four edge kinds of [`EntryKind`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EdgeKind {
    /// A task's forward edges.
    TaskForward,
    /// A task's reverse edges.
    TaskReverse,
    /// A project's forward edges.
    ProjectForward,
    /// A project's reverse edges.
    ProjectReverse,
}

impl EdgeKind {
    fn entry(self) -> EntryKind {
        match self {
            Self::TaskForward => EntryKind::Edges,
            Self::TaskReverse => EntryKind::ReverseEdges,
            Self::ProjectForward => EntryKind::ProjectEdges,
            Self::ProjectReverse => EntryKind::ProjectReverseEdges,
        }
    }

    fn tag(self) -> EdgesTag {
        match self {
            Self::TaskForward => EdgesTag::Edges,
            Self::TaskReverse => EdgesTag::ReverseEdges,
            Self::ProjectForward => EdgesTag::ProjectEdges,
            Self::ProjectReverse => EdgesTag::ProjectReverseEdges,
        }
    }
}

/// The one `kind` a project entry can carry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum ProjectTag {
    /// [`EntryKind::Project`].
    Project,
}

/// The one `kind` a document entry can carry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum DocumentTag {
    /// [`EntryKind::Document`].
    Document,
}

/// The `kind` an edges entry can carry: one of the four edge kinds, and never another.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum EdgesTag {
    /// [`EntryKind::Edges`].
    Edges,
    /// [`EntryKind::ReverseEdges`].
    ReverseEdges,
    /// [`EntryKind::ProjectEdges`].
    ProjectEdges,
    /// [`EntryKind::ProjectReverseEdges`].
    ProjectReverseEdges,
}

/// One project as the cache keeps it, at its own `version`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub(crate) struct ProjectEntry {
    /// [`KIND_ENTRY_SCHEMA`].
    pub(crate) schema: u32,
    /// [`EntryKind::Project`], the only kind it can carry.
    pub(crate) kind: ProjectTag,
    /// The digest of the remote it is from.
    // llmlint: ignore[invalid_states_unrepresentable] An equality key and nothing more: every read compares it byte for byte with this store's own `CacheLocation::remote`, the public `String` a task's entry carries too, so any other value is a miss and never misread; a digest type here alone would validate a value no reader interprets.
    pub(crate) remote: String,
    /// The project, its `id` the entry's own and its `version` the token it was read at.
    pub(crate) project: Project,
}

/// One document as the cache keeps it, at its own `version`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub(crate) struct DocumentEntry {
    /// [`KIND_ENTRY_SCHEMA`].
    pub(crate) schema: u32,
    /// [`EntryKind::Document`], the only kind it can carry.
    pub(crate) kind: DocumentTag,
    /// The digest of the remote it is from.
    // llmlint: ignore[invalid_states_unrepresentable] An equality key and nothing more: every read compares it byte for byte with this store's own `CacheLocation::remote`, the public `String` a task's entry carries too, so any other value is a miss and never misread; a digest type here alone would validate a value no reader interprets.
    pub(crate) remote: String,
    /// The document, its `id` the entry's own and its `version` the token it was read at.
    pub(crate) document: Document,
    /// The manifest of its image assets — each one's name, digest and type — absent until a
    /// read of them followed a read of the document at this version.
    pub(crate) assets: Option<Vec<Asset>>,
}

/// One item's edges in one direction as a dependency read answered them — a task's or a
/// project's forward or reverse edges — at the version that read's own response spelled.
///
/// Never vouches for the item's fields: it holds nothing but the edges.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub(crate) struct EdgesEntry {
    /// [`KIND_ENTRY_SCHEMA`].
    pub(crate) schema: u32,
    /// Which edges it holds, and the directory it is read from.
    pub(crate) kind: EdgesTag,
    /// The digest of the remote it is from.
    // llmlint: ignore[invalid_states_unrepresentable] An equality key and nothing more: every read compares it byte for byte with this store's own `CacheLocation::remote`, the public `String` a task's entry carries too, so any other value is a miss and never misread; a digest type here alone would validate a value no reader interprets.
    pub(crate) remote: String,
    /// The item they are the edges of.
    pub(crate) id: NativeId,
    /// The version the read that answered them spelled.
    pub(crate) version: String,
    /// Every edge, in the order the source answered them.
    pub(crate) edges: Vec<DependencyEdge>,
}

/// The JSON Schema of every entry this build writes under [`KIND_ENTRY_SCHEMA`].
///
/// What `tests/cache.rs` holds that number to, as it holds [`super::ENTRY_SCHEMA`] to a task's.
#[must_use]
pub fn kind_entry_shapes() -> Value {
    serde_json::json!({
        "project": schemars::schema_for!(ProjectEntry),
        "document": schemars::schema_for!(DocumentEntry),
        "edges": schemars::schema_for!(EdgesEntry),
        "bytes": "the raw bytes, named by their own lower-case hex SHA-256",
    })
}

/// A lower-case hex SHA-256, which is all a byte entry may be called: made by hashing bytes, or
/// read from a manifest or a file name and refused when it is not one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ByteDigest(String);

impl ByteDigest {
    /// The digest of `bytes`.
    pub(crate) fn of(bytes: &[u8]) -> Self {
        use std::fmt::Write as _;
        let mut hex = String::with_capacity(64);
        for byte in Sha256::digest(bytes) {
            // Writing to a `String` never fails.
            let _ = write!(hex, "{byte:02x}");
        }
        Self(hex)
    }

    /// The digest `text` spells, when it spells one.
    pub(crate) fn parse(text: &str) -> Option<Self> {
        (text.len() == 64
            && text
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)))
        .then(|| Self(text.to_owned()))
    }

    fn as_str(&self) -> &str {
        &self.0
    }
}

impl SourceCache {
    /// Where the entry of `kind` for `id` lives.
    fn kind_path(&self, kind: EntryKind, id: &NativeId) -> PathBuf {
        self.dir
            .join(kind.directory())
            .join(format!("{}.json", digest(id.as_str())))
    }

    /// The directory this remote's verified bytes live in.
    fn bytes_dir(&self) -> PathBuf {
        bytes_dir(&self.dir, &self.remote)
    }

    /// The entry of `kind` at `path` as JSON, when it reads whole as one of this schema, this
    /// kind and this remote — or `None`, a miss.
    fn read_kind(&self, kind: EntryKind, path: &Path) -> Option<Value> {
        let bytes = self.read_file(path)?;
        let value: Value = serde_json::from_slice(&bytes).ok()?;
        let held = value.get("schema").and_then(Value::as_u64)
            == Some(u64::from(KIND_ENTRY_SCHEMA))
            && value.get("kind") == Some(&serde_json::to_value(kind).ok()?)
            && value.get("remote").and_then(Value::as_str) == Some(self.remote.as_str());
        held.then_some(value)
    }

    /// Write `entry` whole at `path`, or take whatever was there out of service.
    fn write_kind(&self, path: &Path, entry: &impl Serialize) {
        let written = serde_json::to_vec(entry)
            .map_err(std::io::Error::from)
            .and_then(|bytes| self.write_atomically(path, &bytes));
        if written.is_err() {
            let _ = self.retire(path);
        }
    }

    /// The project entry for `id`, or `None` when there is none this build can read.
    pub(crate) fn read_project(&self, id: &NativeId) -> Option<ProjectEntry> {
        let value = self.read_kind(EntryKind::Project, &self.kind_path(EntryKind::Project, id))?;
        let entry: ProjectEntry = serde_json::from_value(value).ok()?;
        (entry.project.id == *id && entry.project.version.is_some()).then_some(entry)
    }

    /// Keep `project`, which a read from the source answered at its own version.
    pub(crate) fn write_project(&self, project: &Project) {
        if project.version.is_none() {
            return;
        }
        self.write_kind(
            &self.kind_path(EntryKind::Project, &project.id),
            &ProjectEntry {
                schema: KIND_ENTRY_SCHEMA,
                kind: ProjectTag::Project,
                remote: self.remote.clone(),
                project: project.clone(),
            },
        );
    }

    /// The document entry for `id`, or `None` when there is none this build can read.
    pub(crate) fn read_document(&self, id: &NativeId) -> Option<DocumentEntry> {
        let value = self.read_kind(
            EntryKind::Document,
            &self.kind_path(EntryKind::Document, id),
        )?;
        let entry: DocumentEntry = serde_json::from_value(value).ok()?;
        (entry.document.id == *id && entry.document.version.is_some()).then_some(entry)
    }

    /// Keep `entry` — only when its document carries the version a read answered it at, since
    /// an entry without one could be served to no pin.
    pub(crate) fn write_document(&self, entry: &DocumentEntry) {
        if entry.document.version.is_none() {
            return;
        }
        self.write_kind(
            &self.kind_path(EntryKind::Document, &entry.document.id),
            entry,
        );
    }

    /// The `kind` edges kept for `id`, or `None` when there are none this build can read.
    pub(crate) fn read_edges(&self, kind: EdgeKind, id: &NativeId) -> Option<EdgesEntry> {
        let value = self.read_kind(kind.entry(), &self.kind_path(kind.entry(), id))?;
        let entry: EdgesEntry = serde_json::from_value(value).ok()?;
        (entry.id == *id && entry.kind == kind.tag()).then_some(entry)
    }

    /// Keep `edges` as `id`'s `kind` edges at `version`.
    pub(crate) fn write_edges(
        &self,
        kind: EdgeKind,
        id: &NativeId,
        version: &str,
        edges: &[DependencyEdge],
    ) {
        self.write_kind(
            &self.kind_path(kind.entry(), id),
            &EdgesEntry {
                schema: KIND_ENTRY_SCHEMA,
                kind: kind.tag(),
                remote: self.remote.clone(),
                id: id.clone(),
                version: version.to_owned(),
                edges: edges.to_vec(),
            },
        );
    }

    /// The bytes whose SHA-256 is `digest`, when they are kept and still hash to it.
    pub(crate) fn read_bytes(&self, digest: &ByteDigest) -> Option<Vec<u8>> {
        let bytes = self.read_file(&self.bytes_dir().join(digest.as_str()))?;
        (ByteDigest::of(&bytes) == *digest).then_some(bytes)
    }

    /// Keep `bytes`, under their own SHA-256.
    pub(crate) fn write_bytes(&self, bytes: &[u8]) {
        let digest = ByteDigest::of(bytes);
        let path = self.bytes_dir().join(digest.as_str());
        if self.read_bytes(&digest).is_some() {
            return;
        }
        if self.write_atomically(&path, bytes).is_err() {
            let _ = self.retire(&path);
        }
    }

    /// Take every entry naming `id` — its task's, its project's, its document's and its edges'
    /// of each kind — out of service for every process before a write naming it
    /// is sent, or refuse the write: what the write sends can change any of them, a task
    /// becoming a project or a document included.
    pub(crate) fn retire_kinds_before_write(&self, id: &NativeId) -> std::io::Result<()> {
        for kind in EntryKind::ALL {
            self.retire(&self.kind_path(kind, id))?;
        }
        Ok(())
    }

    /// Take every `kind` edges entry, whichever item it names, out of service for every process
    /// before a write that can change edges recorded on items it does not name is sent — a
    /// relation is recorded at both of its ends — or refuse the write.
    pub(crate) fn retire_every_before_write(&self, kind: EdgeKind) -> std::io::Result<()> {
        for path in self.kind_files(kind.entry())? {
            self.retire(&path)?;
        }
        Ok(())
    }

    /// Forget every `kind` edges entry, after a write that could change any of them.
    pub(crate) fn invalidate_every(&self, kind: EdgeKind) {
        for path in self.kind_files(kind.entry()).unwrap_or_default() {
            let _ = self.retire(&path);
        }
    }

    /// Every entry file of `kind` this cache's directory holds: each `.json` file there that reads
    /// whole as one of this store's entries of that kind, schema and remote — and each this
    /// process cannot read at all, which another process may still read as one. A file that
    /// reads as anything else is not this store's, and is never touched.
    fn kind_files(&self, kind: EntryKind) -> std::io::Result<Vec<PathBuf>> {
        let ours = |path: &Path| match std::fs::read(path) {
            Err(_) => true,
            Ok(bytes) => serde_json::from_slice::<Value>(&bytes).is_ok_and(|value| {
                value.get("schema").and_then(Value::as_u64) == Some(u64::from(KIND_ENTRY_SCHEMA))
                    && value.get("kind") == serde_json::to_value(kind).ok().as_ref()
                    && value.get("remote").and_then(Value::as_str) == Some(self.remote.as_str())
                    && reads_whole(kind, value)
            }),
        };
        let listed = match std::fs::read_dir(self.dir.join(kind.directory())) {
            Ok(listed) => listed,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(error),
        };
        // An entry of the directory that cannot be read is an error, not a file to pass over: a
        // write retiring every entry has to have seen every one.
        let mut files = Vec::new();
        for file in listed {
            let path = file?.path();
            if path
                .extension()
                .is_some_and(|extension| extension == "json")
                && ours(&path)
            {
                files.push(path);
            }
        }
        Ok(files)
    }

    /// Forget every entry of `id` beside its task's, after a write that could change them.
    pub(crate) fn invalidate_kinds(&self, id: &NativeId) {
        for kind in EntryKind::ALL {
            let _ = self.retire(&self.kind_path(kind, id));
        }
        let mut command = self.command();
        command.document_served.remove(id);
        command.document_read.remove(id);
    }
}

/// Whether `value` reads whole as an entry of `kind` — every member its type holds, of that type.
fn reads_whole(kind: EntryKind, value: Value) -> bool {
    match kind {
        EntryKind::Project => serde_json::from_value::<ProjectEntry>(value).is_ok(),
        EntryKind::Document => serde_json::from_value::<DocumentEntry>(value).is_ok(),
        EntryKind::Edges
        | EntryKind::ReverseEdges
        | EntryKind::ProjectEdges
        | EntryKind::ProjectReverseEdges => serde_json::from_value::<EdgesEntry>(value).is_ok(),
    }
}

/// The directory `remote`'s verified bytes live in under `dir`.
fn bytes_dir(dir: &Path, remote: &str) -> PathBuf {
    dir.join(format!("bytes-v{KIND_ENTRY_SCHEMA}")).join(remote)
}

/// Remove every entry of these kinds this store wrote under `dir` for `remote`, and every byte
/// entry of that remote, answering how many item entries there were.
///
/// A file is removed only when it reads whole as one of this store's entries — of this schema,
/// this kind and this remote — or is a digest-named file in this remote's own byte directory,
/// or a staging file this store left; anything else in those directories is left where it is.
pub(super) fn empty_kinds(dir: &Path, remote: &str) -> std::io::Result<u64> {
    let gone = |result: std::io::Result<()>| match result {
        Err(error) if error.kind() != std::io::ErrorKind::NotFound => Err(error),
        _ => Ok(()),
    };
    let listed = |directory: &Path| -> std::io::Result<Vec<PathBuf>> {
        match std::fs::read_dir(directory) {
            Ok(listed) => listed
                .map(|file| file.map(|file| file.path()))
                .collect::<std::io::Result<Vec<_>>>(),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
            Err(error) => Err(error),
        }
    };
    let staged = format!(".staged-{remote}-");
    let mut entries = 0;
    for kind in EntryKind::ALL {
        for path in listed(&dir.join(kind.directory()))? {
            let name = path.file_name().unwrap_or_default().to_string_lossy();
            if name.starts_with(&staged) {
                gone(std::fs::remove_file(&path))?;
                continue;
            }
            let ours = std::fs::read(&path)
                .ok()
                .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
                .is_some_and(|value| {
                    value.get("schema").and_then(Value::as_u64)
                        == Some(u64::from(KIND_ENTRY_SCHEMA))
                        && value.get("kind") == serde_json::to_value(kind).ok().as_ref()
                        && value.get("remote").and_then(Value::as_str) == Some(remote)
                        && reads_whole(kind, value)
                });
            if name.ends_with(".json") && ours {
                gone(std::fs::remove_file(&path))?;
                entries += 1;
            }
        }
    }
    // A digest-named file in this remote's own byte directory is one this store wrote, whether
    // or not its bytes still hash to its name: a corrupted one is reclaimed with the rest.
    for path in listed(&bytes_dir(dir, remote))? {
        let name = path.file_name().unwrap_or_default().to_string_lossy();
        if name.starts_with(&staged) || ByteDigest::parse(&name).is_some() {
            gone(std::fs::remove_file(&path))?;
        }
    }
    Ok(entries)
}
