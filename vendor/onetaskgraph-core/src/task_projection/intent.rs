//! The producer's durable create intent and binding. No tracker client lives here.
// llmlint: ignore-file[code_lands_in_the_domain_that_owns_it] Config, cache and template establish this repository's core-domain convention: a submodule of onetaskgraph-core, not an Nx project for each concern. This producer-owned journal belongs to the task-projection submodule under that same convention, alongside its sole public wire declaration.

use std::fs::{self, File, OpenOptions};
use std::io::Write as _;
use std::path::{Path, PathBuf};

use onetaskgraph_plugin_api::{Classification, SourceName, Task};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

use super::identity::{HostIdentity, PayloadDigest, ProjectionOrigin};
use super::{
    ExternalItemId, HumanTaskProjected, HumanTaskProjection, ProjectedTaskId, ProjectionProjectId,
    ProjectionState, SourceIdentityDigest, TrackerUrl,
};
use crate::GlobalId;

/// A failure to establish a safe durable projection.
#[derive(Debug, thiserror::Error)]
pub enum ProjectionError {
    /// The intent or tracker evidence does not authorize this operation.
    #[error("human task projection refused: {0}")]
    Refused(String),
    /// The durable store could not be read or committed.
    #[error("human task projection durable store: {0}")]
    Io(#[from] std::io::Error),
    /// A journal or request could not be serialized.
    #[error("human task projection journal: {0}")]
    Json(#[from] serde_json::Error),
}

/// Placement validated through the engine before the create intent is committed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectionPlacement {
    /// The same source identity digest used by the remote source's local cache.
    pub identity: SourceIdentityDigest,
    /// The resolved configured source.
    pub source: SourceName,
    /// The existing resolved member project.
    pub project: ProjectionProjectId,
    /// The privacy classification the write was admitted under.
    pub classification: Classification,
}

/// The phase that determines whether sending a create is still permitted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IntentPhase {
    /// No create has been authorized for transmission.
    Ready,
    /// A create may have reached the tracker; only positive reconciliation can bind it.
    Creating,
    /// A canonical tracker identity has been committed.
    Bound,
    /// A failed durable commit requires reopening the claim before any tracker write.
    Uncertain,
}

/// A terminal projection's outcome: only a dismissal carries a reason, and an absent one stays
/// absent.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Outcome {
    Completed,
    Dismissed(Option<String>),
}

impl Outcome {
    /// The outcome `state` and `reason` name, refused for an open state and for a reason beside
    /// a completion.
    fn of(state: ProjectionState, reason: Option<String>) -> Result<Self, ProjectionError> {
        match (state, reason) {
            (ProjectionState::Completed, None) => Ok(Self::Completed),
            (ProjectionState::Completed, Some(_)) => {
                Err(refused("a reason is recorded only for a dismissal"))
            }
            (ProjectionState::Dismissed, reason) => Ok(Self::Dismissed(reason)),
            (ProjectionState::Open, _) => Err(refused("an open state is not a terminal intent")),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(from = "bool", into = "bool")]
enum Acknowledgment {
    Pending,
    Acknowledged,
}

impl From<bool> for Acknowledgment {
    fn from(acknowledged: bool) -> Self {
        if acknowledged {
            Self::Acknowledged
        } else {
            Self::Pending
        }
    }
}

impl From<Acknowledgment> for bool {
    fn from(acknowledgment: Acknowledgment) -> Self {
        acknowledgment == Acknowledgment::Acknowledged
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "StoredTerminal", into = "StoredTerminal")]
struct Terminal {
    outcome: Outcome,
    acknowledged: Acknowledgment,
}

/// A terminal intent as the journal spells it.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredTerminal {
    state: ProjectionState,
    reason: Option<String>,
    acknowledged: Acknowledgment,
}

impl TryFrom<StoredTerminal> for Terminal {
    type Error = String;

