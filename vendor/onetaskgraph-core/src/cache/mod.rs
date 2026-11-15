//! The store's local cache of remote sources.
//!
//! **Opt-in, one source at a time, and keyed on nothing but the change token a source gives.**
//! A source configured with `cache.enabled` has its tasks — each one's fields, metadata,
//! comments, assets and forward and reverse edges — its projects with their forward edges, its
//! documents with the manifest of their image assets, and the verified bytes of those assets kept
//! on this host, one file per item,
//! under a directory named for the remote it reads; `kinds` says how each kind beside a task's
//! is laid out. Every process that configures the same remote shares it: the
//! command line, both SDKs and a Rust caller of the engine.
//!
//! The rule it may never break is that **it never serves stale data**; what it costs is held over
//! an hour of the assumed workload, which it must spend fewer requests on than no cache, rather
//! than read by read. Freshness comes only from tokens the store already holds — a caller's pin, or the version a
//! listing in the same command carried — and an entry is served only when it is at that version
//! or a later one, by the ordering of the source that spelled both. Nothing expires by age, and
//! nothing asks the source whether something changed. A part of a task the source's token does
//! not cover is never served at all.
//!
//! **Releases of different ages share one directory**, so an entry it cannot read — an entry
//! schema it does not know, bytes that do not parse, another remote's — is a miss, never a
//! failure. Each entry is written whole to a staging file and renamed into place, so a reader
//! never sees part of one, and no lock is taken.
//!
//! This is the one place the store keeps a user's work outside the plugin that owns it, and
//! only because a configuration asked it to; `AGENTS.md` names it as that invariant's one
//! exception.

mod kinds;
mod source;

use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering as AtomicOrdering};
use std::sync::{Arc, Mutex, MutexGuard};

use onetaskgraph_plugin_api::{
    Asset, Comment, Cursor, DependencyEdge, DocumentVersionCoverage, ItemKind, NativeId,
    PartOutcome, ProjectVersionCoverage, SourceError, SourceName, Task, VersionCoverage,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::config::{Config, SourceConfig};
use crate::engine::Spent;

pub use kinds::{KIND_ENTRY_SCHEMA, kind_entry_shapes};
pub(crate) use source::CachedSource;

/// The schema every entry this build writes is spelled in.
///
/// A reader that meets any other value treats the entry as a miss: this directory is shared by
/// every release on a host, so an entry a later release wrote in a shape this one does not know
/// must cost a read, never a failure or a misreading.
///
/// Version 2 admits the opaque form of a dependency endpoint among a task's edges, which an
/// earlier release would misread as a native id. Version 3 changes no byte an entry holds: the
/// schema its shapes are digested from now describes a task's classification as its own, never
/// tightened by its project, and an entry version 2 wrote is read as a miss. Version 4 adds a
/// task's `assignees`, held for a source that opted in to them: an entry an earlier release
/// wrote holds none, so it is read as a miss rather than served as an unassigned task.
pub const ENTRY_SCHEMA: u32 = 4;

/// Where a source's cache lives, and what names the remote it caches.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CacheLocation {
    /// The directory.
    pub dir: PathBuf,
    /// A digest of what names the remote — the plugin and its own identity of it.
    pub remote: String,
}

/// Where the source `config` keeps its cache, when it keeps one: `None` for a source with
/// caching off, and for one with no remote.
///
/// `home` is the directory caches default to living under — [`Config::cache_home`], or the
/// process's own when the configuration names none.
#[must_use]
pub fn location(config: &SourceConfig, home: Option<&Path>) -> Option<CacheLocation> {
    if !config.cache().enabled {
        return None;
    }
    let remote = remote_digest(config)?;
    let dir = match &config.cache().dir {
        Some(dir) => dir.clone(),
        None => home?.join("onetaskgraph").join(&remote),
    };
    Some(CacheLocation { dir, remote })
}

/// The digest naming the remote `config` reads — the one its cache directory is named for, its
/// designator is held to and a projection keys its intents by — or `None` for a source with no
/// remote. The one spelling of that identity.
#[must_use]
pub(crate) fn remote_digest(config: &SourceConfig) -> Option<String> {
    let identity = config.plugin().plugin().remote_identity(config.config())?;
    Some(digest(&format!("{}\n{identity}", config.plugin().as_str())))
}

/// Where the source `config` keeps the designator its backend answered, whether or not its
/// cache is on: the path of `designator.json` in the directory the cache lives in — `cache.dir`,
/// or the directory named for the remote under `home` — and the digest of that remote, which a
/// reader holds the file to. `None` for a source with no remote, whose designator costs no request.
///
/// A designator is a digest naming the tracker, never anything of a user's work, so keeping it
/// is not the cache's exception to the rule that nothing is stored outside a plugin.
#[must_use]
pub(crate) fn designator_location(
    config: &SourceConfig,
    home: Option<&Path>,
) -> Option<(PathBuf, String)> {
    let remote = remote_digest(config)?;
    let dir = match &config.cache().dir {
        Some(dir) => dir.clone(),
        None => home?.join("onetaskgraph").join(&remote),
    };
    Some((dir.join("designator.json"), remote))
}

/// The directory caches default to living under for a configuration that names none: the
/// process's own `$XDG_CACHE_HOME`, or `$HOME/.cache`.
#[must_use]
pub fn process_home() -> Option<PathBuf> {
    crate::config::cache_home(&crate::Environment::from_process())
}

/// Every cache a configuration names, by source, each where it lives.
#[must_use]
pub fn locations(config: &Config) -> Vec<(SourceName, CacheLocation)> {
    let home = config
        .cache_home()
        .map(Path::to_path_buf)
        .or_else(process_home);
    config
        .sources()
        .iter()
        .filter_map(|(name, source)| {
            location(source, home.as_deref()).map(|location| (name.clone(), location))
        })
        .collect()
}

/// What `cache clear` emptied: one entry per cache it reached.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CacheCleared {
    /// The caches emptied, by source, in name order.
    pub cleared: Vec<ClearedCache>,
}

/// One cache `cache clear` emptied.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ClearedCache {
    /// The source whose cache it is.
    pub source: SourceName,
    /// The directory it lives in.
    // llmlint: ignore[invalid_states_unrepresentable] A filesystem path reported back as the place a cache lives; it is never parsed or acted on by a reader, and a path is not a value a narrower type describes.
    pub dir: String,
    /// How many item entries it held — tasks', projects', documents' and the forward edges kept
    /// on their own. The verified bytes of their image assets are removed with them and not
    /// counted.
    pub entries: u64,
}

/// Empty the cache of `only`, or of every source the configuration caches.
///
/// Only what this store writes there is removed — the item entries, the verified bytes of their
/// image assets, and the board entry — so a `cache.dir` pointed at a directory holding anything
/// else loses nothing else.
///
/// # Errors
///
/// A source `only` names that the configuration does not cache, and a directory that could not
/// be emptied, are refused with what to do next.
pub fn clear(config: &Config, only: Option<&SourceName>) -> Result<CacheCleared, String> {
    let all = locations(config);
    if let Some(name) = only
        && !all.iter().any(|(source, _)| source == name)
    {
        return Err(if config.incomplete().contains(name) {
            crate::EngineError::IncompleteSource { name: name.clone() }
                .to_string()
                .replacen('\n', "; ", 1)
        } else if config.sources().contains_key(name) {
            format!(
                "source {name} keeps no cache: it has no remote, or its cache.enabled is not \
                 set; next: set sources.{name}.cache.enabled, or name a source that caches"
            )
        } else {
            format!(
                "no source named {name} is configured; next: name a configured source — \
                 `onetaskgraph config show` lists them"
            )
        });
    }
    let mut cleared = Vec::new();
    for (source, location) in all {
        if only.is_some_and(|name| *name != source) {
            continue;
        }
        let entries = SourceCache::empty(&location.dir, &location.remote).map_err(|error| {
            format!(
                "the cache of source {source} at {} could not be emptied: {error}; next: \
                 check that directory's permissions",
                location.dir.display()
            )
        })?;
        cleared.push(ClearedCache {
            source,
            dir: location.dir.display().to_string(),
            entries,
        });
    }
    Ok(CacheCleared { cleared })
}

/// Where one read of a caching source's task was answered from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum Served {
    /// From the cache, with no request.
    Cache,
    /// From the source.
    Remote,
}