    fn try_from(stored: StoredTerminal) -> Result<Self, Self::Error> {
        Ok(Self {
            outcome: Outcome::of(stored.state, stored.reason).map_err(|error| error.to_string())?,
            acknowledged: stored.acknowledged,
        })
    }
}

impl From<Terminal> for StoredTerminal {
    fn from(terminal: Terminal) -> Self {
        let (state, reason) = match terminal.outcome {
            Outcome::Completed => (ProjectionState::Completed, None),
            Outcome::Dismissed(reason) => (ProjectionState::Dismissed, reason),
        };
        Self {
            state,
            reason,
            acknowledged: terminal.acknowledged,
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
enum Phase {
    #[serde(skip)]
    Uncertain {},
    Ready {},
    Creating {},
    Bound {
        binding: HumanTaskProjected,
        terminal: Option<Terminal>,
        /// The ids — never the title or the content — the source gave for the task it wrote,
        /// which a later terminal write takes instead of reading the task again.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        written: Option<serde_json::Value>,
    },
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(try_from = "u32", into = "u32")]
struct JournalVersion;

impl TryFrom<u32> for JournalVersion {
    type Error = String;

    fn try_from(version: u32) -> Result<Self, Self::Error> {
        if version == 1 {
            Ok(Self)
        } else {
            Err(format!("unsupported projection journal version {version}"))
        }
    }
}

impl From<JournalVersion> for u32 {
    fn from(_: JournalVersion) -> Self {
        1
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Journal {
    schema_version: JournalVersion,
    item_id: ExternalItemId,
    host: HostIdentity,
    origin: ProjectionOrigin,
    payload_digest: PayloadDigest,
    placement: ProjectionPlacement,
    phase: Phase,
}

/// A source's kept placement: the immutable knowledge of where it creates — field and
/// repository ids under the plugin's setup-context token — in the store's per-host local cache
/// for that source, keyed by its source identity as the cache's own directory is.
///
/// It holds no item's title, content or membership and authorizes nothing: the plugin takes it
/// only while its token, computed from a read the create makes anyway, still holds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlacementCache {
    path: PathBuf,
}

impl PlacementCache {
    /// The kept placement at `path`, the source's cache directory's `placement.json`.
    #[must_use]
    pub fn at(path: PathBuf) -> Self {
        Self { path }
    }

    /// What was kept, when anything readable was: a context that is not JSON reads as none.
    ///
    /// # Errors
    ///
    /// Refuses a cache that cannot be read.
    pub fn read(&self) -> Result<Option<serde_json::Value>, ProjectionError> {
        match fs::read(&self.path) {
            Ok(bytes) => Ok(serde_json::from_slice(&bytes).ok()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error.into()),
        }
    }

    /// Keep `context` in place of what was kept, atomically.
    ///
    /// # Errors
    ///
    /// Refuses a cache that cannot be written.
    pub fn keep(&self, context: &serde_json::Value) -> Result<(), ProjectionError> {
        let directory = self
            .path
            .parent()
            .ok_or_else(|| refused("a kept placement needs a directory"))?;
        create_directories(directory)?;
        let name = self
            .path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("placement.json");
        let temporary = directory.join(format!("{name}.{}.pending", std::process::id()));
        let mut file = private_options()
            .create(true)
            .truncate(true)
            .write(true)
            .open(&temporary)?;
        file.write_all(&serde_json::to_vec(context)?)?;
        file.sync_all()?;
        fs::rename(temporary, &self.path)?;
        sync_directory(directory)?;
        Ok(())
    }

    /// Forget what was kept, so the next create reads its placement afresh.
    ///
    /// # Errors
    ///
    /// Refuses a cache that cannot be written.
    pub fn forget(&self) -> Result<(), ProjectionError> {
        match fs::remove_file(&self.path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error.into()),
        }
    }
}

/// The local authoritative intent store, shared by processes on one host.
///
/// The caller obtains its host and directory from host-local configuration. This store
/// never arbitrates across hosts; an explicit authority mismatch refuses before a new
/// intent can be written.
#[derive(Debug)]
pub struct IntentStore {
    directory: PathBuf,
    host: HostIdentity,
    current_host: HostIdentity,
}

impl IntentStore {
    /// Select the authoritative host and its durable directory.
    ///
    /// # Errors
    ///
    /// Refuses an invalid host identity or a relative intent directory.
    pub fn new(
        directory: PathBuf,
        host: String,
        current_host: String,
    ) -> Result<Self, ProjectionError> {
        if !directory.is_absolute() {
            return Err(refused(
                "task_projection requires an absolute intent directory",
            ));
        }
        Ok(Self {
            directory,
            host: HostIdentity::new(host).map_err(refused)?,
            current_host: HostIdentity::new(current_host).map_err(refused)?,
        })
    }

    /// Lock one source identity and external item id, then commit or load its intent.
    ///
    /// The lock remains held until the returned claim is dropped. A crash releases the
    /// operating system lock but cannot erase a committed Creating phase.
    ///
    /// # Errors
    ///
    /// Refuses a foreign host, malformed journal, changed payload/placement, or a terminal
    /// projection that conflicts with an earlier terminal intent.
    pub fn claim(
        &self,
        request: &HumanTaskProjection,
        placement: &ProjectionPlacement,
    ) -> Result<ProjectionClaim, ProjectionError> {
        if self.current_host != self.host {
            return Err(refused(format!(
                "current host {:?} is not authoritative host {:?}; inspect sources.{}.task_projection",
                self.current_host.as_str(),
                self.host.as_str(),
                placement.source
            )));
        }
        let payload_digest = payload_digest(request, placement)?;
        let key = digest(&serde_json::to_vec(&(
            &placement.identity,
            &request.item_id,
        ))?);
        let origin =
            ProjectionOrigin::new(format!("human-task:{}:{key}", self.host)).map_err(refused)?;
        let directory = self.directory.join(placement.identity.as_str());
        create_directories(&directory)?;
        let lock = private_options()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(directory.join(format!("{key}.lock")))?;
        lock.lock()?;
        let path = directory.join(format!("{key}.json"));
        let existing = match fs::read(&path) {
            Ok(bytes) => Some(serde_json::from_slice::<Journal>(&bytes)?),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => return Err(error.into()),
        };
        let journal = match existing {
            Some(journal) => {
                if journal.item_id != request.item_id
                    || journal.host != self.host
                    || journal.payload_digest != payload_digest
                    || journal.placement != *placement
                    || journal.origin != origin
                {
                    return Err(refused(
                        "persisted intent differs in owner, payload, route or privacy; reconcile it before projecting again",
                    ));
                }
                if let Phase::Bound {
                    binding, terminal, ..
                } = &journal.phase
                {
                    if !binding_valid(binding, placement) {
                        return Err(refused(
                            "invalid persisted tracker binding; reconcile the durable store",
                        ));
                    }
                    if let Some(terminal) = terminal
                        && Outcome::of(request.state, request.reason.clone())
                            .ok()
                            .as_ref()
                            != Some(&terminal.outcome)
                    {
                        return Err(refused(
                            "conflicting terminal projection; the prior terminal intent is immutable",
                        ));
                    }
                }
                journal
            }
            None => Journal {
                schema_version: JournalVersion,
                item_id: request.item_id.clone(),
                host: self.host.clone(),
                origin,
                payload_digest,
                placement: placement.clone(),
                phase: Phase::Ready {},
            },
        };
        let mut claim = ProjectionClaim {
            path,
            journal,
            _lock: lock,
        };
        claim.persist()?;
        Ok(claim)
    }
}

/// A locked producer intent. Methods commit transitions before handing authority back.
#[derive(Debug)]
pub struct ProjectionClaim {
    path: PathBuf,
    journal: Journal,
    _lock: File,
}

impl ProjectionClaim {
    /// Whether a create has never been sent, needs reconciliation, or is already bound.
    #[must_use]
    pub fn phase(&self) -> IntentPhase {
        match self.journal.phase {
            Phase::Uncertain {} => IntentPhase::Uncertain,
            Phase::Ready {} => IntentPhase::Ready,
            Phase::Creating {} => IntentPhase::Creating,
            Phase::Bound { .. } => IntentPhase::Bound,
        }
    }

    /// The resolved immutable placement this claim is bound to.
    #[must_use]
    pub fn placement(&self) -> &ProjectionPlacement {
        &self.journal.placement
    }

    /// The stable external idempotency identity.
    #[must_use]
    pub fn item_id(&self) -> &str {
        self.journal.item_id.as_str()
    }

    /// The stable origin included in the initial issue body.
    #[must_use]
    pub fn origin(&self) -> &str {
        self.journal.origin.as_str()
    }

    /// The immutable payload and placement digest used to verify recovery candidates.
    #[must_use]
    pub fn payload_digest(&self) -> &str {
        self.journal.payload_digest.as_str()
    }

    /// The authoritative host recorded in the initial issue metadata and binding.
    #[must_use]
    pub fn host(&self) -> &str {
        self.journal.host.as_str()
    }

    /// The canonical identity previously committed, if any.
    #[must_use]
    pub fn binding(&self) -> Option<&HumanTaskProjected> {
        match &self.journal.phase {
            Phase::Bound { binding, .. } => Some(binding),
            Phase::Uncertain {} | Phase::Ready {} | Phase::Creating {} => None,
        }
    }

    /// The ids the source gave for the task it wrote, when the binding kept any.
    #[must_use]
    pub fn written_context(&self) -> Option<&serde_json::Value> {
        match &self.journal.phase {
            Phase::Bound { written, .. } => written.as_ref(),
            Phase::Uncertain {} | Phase::Ready {} | Phase::Creating {} => None,
        }
    }

    /// Whether a terminal intent was committed — acknowledged or not.
    #[must_use]
    pub fn terminal_committed(&self) -> bool {
        matches!(
            &self.journal.phase,
            Phase::Bound {
                terminal: Some(_),
                ..
            }
        )
    }

    /// Durably bind a returned identity with the ids the source gave for what it wrote.
    ///
    /// # Errors
    ///
    /// As [`bind`](Self::bind).
    pub fn bind_written(
        &mut self,
        binding: HumanTaskProjected,
        written: Option<serde_json::Value>,
    ) -> Result<(), ProjectionError> {
        self.bind(binding)?;
        if let Phase::Bound {
            written: held @ None,
            terminal: None,
            ..
        } = &mut self.journal.phase
            && written.is_some()
        {
            *held = written;
            return self.persist();
        }
        Ok(())
    }

    /// Persist that a create may reach the tracker before the caller sends it.
    ///
    /// # Errors
    ///
    /// Refuses a second create, including one after a lost response.
    pub fn begin_create(&mut self) -> Result<(), ProjectionError> {
        if !matches!(self.journal.phase, Phase::Ready {}) {
            return Err(refused(
                "a create may already exist; reconcile by persisted origin instead of creating again",
            ));
        }
        self.journal.phase = Phase::Creating {};
        self.persist()
    }

    /// Durably bind a returned canonical tracker identity before acknowledging it.
    ///
    /// # Errors
    ///
    /// Refuses a different binding or a response incompatible with resolved placement.
    pub fn bind(&mut self, binding: HumanTaskProjected) -> Result<(), ProjectionError> {
        if !binding_valid(&binding, &self.journal.placement) {
            return Err(refused(
                "tracker binding disagrees with the committed source/project",
            ));
        }
        match &self.journal.phase {
            Phase::Bound { binding: held, .. } => {
                return if held == &binding {
                    Ok(())
                } else {
                    Err(refused("a different ticket is already bound"))
                };
            }
            Phase::Uncertain {} => {
                return Err(refused(
                    "a durable commit failed; drop and reopen this claim",
                ));
            }
            Phase::Ready {} => return Err(refused("a create was not durably claimed")),
            Phase::Creating {} => {}
        }
        self.journal.phase = Phase::Bound {
            binding,
            terminal: None,
            written: None,
        };
        self.persist()
    }

    /// Bind exactly one tracker-origin match after verifying immutable identity and fields.
    ///
    /// The caller supplies the complete origin query from the existing source interface.
    /// An empty, multiple, unindexed or unverifiable answer never authorizes a second create.
    ///
    /// # Errors
    ///
    /// Refuses anything other than one verified indexed task with its canonical URL.
    pub fn reconcile(
        &mut self,
        request: &HumanTaskProjection,
        matches: &[Task],
    ) -> Result<(), ProjectionError> {
        if !matches!(self.journal.phase, Phase::Creating {}) || matches.len() != 1 {
            return Err(refused(
                "pending create has zero or multiple origin matches; inspect the tracker and reconcile; no second create was sent",
            ));
        }
        let placement = &self.journal.placement;
        if payload_digest(request, placement)? != self.journal.payload_digest {
            return Err(refused(
                "recovery request differs from the committed payload",
            ));
        }
        let task = &matches[0];
        let metadata_matches = [
            ("human.item_id", self.item_id()),
            (GlobalId::ORIGIN_KEY, self.origin()),
            ("human.payload_digest", self.payload_digest()),
            ("human.host", self.host()),
        ]
        .iter()
        .all(|(key, expected)| {
            task.metadata.get(*key).and_then(serde_json::Value::as_str) == Some(*expected)
        });
        if !metadata_matches
            || task.project.as_ref().map(|id| id.as_str()) != Some(placement.project.as_str())
            || task.classification != placement.classification
            || task.title != request.title.as_str()
            || task.content.as_deref().unwrap_or_default() != request.body
            || task.priority != request.priority
            || repository_set(&task.repositories) != repository_set(&request.repositories)
            || task.metadata.get("human.owner") != Some(&serde_json::to_value(&request.owner)?)
        {
            return Err(refused(
                "origin match is unindexed or differs in item_id, host, route, privacy or payload; inspect and reconcile; no second create was sent",
            ));
        }
        self.bind(HumanTaskProjected {
            task: ProjectedTaskId::try_from(GlobalId::new(
                placement.source.clone(),
                task.id.clone(),
            ))
            .map_err(refused)?,
            url: TrackerUrl::new(
                task.url
                    .clone()
                    .ok_or_else(|| refused("origin match has no canonical issue URL"))?,
            )
            .map_err(refused)?,
            project: placement.project.clone(),
            source: placement.source.clone(),
        })
    }

    /// Commit a terminal intent before writing it, preserving an absent dismissal reason.
    ///
    /// # Errors
    ///
    /// Refuses an open state, unbound task or conflicting terminal intent.
    pub fn begin_terminal(
        &mut self,
        state: ProjectionState,
        reason: Option<String>,
    ) -> Result<(), ProjectionError> {
        let outcome = Outcome::of(state, reason)?;
        let Phase::Bound { terminal, .. } = &mut self.journal.phase else {
            return Err(refused(
                "terminal projection requires an already bound task",
            ));
        };
        if let Some(terminal) = terminal {
            return if terminal.outcome == outcome {
                Ok(())
            } else {
                Err(refused("conflicting terminal projection"))
            };
        }
        *terminal = Some(Terminal {
            outcome,
            acknowledged: Acknowledgment::Pending,
        });
        self.persist()
    }

    /// Whether the terminal write was durably acknowledged.
    #[must_use]
    pub fn terminal_acknowledged(&self) -> bool {
        matches!(
            &self.journal.phase,
            Phase::Bound {
                terminal: Some(Terminal {
                    acknowledged: Acknowledgment::Acknowledged,
                    ..
                }),
                ..
            }
        )
    }

    /// Acknowledge the terminal write after the tracker confirms it.
    ///
    /// # Errors
    ///
    /// Refuses a claim without a terminal intent.
    pub fn acknowledge_terminal(&mut self) -> Result<(), ProjectionError> {
        let Phase::Bound {
            terminal: Some(terminal),
            ..
        } = &mut self.journal.phase
        else {
            return Err(refused("there is no terminal intent to acknowledge"));
        };
        terminal.acknowledged = Acknowledgment::Acknowledged;
        self.persist()
    }

    fn persist(&mut self) -> Result<(), ProjectionError> {
        let result = self.persist_to_disk();
        if result.is_err() {
            self.journal.phase = Phase::Uncertain {};
        }
        result
    }

    fn persist_to_disk(&self) -> Result<(), ProjectionError> {
        let bytes = serde_json::to_vec(&self.journal)?;
        let temporary = self.path.with_extension("pending");
        let mut file = private_options()
            .create(true)
            .truncate(true)
            .write(true)
            .open(&temporary)?;
        file.write_all(&bytes)?;
        file.sync_all()?;
        fs::rename(temporary, &self.path)?;
        sync_directory(self.path.parent().expect("a journal has a directory"))?;
        Ok(())
    }
}

fn payload_digest(
    request: &HumanTaskProjection,
    placement: &ProjectionPlacement,
) -> Result<PayloadDigest, ProjectionError> {
    let mut payload = serde_json::to_value(request)?;
    let fields = payload.as_object_mut().expect("a request is a mapping");
    fields.remove("state");
    fields.remove("reason");
    PayloadDigest::new(digest(&serde_json::to_vec(&(payload, placement))?)).map_err(refused)
}

fn sync_directory(path: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    File::open(path)?.sync_all()?;
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn refused(message: impl Into<String>) -> ProjectionError {
    ProjectionError::Refused(message.into())
}

fn private_options() -> OpenOptions {
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        let mut options = OpenOptions::new();
        options.mode(0o600);
        options
    }
    #[cfg(not(unix))]
    OpenOptions::new()
}

fn create_directories(path: &Path) -> std::io::Result<()> {
    if path.is_dir() {
        return Ok(());
    }
    if let Some(parent) = path.parent() {
        create_directories(parent)?;
    }
    #[cfg(unix)]
    let builder = {
        use std::os::unix::fs::DirBuilderExt as _;
        let mut builder = fs::DirBuilder::new();
        builder.mode(0o700);
        builder
    };
    #[cfg(not(unix))]
    let builder = fs::DirBuilder::new();
    match builder.create(path) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists && path.is_dir() => {}
        Err(error) => return Err(error),
    }
    if let Some(parent) = path.parent() {
        sync_directory(parent)?;
    }
    Ok(())
}

fn binding_valid(binding: &HumanTaskProjected, placement: &ProjectionPlacement) -> bool {
    binding.task.global_id().source == placement.source
        && binding.source == placement.source
        && binding.project == placement.project
}

fn repository_set(
    repositories: &[onetaskgraph_plugin_api::Repository],
) -> std::collections::BTreeSet<&str> {
    repositories
        .iter()
        .map(|repository| repository.as_str())
        .collect()
}