/// How one read of a caching source's task used the cache: the `cache` member of its JSON.
///
/// Built only by [`CacheUse::cache`], which carries no reason, and [`CacheUse::remote`], so a
/// served entry never carries one.
// llmlint: ignore-block[invalid_states_unrepresentable] The task this lands fixes the wire shape as `cache: {served: "cache"|"remote", bypassed: null|"<reason>"}`, which both SDKs are generated from; an enum would serialize as another shape. The two constructors below are the only ways one is built, and neither pairs `cache` with a reason.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CacheUse {
    /// Where it was answered from.
    pub served: Served,
    /// Why the cache could not be used at all for it — a part the source's change token does
    /// not cover, or a task that carries no token — or `null` when nothing kept it from being.
    pub bypassed: Option<String>,
}
// llmlint: ignore-end[invalid_states_unrepresentable]

impl CacheUse {
    fn cache() -> Self {
        Self {
            served: Served::Cache,
            bypassed: None,
        }
    }

    fn remote(bypassed: Option<String>) -> Self {
        Self {
            served: Served::Remote,
            bypassed,
        }
    }
}

/// What a write that expected a version of its task came to: the `result`, `fallback` and
/// `spent` members of its JSON.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ExpectedWrite {
    /// What the item holds once the write is done — its version, status and priority — and,
    /// for a write made with `--expect` on a source that reports its writes' parts, what each
    /// mutation it sent came to. Absent for a dry run and for an item nothing was written to
    /// or read of.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<WriteResult>,
    /// That the write had to read its target first, and why — absent when it sent only its
    /// mutations.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fallback: Option<WriteFallback>,
    /// What the write spent at the sources that meter their own requests — the fallback's
    /// read included.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spent: Option<Spent>,
}

/// What an item holds once a write is done, read off the write's own responses — and, for an
/// expected write, what each mutation it sent came to.
///
/// Every write verb and every item a copy lands answers one, with no request added for it: a
/// field the write kept is what the read before it held. `version` is `null` where the source
/// cannot vouch for a token from its write's responses — a GitHub board for an item it has just
/// written, a stdio-hosted plugin, an in-memory source — and otherwise the token a read would
/// then give.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct WriteResult {
    /// The item's version after the write, where the write's responses determine it.
    pub version: Option<String>,
    /// One per mutation an expected write sent, in the order sent — absent for a write made
    /// without `--expect`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parts: Option<Vec<WritePart>>,
    /// The item's status after the write, exactly as `task show` spells it — `null` for a
    /// document, which has none, and where an expected write that failed cannot say.
    #[serde(default)]
    pub status: Option<onetaskgraph_plugin_api::Status>,
    /// A task's priority after the write — `null` for a project or a document, which carry
    /// none, and where an expected write that failed cannot say.
    #[serde(default)]
    pub priority: Option<onetaskgraph_plugin_api::Priority>,
}

impl WriteResult {
    /// What `task` holds, at the version given.
    #[must_use]
    pub fn of_task(task: &onetaskgraph_plugin_api::Task, version: Option<String>) -> Self {
        Self {
            version,
            parts: None,
            status: Some(task.status.clone()),
            priority: Some(task.priority),
        }
    }

    /// This result with `later`'s item fields, keeping the parts an expected write recorded and
    /// the version it read off its last response when `later` has none.
    #[must_use]
    pub fn merged(self, later: Self) -> Self {
        Self {
            version: later.version.or(self.version),
            parts: self.parts.or(later.parts),
            status: later.status.or(self.status),
            priority: later.priority.or(self.priority),
        }
    }
}

/// One mutation of an expected write.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct WritePart {
    /// Which part it was, in the source's words: `fields`, `issue`, `comment`.
    pub part: String,
    /// `landed`, `rejected` with the backend's error, or `unknown`.
    #[serde(flatten)]
    pub outcome: PartOutcome,
    /// Each field it set, with the value the response shows.
    pub fields: std::collections::BTreeMap<String, Value>,
}

/// That an expected write read its target before writing, and why.
// llmlint: ignore-block[invalid_states_unrepresentable] The task this lands fixes the wire shape as `fallback: {read: true, reason}`; the member is present only when a read was made, and the one place one is built sets `read` from that fact, so `false` is never written.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct WriteFallback {
    /// Always `true`: the write made one read.
    pub read: bool,
    /// Why it could not take what it needed from the cache.
    pub reason: String,
}
// llmlint: ignore-end[invalid_states_unrepresentable]

/// One task as the cache keeps it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub(crate) struct Entry {
    /// [`ENTRY_SCHEMA`].
    pub(crate) schema: u32,
    /// The digest of the remote it is from, which a reader holds it to.
    pub(crate) remote: String,
    /// The task, its `id` the entry's own and its `version` the token the entry was read at —
    /// one value each, so an entry cannot vouch for another task or another version than the
    /// one it holds.
    pub(crate) task: Task,
    /// Every comment on it, oldest first — absent when the read that wrote it did not carry
    /// them whole.
    pub(crate) comments: Option<Vec<Comment>>,
    /// Its image assets, absent until a read carried them.
    pub(crate) assets: Option<Vec<Asset>>,
    /// Its forward dependency edges, absent until a read carried them.
    pub(crate) edges: Option<Vec<DependencyEdge>>,
    /// What its source said a write expecting this version could take instead of reading.
    pub(crate) context: Option<Value>,
}

impl Entry {
    /// The token the entry was read at.
    pub(crate) fn version(&self) -> Option<&str> {
        self.task.version.as_deref()
    }
}

/// A board's context, as the cache keeps it.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
struct BoardEntry {
    schema: u32,
    remote: String,
    context: Value,
}

/// A write expected on one task at one version, while the engine runs it.
#[derive(Debug, Clone)]
pub(crate) struct Expectation {
    pub(crate) id: NativeId,
    pub(crate) version: String,
    /// Whether the source took what the cache kept — decided at the first call touching the
    /// task, so every read the write's own verb makes of it first is answered from what was
    /// primed too — or `None` until then.
    pub(crate) primed: Option<Priming>,
}

/// What priming a source for an expected write came to, each with the reason a fallback read
/// would report.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Priming {
    /// The source took what the cache kept; a read can still follow when the write needs what
    /// it did not hold, which `reason` says.
    Primed {
        /// Why a read followed, should one.
        reason: String,
    },
    /// The source took nothing, so the write reads its target, for `reason`.
    Unprimed {
        /// Why.
        reason: String,
    },
}

impl Priming {
    /// Whether the source took what the cache kept.
    pub(crate) fn primed(&self) -> bool {
        matches!(self, Self::Primed { .. })
    }

    /// Why a read followed, should one.
    pub(crate) fn reason(&self) -> &str {
        match self {
            Self::Primed { reason } | Self::Unprimed { reason } => reason,
        }
    }
}

/// Everything one command has learnt about a caching source's items, dropped when it ends.
#[derive(Debug, Default)]
struct Command {
    /// Versions callers pinned project reads to.
    project_pins: HashMap<NativeId, String>,
    /// Versions a project listing in this command carried.
    project_listed: HashMap<NativeId, String>,
    /// Versions callers pinned document reads to.
    document_pins: HashMap<NativeId, String>,
    /// Versions a document listing in this command carried.
    document_listed: HashMap<NativeId, String>,
    /// Versions documents were read from the source at in this command, which the manifest read
    /// next is recorded under.
    document_read: HashMap<NativeId, String>,
    /// Document entries served in this command, whose manifest is served with them.
    document_served: HashMap<NativeId, kinds::DocumentEntry>,
    /// A forward-edge walk of a task or a project under way at the source: the edges it has
    /// answered so far, and the cursor that continues it.
    walking: HashMap<(ItemKind, NativeId), (Vec<DependencyEdge>, Cursor)>,
    /// Versions callers pinned reads to.
    pins: HashMap<NativeId, String>,
    /// Versions a listing in this command carried.
    listed: HashMap<NativeId, String>,
    /// How the last read of each task used the cache.
    used: HashMap<NativeId, CacheUse>,
    /// Versions read from the source in this command, which assets and edges read next are
    /// recorded under.
    read: HashMap<NativeId, String>,
    /// Entries served in this command, whose assets and edges are served with them.
    served: HashMap<NativeId, Entry>,
    /// The write being expected, while it runs.
    expected: Option<Expectation>,
    /// What each expected write came to, until the engine takes it.
    reports: HashMap<NativeId, ExpectedWrite>,
    /// The last expected write's report, with when it was made among every source's, until
    /// anything takes it — for a write that failed.
    last: Option<(u64, ExpectedWrite)>,
}

/// One source's cache: the directory, and what the running command knows about it.
#[derive(Debug)]
pub struct SourceCache {
    dir: PathBuf,
    remote: String,
    coverage: VersionCoverage,
    project_coverage: ProjectVersionCoverage,
    document_coverage: DocumentVersionCoverage,
    command: Mutex<Command>,
    /// Files this process could neither replace nor take out of service, which it never reads
    /// again until a write of its own lands there.
    withheld: Mutex<HashSet<PathBuf>>,
}

/// Distinguishes the staging files one process writes.
static STAGED: AtomicU64 = AtomicU64::new(0);

/// Orders the expected writes of every source of one process, so the last of them is known.
static REPORTED: AtomicU64 = AtomicU64::new(0);

impl SourceCache {
    /// The cache at `location`, for a source whose token covers `coverage` of a task,
    /// `project_coverage` of a project and `document_coverage` of a document.
    pub(crate) fn new(
        location: CacheLocation,
        coverage: VersionCoverage,
        project_coverage: ProjectVersionCoverage,
        document_coverage: DocumentVersionCoverage,
    ) -> Arc<Self> {
        Arc::new(Self {
            dir: location.dir,
            remote: location.remote,
            coverage,
            project_coverage,
            document_coverage,
            command: Mutex::new(Command::default()),
            withheld: Mutex::new(HashSet::new()),
        })
    }

    /// The directory this cache lives in.
    #[must_use]
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    fn command(&self) -> MutexGuard<'_, Command> {
        // What a command learnt is an optimisation: a lock poisoned by a panic elsewhere is
        // read as it stands rather than failing a read over it.
        self.command
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Pin the next reads of `id` in this command to `version`.
    pub(crate) fn pin(&self, id: &NativeId, version: &str) {
        self.command().pins.insert(id.clone(), version.to_owned());
    }

    /// Pin the next reads of project `id` in this command to `version`.
    pub(crate) fn pin_project(&self, id: &NativeId, version: &str) {
        self.command()
            .project_pins
            .insert(id.clone(), version.to_owned());
    }

    /// Pin the next reads of document `id` in this command to `version`.
    pub(crate) fn pin_document(&self, id: &NativeId, version: &str) {
        self.command()
            .document_pins
            .insert(id.clone(), version.to_owned());
    }

    /// How the last read of `id` in this command used the cache.
    pub(crate) fn used(&self, id: &NativeId) -> Option<CacheUse> {
        self.command().used.get(id).cloned()
    }

    /// Forget everything this command learnt.
    pub(crate) fn end_command(&self) {
        *self.command() = Command::default();
    }

    /// Expect the next write of `id` to find it at `version`.
    pub(crate) fn expect(&self, id: &NativeId, version: &str) {
        let mut command = self.command();
        command.reports.remove(id);
        command.expected = Some(Expectation {
            id: id.clone(),
            version: version.to_owned(),
            primed: None,
        });
    }

    /// Stop expecting, answering with what the expected writes of `id` came to.
    pub(crate) fn take_expected(&self, id: &NativeId) -> Option<ExpectedWrite> {
        let mut command = self.command();
        if command
            .expected
            .as_ref()
            .is_some_and(|expected| expected.id == *id)
        {
            command.expected = None;
        }
        command.last = None;
        command.reports.remove(id)
    }

    /// When the last expected write of this cache was made, among every source's, if one is
    /// still untaken.
    pub(crate) fn last_made(&self) -> Option<u64> {
        self.command().last.as_ref().map(|(made, _)| *made)
    }

    /// What the last expected write came to, when nothing has taken it.
    pub(crate) fn take_last(&self) -> Option<ExpectedWrite> {
        let mut command = self.command();
        let (_, last) = command.last.take()?;
        command.reports.clear();
        Some(last)
    }

    /// Record what the expected write of `id` came to, ending the expectation: a later write
    /// of the same task in this command — an undo, a comment the copy carries — is a plain one.
    pub(crate) fn report(&self, id: &NativeId, report: ExpectedWrite) {
        let mut command = self.command();
        command.expected = None;
        command.last = Some((
            REPORTED.fetch_add(1, AtomicOrdering::Relaxed),
            report.clone(),
        ));
        command.reports.insert(id.clone(), report);
    }

    fn task_path(&self, id: &NativeId) -> PathBuf {
        self.dir
            .join("tasks")
            .join(format!("{}.json", digest(id.as_str())))
    }

    fn board_path(&self) -> PathBuf {
        self.dir.join("board.json")
    }

    /// The entry for `id`, or `None` when there is none this build can read.
    pub(crate) fn read(&self, id: &NativeId) -> Option<Entry> {
        let bytes = self.read_file(&self.task_path(id))?;
        let value: Value = serde_json::from_slice(&bytes).ok()?;
        if value.get("schema").and_then(Value::as_u64) != Some(u64::from(ENTRY_SCHEMA)) {
            return None;
        }
        let entry: Entry = serde_json::from_value(value).ok()?;
        (entry.remote == self.remote && entry.task.id == *id).then_some(entry)
    }

    /// Write `entry`, whole, in place of whatever was there.
    ///
    /// A cache that cannot be written is a cache that is not used: the read or write that
    /// produced the entry has its answer either way, so a failure here does not fail it. It
    /// does take the entry already there out of service, because that entry is no longer
    /// what the task was last seen holding.
    pub(crate) fn write(&self, entry: &Entry) {
        let path = self.task_path(&entry.task.id);
        let written = serde_json::to_vec(entry)
            .map_err(std::io::Error::from)
            .and_then(|bytes| self.write_atomically(&path, &bytes));
        if written.is_err() {
            let _ = self.retire(&path);
        }
    }

    /// Make the entry for `id` unserveable: a write whose response could not vouch for what
    /// the task now holds.
    pub(crate) fn invalidate(&self, id: &NativeId) {
        let _ = self.retire(&self.task_path(id));
        self.invalidate_kinds(id);
        let mut command = self.command();
        command.served.remove(id);
        command.read.remove(id);
        command.walking.remove(&(ItemKind::Task, id.clone()));
        command.walking.remove(&(ItemKind::Project, id.clone()));
    }

    /// Take the entry for `id` out of service for every process before a write of it is
    /// sent, or refuse the write.
    ///
    /// Done before the write rather than after it, because what the write sends changes the
    /// task at the source while an entry this process could not remove, empty or re-mode would
    /// still be served to another process — a later command line or SDK call — pinned to the
    /// version it holds. Only a removal, or the entry shown absent, counts: that this process
    /// cannot open the file says nothing about another — it may have descriptors to spare, or
    /// other permissions — so a file still there refuses the write.
    ///
    /// Every entry naming `id` is retired, not only its task's: a write can change a project or
    /// a document of that id, or turn one kind into another.
    pub(crate) fn retire_before_write(&self, id: &NativeId) -> Result<(), SourceError> {
        let path = self.task_path(id);
        match self
            .retire(&path)
            .and_then(|()| self.retire_kinds_before_write(id))
        {
            Ok(()) => Ok(()),
            Err(error) => Err(SourceError::Config {
                message: format!(
                    "the cache entry for {id} at {} could not be removed, emptied or made \
                     writable ({error}), so the write was not sent: once it changed the task, \
                     another process pinned to the entry's version would be served it. Make \
                     the cache directory {} writable, then run the command again",
                    path.display(),
                    self.dir.display()
                ),
            }),
        }
    }

    /// Take every `kind` edges entry out of service for every process before a write that can
    /// change edges recorded on other items than the one it names is sent — a relation is
    /// recorded at both of its ends, and a deleted item takes its relations with it — or refuse
    /// the write, on the terms of [`Self::retire_before_write`].
    pub(crate) fn retire_edges_before_write(
        &self,
        kind: kinds::EdgeKind,
    ) -> Result<(), SourceError> {
        self.retire_every_before_write(kind)
            .map_err(|error| SourceError::Config {
                message: format!(
                    "a cache entry of another item's edges under {} could not be removed, \
                     emptied or made writable ({error}), so the write was not sent: it can \
                     change those edges, and another process pinned to the entry's version \
                     would be served them. Make the cache directory writable, then run the \
                     command again",
                    self.dir.display()
                ),
            })
    }

    /// The board context, when one is kept.
    pub(crate) fn board(&self) -> Option<Value> {
        let bytes = self.read_file(&self.board_path())?;
        let entry: BoardEntry = serde_json::from_slice(&bytes).ok()?;
        (entry.schema == ENTRY_SCHEMA && entry.remote == self.remote).then_some(entry.context)
    }

    /// Keep `context` as the board's.
    pub(crate) fn write_board(&self, context: Value) {
        let entry = BoardEntry {
            schema: ENTRY_SCHEMA,
            remote: self.remote.clone(),
            context,
        };
        let path = self.board_path();
        let written = serde_json::to_vec(&entry)
            .map_err(std::io::Error::from)
            .and_then(|bytes| self.write_atomically(&path, &bytes));
        if written.is_err() {
            let _ = self.retire(&path);
        }
    }

    /// Drop the board context, so the next write needing it reads it.
    pub(crate) fn drop_board(&self) {
        let _ = self.retire(&self.board_path());
    }

    fn withheld(&self) -> MutexGuard<'_, HashSet<PathBuf>> {
        self.withheld
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// The bytes at `path`, unless this process took them out of service and could not
    /// remove them.
    fn read_file(&self, path: &Path) -> Option<Vec<u8>> {
        if self.withheld().contains(path) {
            return None;
        }
        std::fs::read(path).ok()
    }

    /// Take the file at `path` out of service, however much of that the filesystem allows.
    ///
    /// Removed, it is a miss for every process. When the removal is refused it is emptied in
    /// place — an empty file parses as nothing, so that too is a miss for every process — and,
    /// on a host where only its mode stood in the way, made writable first. When even that is
    /// refused — a file this user can neither write nor re-mode — this process withholds it from
    /// itself until a write of its own replaces it. Another process can still read that file,
    /// which is why a write of a task retires its entry with [`Self::retire_before_write`]
    /// first and is not sent when that fails.
    ///
    /// Answers why the file is still there, when it is.
    fn retire(&self, path: &Path) -> std::io::Result<()> {
        let retired = match std::fs::remove_file(path) {
            Ok(()) => Ok(()),
            // Absent, positively: nothing at the path, or a file where one of its directories
            // belongs, under which no entry can be.
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory
                ) =>
            {
                Ok(())
            }
            Err(_) => empty_in_place(path),
        };
        if retired.is_err() {
            self.withheld().insert(path.to_path_buf());
        }
        retired
    }

    /// Stage `bytes` beside `path` and rename them into place, so no reader ever opens part of
    /// them: a rename within one directory replaces the old file whole.
    fn write_atomically(&self, path: &Path, bytes: &[u8]) -> std::io::Result<()> {
        let directory = path.parent().unwrap_or(&self.dir);
        std::fs::create_dir_all(directory)?;
        let staged = directory.join(format!(
            ".staged-{}-{}-{}",
            self.remote,
            std::process::id(),
            STAGED.fetch_add(1, AtomicOrdering::Relaxed)
        ));
        let written = (|| {
            let mut file = std::fs::File::create(&staged)?;
            file.write_all(bytes)?;
            file.sync_all()?;
            std::fs::rename(&staged, path)
        })();
        match written {
            Ok(()) => {
                self.withheld().remove(path);
            }
            Err(_) => {
                let _ = std::fs::remove_file(&staged);
            }
        }
        written
    }

    /// Remove every entry this store wrote under `dir` for `remote`, answering how many item
    /// entries there were.
    ///
    /// A file is removed only when it reads as one of this store's entries for that remote — a
    /// task entry under `tasks/`, an entry of another kind or a verified byte entry
    /// (`kinds::empty_kinds`), the board entry, or a staging file this store left — so a
    /// `cache.dir` pointed at a directory holding anything else, another remote's cache
    /// included, loses nothing else. An entry of a schema this build does not know is another
    /// release's, and is left for that release to clear.
    fn empty(dir: &Path, remote: &str) -> std::io::Result<u64> {
        // A file is the store's when it reads whole as one of its entries, of its schema, for
        // this remote — never by a member or two it happens to share with one.
        let read = |path: &Path| std::fs::read(path).ok();
        let task_entry = |path: &Path| {
            read(path)
                .and_then(|bytes| serde_json::from_slice::<Entry>(&bytes).ok())
                .is_some_and(|entry| entry.schema == ENTRY_SCHEMA && entry.remote == remote)
        };
        let board_entry = |path: &Path| {
            read(path)
                .and_then(|bytes| serde_json::from_slice::<BoardEntry>(&bytes).ok())
                .is_some_and(|entry| entry.schema == ENTRY_SCHEMA && entry.remote == remote)
        };
        let gone = |result: std::io::Result<()>| match result {
            Err(error) if error.kind() != std::io::ErrorKind::NotFound => Err(error),
            _ => Ok(()),
        };
        let mut entries = 0;
        let tasks = dir.join("tasks");
        match std::fs::read_dir(&tasks) {
            Ok(listed) => {
                for file in listed {
                    let path = file?.path();
                    let name = path.file_name().unwrap_or_default().to_string_lossy();
                    if name.starts_with(&format!(".staged-{remote}-")) {
                        gone(std::fs::remove_file(&path))?;
                    } else if name.ends_with(".json") && task_entry(&path) {
                        gone(std::fs::remove_file(&path))?;
                        entries += 1;
                    }
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
        let board = dir.join("board.json");
        if board_entry(&board) {
            gone(std::fs::remove_file(&board))?;
        }
        Ok(entries + kinds::empty_kinds(dir, remote)?)
    }
}

/// Truncate the file at `path` to nothing, making it writable first where its mode is all
/// that refuses.
///
/// Only a regular file is emptied, and only the very file that was inspected: a symlink, or a
/// path replaced between the look and the open, could otherwise point this at a file outside
/// the cache that it has no business changing. That is checked by device and inode, which only
/// a Unix host offers, so elsewhere nothing is emptied: an entry its directory will not let go
/// is withheld, and its task's writes refused, instead.
fn empty_in_place(path: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        empty_inspected(path, &std::fs::symlink_metadata(path)?)
    }
    #[cfg(not(unix))]
    {
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            format!(
                "{} cannot be emptied in place on this platform",
                path.display()
            ),
        ))
    }
}

/// [`empty_in_place`], of the file `inspected` describes, refusing whatever else `path` opens.
#[cfg(unix)]
fn empty_inspected(path: &Path, inspected: &std::fs::Metadata) -> std::io::Result<()> {
    use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};
    if !inspected.file_type().is_file() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "it is not a regular file, so it is not emptied",
        ));
    }
    let open = |write: bool| -> std::io::Result<std::fs::File> {
        let file = std::fs::OpenOptions::new()
            .read(!write)
            .write(write)
            .open(path)?;
        let opened = file.metadata()?;
        if (inspected.dev(), inspected.ino()) != (opened.dev(), opened.ino()) {
            return Err(std::io::Error::other(
                "it was replaced while it was being emptied, so it is not",
            ));
        }
        Ok(file)
    };
    let file = match open(true) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => {
            // Through the handle, so the file re-moded is the one inspected.
            let held = open(false)?;
            let mut permissions = held.metadata()?.permissions();
            permissions.set_mode(permissions.mode() | 0o200);
            held.set_permissions(permissions)?;
            open(true)?
        }
        Err(error) => return Err(error),
    };
    file.set_len(0)?;
    file.sync_all()
}

/// The JSON Schema of every entry this build writes under [`ENTRY_SCHEMA`]: a task's and the
/// board's.
///
/// What `tests/cache.rs` holds that number to: a change to either shape that does not move
/// [`ENTRY_SCHEMA`] would have a release read another's entries as its own, so the shape is
/// pinned beside the number and a change to one without the other fails there.
#[must_use]
pub fn entry_shapes() -> Value {
    serde_json::json!({
        "task": schemars::schema_for!(Entry),
        "board": schemars::schema_for!(BoardEntry),
    })
}

/// The lower-case hex SHA-256 of `text`.
fn digest(text: &str) -> String {
    let mut hex = String::with_capacity(64);
    for byte in Sha256::digest(text.as_bytes()) {
        // Writing to a `String` never fails.
        let _ = write!(hex, "{byte:02x}");
    }
    hex
}
