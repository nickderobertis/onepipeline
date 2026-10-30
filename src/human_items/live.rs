//! The live human-item store: the host-owned journal every item is recorded in,
//! the read index that keeps a warm list from replaying it, and the actions.
//!
//! **The journal is the authority.** `human-items/events.ndjson` beneath the runs
//! root holds one existing journal envelope per item event, on one host stream
//! whose `seq` is allocated under the journal's append lock — the same lock every
//! appender takes — so two processes raising or resolving at once are serialized
//! rather than one lost. A mutation is acknowledged only after its record has
//! been synced; a record the host would not keep is refused out loud.
//!
//! **The index is a cache of the journal, never a second source.** It records the
//! byte position it has folded to and a digest of the record that ends there and
//! of the record that begins the file; a journal replaced, truncated or rewritten
//! under it fails one of those and the index is rebuilt from the journal. An index
//! that is missing, of another version, or whose checksum does not hold is
//! likewise rebuilt — never trusted and never silently partial. A trailing record
//! with no terminator is one a writer is still writing: it is left for the next
//! read rather than parsed or cut.
//!
//! The run DAGs a blocked-step summary is computed from are cached in the same
//! index, each keyed to its own run journal's length and final bytes, under the
//! run's own key: a host stream and a run cannot collide because they are kept in
//! different maps.

use std::collections::{BTreeMap, BTreeSet};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::time::Instant;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::{
    HumanItem, HumanItemAction, HumanItemBlock, HumanItemBlocked, HumanItemBlockedPlan,
    HumanItemDetail, HumanItemEventAction, HumanItemEventPayload, HumanItemFilter, HumanItemInput,
    HumanItemKind, HumanItemNode, HumanItemReceipt, HumanItemRefusal, HumanItemResolution,
    HumanItemState, HumanItemView, Priority, JOURNAL_PATH,
};
use crate::event::{Envelope, Source, ENVELOPE_VERSION};

/// The one stream every host human-item event is written on.
pub(crate) const STREAM: &str = "human-items";
/// The read index's own format version; any other is rebuilt.
const INDEX_VERSION: u32 = 1;
/// How much of a run journal's end its cache key digests.
const RUN_TAIL: u64 = 512;
/// The reason every item a run's end retires carries.
pub const RUN_ENDED: &str = "the run ended";
/// The definitive host outages the engine raises an item for, and nothing else.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum HostCondition {
    /// The human-item journal's filesystem has no space.
    JournalNoSpace,
    /// A dispatch found no space to start in.
    DispatchNoSpace,
    /// Every identity of a dispatched side was refused definitively.
    IdentitiesRefused,
}

impl HostCondition {
    /// The word its items' dedup keys and probe records name it by.
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::JournalNoSpace => "journal-no-space",
            Self::DispatchNoSpace => "dispatch-no-space",
            Self::IdentitiesRefused => "identities-refused",
        }
    }
}

/// The host condition the journal running out of space raises.
pub(crate) const JOURNAL_NO_SPACE: HostCondition = HostCondition::JournalNoSpace;
/// The host condition a dispatch refused for want of space raises.
pub(crate) const DISPATCH_NO_SPACE: HostCondition = HostCondition::DispatchNoSpace;
/// The host condition every identity of a dispatched side being refused raises.
pub(crate) const IDENTITIES_REFUSED: HostCondition = HostCondition::IdentitiesRefused;
/// How often an identity outage is probed, at most.
const IDENTITY_PROBE_EVERY_MS: u64 = 10_000;
/// The dedup-key prefixes only the engine raises under: a run's human step, a
/// configured phase, and a host condition.
const ENGINE_KEYS: [&str; 3] = ["run:", super::phases::PHASE_PREFIX, HOST_CONDITION_PREFIX];
/// The dedup-key prefix of every host condition item, followed by the condition.
pub const HOST_CONDITION_PREFIX: &str = "host:";
/// The environment variable naming a file a read appends its telemetry line to.
pub const TELEMETRY_ENV: &str = "ONEPIPELINE_HUMAN_ITEMS_TELEMETRY";

/// What a completion of a task carrying an approval reference consults: `None`
/// to complete, a replacement when the document moved on, or a refusal.
pub(crate) type Approve<'a> =
    dyn FnMut(&HumanItem, &Actor) -> Result<Option<HumanItemInput>, HumanItemRefusal> + 'a;

fn conflict(reason: impl Into<String>) -> HumanItemRefusal {
    HumanItemRefusal::Conflict {
        reason: reason.into(),
    }
}

fn hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// The stable, host-unique item id a dedup key names: the same key always
/// mints the same id, so a raise retried after a crash finds its own item.
pub fn item_id_for(dedup_key: &str) -> String {
    format!("hi-{}", &hex(dedup_key.as_bytes())[..20])
}

/// Who an action was taken by: the human directly, or a manager session
/// recording the human's chat input on their behalf.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Actor {
    /// Who took the action.
    // llmlint: ignore[invalid_states_unrepresentable] the approved resolution records `actor` and `on_behalf_of` as strings on the published HumanItemResolution; this carries exactly those two values to it, minted only by `from_environment` and the engine's own `engine` actor.
    pub actor: String,
    /// The human a manager acted for, when one did.
    pub on_behalf_of: Option<String>,
}

impl Actor {
    /// The actor this process acts as: a manager session when
    /// `ONEPIPELINE_LAUNCHER_SESSION` names one, acting on behalf of the login
    /// user; otherwise the login user directly.
    pub fn from_environment() -> Self {
        let user = ["USER", "USERNAME", "LOGNAME"]
            .iter()
            .find_map(|name| std::env::var(name).ok().filter(|v| !v.trim().is_empty()))
            .unwrap_or_else(|| "operator".to_owned());
        match std::env::var(crate::sys::LAUNCHER_SESSION_ENV)
            .ok()
            .filter(|v| !v.trim().is_empty())
        {
            Some(session) => Self {
                actor: format!("session:{session}"),
                on_behalf_of: Some(format!("user:{user}")),
            },
            None => Self {
                actor: format!("user:{user}"),
                on_behalf_of: None,
            },
        }
    }
}

// llmlint: ignore-block[invalid_states_unrepresentable] the private read index's cache of a run's DAG, rebuilt from the run journal through `checkpoint::current` whenever its key moves and checksummed with the index; its ids are the graph's own validated node ids and the run's launch project, and the two flags are what the derivation read off the node's kind and status.
/// One node of a run's DAG as the blocked-step derivation reads it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct DagNode {
    pub id: String,
    pub title: String,
    pub deps: Vec<String>,
    pub human: bool,
    /// Not yet dispatched: pending, ready, waiting or blocked.
    pub undispatched: bool,
}

/// One run's DAG, cached under its journal's position.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RunDag {
    journal_len: u64,
    tail: String,
    pub plan: String,
    pub nodes: Vec<DagNode>,
}
// llmlint: ignore-end[invalid_states_unrepresentable]

/// Where the index's fold of the host journal stands.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Position {
    consumed: u64,
    records: u64,
    next_seq: u64,
    head: String,
    tail: String,
    tail_len: u64,
}

/// The read index: private, versioned, checksummed and rebuildable.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Index {
    version: u32,
    journal: Position,
    items: BTreeMap<String, HumanItem>,
    events: BTreeMap<String, String>,
    runs: BTreeMap<String, RunDag>,
    checksum: String,
}

impl Index {
    fn sealed(mut self) -> Self {
        self.checksum = String::new();
        let body = serde_json::to_vec(&self).unwrap_or_default();
        self.checksum = hex(&body);
        self
    }
    fn intact(&self) -> bool {
        let mut copy = self.clone();
        copy.checksum = String::new();
        let body = serde_json::to_vec(&copy).unwrap_or_default();
        self.version == INDEX_VERSION && hex(&body) == self.checksum
    }
}

/// How a read reached its state, for the telemetry a budget reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ReadMode {
    /// Nothing new: the index answered alone.
    Warm,
    /// Only records appended past the index were read.
    Tail,
    /// The index was absent or untrustworthy and the journal was replayed.
    Cold,
}

/// What one read cost, recorded per process for the human-items budget.
#[derive(Debug, Clone, Serialize)]
pub struct ReadTelemetry {
    /// Warm, tail or cold.
    pub mode: ReadMode,
    /// Why a cold read was cold, where it was.
    pub rebuilt_because: Option<String>,
    /// Host journal records folded by this read.
    pub item_events_replayed: u64,
    /// Host journal bytes folded by this read.
    pub item_bytes_replayed: u64,
    /// Run journals whose DAG this read derived again.
    pub runs_rederived: u64,
    /// Run journal records folded to derive them.
    pub run_events_replayed: u64,
    /// Runs discovered under the root.
    pub runs_discovered: u64,
    /// Milliseconds spent discovering runs and checking their cache keys.
    pub discovery_ms: f64,
    /// Milliseconds spent replaying journals.
    pub replay_ms: f64,
    /// Milliseconds spent reading and writing the index.
    pub index_io_ms: f64,
    /// Milliseconds spent computing blocked work.
    pub blocker_ms: f64,
    /// Why the caught-up index could not be written back, where it could not.
    pub index_unwritten: Option<String>,
}

impl ReadTelemetry {
    fn new() -> Self {
        Self {
            mode: ReadMode::Warm,
            rebuilt_because: None,
            item_events_replayed: 0,
            item_bytes_replayed: 0,
            runs_rederived: 0,
            run_events_replayed: 0,
            runs_discovered: 0,
            discovery_ms: 0.0,
            replay_ms: 0.0,
            index_io_ms: 0.0,
            blocker_ms: 0.0,
            index_unwritten: None,
        }
    }
}

fn ms(since: Instant) -> f64 {
    since.elapsed().as_secs_f64() * 1000.0
}

/// A failure reading or writing the store, carried as the typed refusal every
/// caller already handles.
fn storage(what: &str, path: &Path, error: impl std::fmt::Display) -> HumanItemRefusal {
    conflict(format!(
        "the human-item store could not {what} {}: {error}",
        path.display()
    ))
}

/// The live human-item store under one runs root, acting under one effective
/// configuration.
#[derive(Debug, Clone)]
pub struct HumanItems {
    root: PathBuf,
    config: super::config::EffectiveConfig,
    /// Where the store `task_source` names is configured from: the checkout a
    /// command runs in, or a run's launch directory.
    dir: PathBuf,
}

/// The state a read or a write works from: every item, and every run DAG.
pub(crate) struct Snapshot {
    pub items: BTreeMap<String, HumanItem>,
    events: BTreeMap<String, String>,
    pub runs: BTreeMap<String, RunDag>,
}

impl HumanItems {
    /// The store under `root`, the runs root this process reads and writes,
    /// acting under `config` — one the effective loader produced and validated.
    pub(crate) fn at(root: impl Into<PathBuf>, config: super::config::EffectiveConfig) -> Self {
        Self {
            root: root.into(),
            config,
            dir: std::env::current_dir().unwrap_or_default(),
        }
    }

    /// This store, projecting through the plan store configured from `dir`.
    #[must_use]
    pub fn in_dir(mut self, dir: impl Into<PathBuf>) -> Self {
        self.dir = dir.into();
        self
    }

    /// The store under `root` for a consumer working in `dir`: the effective
    /// configuration for `dir` is loaded, and a configuration that does not
    /// enable human items is refused as unavailable.
    ///
    /// # Errors
    ///
    /// [`HumanItemRefusal::Unavailable`] when disabled, and a conflict naming
    /// the file when the configuration is malformed.
    pub fn open(root: impl Into<PathBuf>, dir: &Path) -> Result<Self, HumanItemRefusal> {
        let config = super::config::effective(dir).map_err(conflict)?;
        if !config.human_items.enabled {
            return Err(HumanItemRefusal::Unavailable);
        }
        Ok(Self::at(root, config).in_dir(dir))
    }

    /// The effective configuration this store acts under.
    pub fn config(&self) -> &super::config::EffectiveConfig {
        &self.config
    }

    /// Raise what a configured phase's hook expands to against `context`.
    /// Answers the receipt of the item raised, or `None` when the phase raised
    /// nothing (a review with no tickets).
    ///
    /// # Errors
    ///
    /// An unconfigured phase, an expansion that is not a valid item, or a
    /// refusal of the raise itself.
    pub fn raise_phase(
        &self,
        phase: &str,
        context: &super::phases::PhaseContext,
    ) -> Result<Option<HumanItemReceipt>, HumanItemRefusal> {
        let expanded = super::phases::expand(&self.config, phase, context).map_err(conflict)?;
        let engine = Actor {
            actor: "engine".into(),
            on_behalf_of: None,
        };
        let mut writer = Writer::open(self)?;
        let (receipt, retired) = match expanded {
            super::phases::Expanded::Raise {
                item,
                findings,
                retired,
            } => {
                let receipt = writer.raise(*item)?;
                for finding in findings {
                    writer.raise(finding)?;
                }
                (Some(receipt), retired)
            }
            super::phases::Expanded::Retire(retired) => (None, retired),
        };
        for (dedup_key, reason) in retired {
            let id = item_id_for(&dedup_key);
            if let Some(item) = writer.snapshot.items.get(&id).cloned() {
                if item.state == HumanItemState::Open {
                    writer.resolve(
                        &item,
                        HumanItemState::Dismissed,
                        HumanItemEventAction::Dismissed,
                        &engine,
                        None,
                        Some(reason),
                    )?;
                }
            }
        }
        writer.finish();
        Ok(receipt)
    }

    fn dir(&self) -> PathBuf {
        self.journal()
            .parent()
            .map_or_else(|| self.root.clone(), Path::to_path_buf)
    }
    pub(crate) fn journal(&self) -> PathBuf {
        self.root.join(JOURNAL_PATH)
    }
    fn index(&self) -> PathBuf {
        self.dir().join("index.json")
    }
    fn index_lock(&self) -> PathBuf {
        self.dir().join("index.lock")
    }

    /// Every matching item, ordered blocking first, then by priority, then by
    /// creation and id; state defaults to open.
    pub fn query(&self, filter: &HumanItemFilter) -> Result<Vec<HumanItemView>, HumanItemRefusal> {
        let (snapshot, mut telemetry) = self.recovered()?;
        let started = Instant::now();
        let views = query(&snapshot, filter);
        telemetry.blocker_ms += ms(started);
        self.report(&telemetry);
        Ok(views)
    }

    /// One item with every blocked step.
    pub fn show(&self, id: &str) -> Result<HumanItemDetail, HumanItemRefusal> {
        let (snapshot, mut telemetry) = self.recovered()?;
        let started = Instant::now();
        let item = snapshot
            .items
            .get(id)
            .ok_or_else(|| conflict(format!("no such human item {id}")))?;
        let detail = detail(&snapshot, item);
        telemetry.blocker_ms += ms(started);
        self.report(&telemetry);
        Ok(detail)
    }

    /// Apply one action, durably, and answer its receipt. A raise naming an
    /// approval reference is refused: only a configured phase may raise a task
    /// that completes through an approval authority.
    pub fn apply(&self, action: HumanItemAction) -> Result<HumanItemReceipt, HumanItemRefusal> {
        if let HumanItemAction::Raise(input) = &action {
            // The engine's own namespaces: a run's human step, a configured phase and
            // a host condition each act on what their key names, so no caller mints one.
            if let Some(reserved) = ENGINE_KEYS
                .iter()
                .find(|prefix| input.dedup_key.starts_with(*prefix))
            {
                return Err(conflict(format!(
                    "dedup keys beginning {reserved:?} are the engine's own; raise under another key"
                )));
            }
            if input.approval.is_some() {
                return Err(conflict(
                    "an approval reference is accepted only from a configured phase: raise it \
                     with --phase, whose hook names the approval adapter",
                ));
            }
        }
        let config = self.config.clone();
        let receipt = self.apply_with(action, &mut |item, actor| {
            super::approval::complete(&config, item, actor)
        })?;
        // The resolution is durable; the human step it stands for is released
        // through its run's own commands. One that cannot be released now is
        // released by the run's driver, so this is said and the receipt stands.
        if let Err(why) = super::runtime::release(&self.root, &receipt.item.item) {
            eprintln!("onepipeline: {why}");
        }
        Ok(receipt)
    }

    /// Apply one action, consulting `complete` for a task carrying an approval
    /// reference: it answers `Ok(None)` to complete, `Ok(Some(replacement))` for
    /// changed evidence (the obsolete task is dismissed and the replacement
    /// raised), or a refusal that leaves the item open.
    pub(crate) fn apply_with(
        &self,
        action: HumanItemAction,
        complete: &mut Approve<'_>,
    ) -> Result<HumanItemReceipt, HumanItemRefusal> {
        let mut writer = Writer::open(self)?;
        let receipt = writer.apply(action, complete)?;
        writer.finish();
        Ok(receipt)
    }

    /// Dismiss every open item holding the work of `run` — its work and its plan
    /// holds — with the reason [`RUN_ENDED`]. Host holds and items held by
    /// nothing are left open. Answers how many were retired.
    pub fn run_ended(&self, run: &str) -> Result<usize, HumanItemRefusal> {
        let mut writer = Writer::open(self)?;
        let targets: Vec<HumanItem> = writer
            .snapshot
            .items
            .values()
            .filter(|item| item.state == HumanItemState::Open)
            .filter(|item| match &item.block {
                HumanItemBlock::Work { run: held, .. } => held == run,
                // A plan hold the run raised is that run's; one raised outside
                // it, or by another run of the plan, outlives it.
                HumanItemBlock::Plan { .. } => item.owner.run.as_deref() == Some(run),
                HumanItemBlock::Host | HumanItemBlock::Nothing => false,
            })
            .cloned()
            .collect();
        for item in &targets {
            writer.resolve(
                item,
                HumanItemState::Dismissed,
                HumanItemEventAction::AutoResolved,
                &Actor {
                    actor: "engine".into(),
                    on_behalf_of: None,
                },
                None,
                Some(RUN_ENDED.to_owned()),
            )?;
        }
        writer.finish();
        Ok(targets.len())
    }

    /// Raise one urgent host item for a definitive host condition, owned by
    /// `session`, unless one is already open for that condition.
    pub(crate) fn raise_condition(
        &self,
        condition: HostCondition,
        session: &str,
        title: &str,
        body: &str,
        probe: Probe,
    ) -> Result<HumanItemReceipt, HumanItemRefusal> {
        let mut known = probes(self);
        known.insert(condition.as_str().to_owned(), probe);
        keep_probes(self, &known);
        let mut writer = Writer::open(self)?;
        let receipt = writer.condition(condition, session, title, body)?;
        writer.finish();
        Ok(receipt)
    }

    fn report(&self, telemetry: &ReadTelemetry) {
        record_telemetry(&serde_json::json!({"pid": std::process::id(), "read": telemetry}));
    }

    /// The caught-up snapshot, after probing a journal outage it records as
    /// open: a writer's open allocates the reserve again, and that allocation is
    /// the probe. Only the failed capability is asked; nothing is dispatched.
    fn recovered(&self) -> Result<(Snapshot, ReadTelemetry), HumanItemRefusal> {
        let (snapshot, telemetry) = self.read()?;
        let open = snapshot.items.values().any(|i| {
            i.state == HumanItemState::Open && i.dedup_key.starts_with(HOST_CONDITION_PREFIX)
        });
        if !open {
            return Ok((snapshot, telemetry));
        }
        self.probe();
        Ok((self.read()?.0, telemetry))
    }

    /// Project every task whose state is ahead of what was last projected into
    /// the configured `task_source`, outside the journal's lock. A refusal is
    /// said and retried by the next write; the local item and its hold stand.
    fn project_pending(&self) {
        thread_local! {
            static PROJECTING: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
        }
        let Some(source) = self.config.human_items.task_source.clone() else {
            return;
        };
        if PROJECTING.with(std::cell::Cell::get) {
            return;
        }
        PROJECTING.with(|p| p.set(true));
        let journal = self.journal();
        let pending = self
            .items()
            .map(|items| super::projection::pending(&journal, items.values()))
            .unwrap_or_default();
        for item in pending {
            match super::projection::project(&self.dir, &journal, &source, &item) {
                Ok(tracker) => {
                    super::projection::record(&journal, &item);
                    if item.state == HumanItemState::Open && item.tracker.as_ref() != Some(&tracker)
                    {
                        // llmlint: ignore[changed_behavior_has_e2e] a binding the journal refuses is a refusal of the append every write makes, which the full-disk journeys drive; the item is then open with no tracker, which `projection::pending` owes again, and the next write's re-projection is answered by the store's intent with this same task — the replay `tasks_project_once_into_the_task_source_and_questions_never_do` drives.
                        if let Err(why) = self.bind_tracker(&item.id, tracker) {
                            eprintln!(
                                "onepipeline: {} was projected and its tracker not recorded \
                                 ({why}); the next write binds it again",
                                item.id
                            );
                        }
                    }
                }
                Err(why) => eprintln!(
                    "onepipeline: the task {} is recorded and held as it is, and its projection \
                     into {source} failed: {why}; the next human-items write projects it again",
                    item.id
                ),
            }
        }
        PROJECTING.with(|p| p.set(false));
    }

    fn bind_tracker(
        &self,
        id: &str,
        tracker: super::HumanItemTracker,
    ) -> Result<(), HumanItemRefusal> {
        let mut writer = Writer::open(self)?;
        if let Some(mut item) = writer.snapshot.items.get(id).cloned() {
            if item.state == HumanItemState::Open {
                item.tracker = Some(tracker);
                item.updated_at = crate::sys::now_rfc3339();
                writer.append(&item, HumanItemEventAction::Raised)?;
            }
        }
        writer.finish();
        Ok(())
    }

    /// Probe every open host condition by the capability that failed; a writer's
    /// open is where each probe is made.
    pub(crate) fn probe(&self) {
        if let Ok(writer) = Writer::open(self) {
            writer.finish();
        }
    }

    /// Every item, caught up with the journal, without reading any run: what a
    /// run's driver asks on its passes.
    pub(crate) fn items(&self) -> Result<BTreeMap<String, HumanItem>, HumanItemRefusal> {
        Ok(self.read_with(false)?.0.items)
    }

    /// Raise an item under one of the engine's own keys: a run's human step.
    pub(crate) fn raise_engine(
        &self,
        input: HumanItemInput,
    ) -> Result<HumanItemReceipt, HumanItemRefusal> {
        let mut writer = Writer::open(self)?;
        let receipt = writer.raise(input)?;
        writer.finish();
        Ok(receipt)
    }

    /// Resolve one open item automatically with `reason`. Answers whether it was open.
    pub(crate) fn auto_resolve_item(
        &self,
        id: &str,
        reason: &str,
    ) -> Result<bool, HumanItemRefusal> {
        let mut writer = Writer::open(self)?;
        let Some(item) = writer.snapshot.items.get(id).cloned() else {
            return Ok(false);
        };
        if item.state != HumanItemState::Open {
            return Ok(false);
        }
        writer.resolve(
            &item,
            HumanItemState::Dismissed,
            HumanItemEventAction::AutoResolved,
            &Actor {
                actor: "engine".into(),
                on_behalf_of: None,
            },
            None,
            Some(reason.to_owned()),
        )?;
        writer.finish();
        Ok(true)
    }

    /// Catch the index up under its lock, and answer the snapshot it holds.
    fn read(&self) -> Result<(Snapshot, ReadTelemetry), HumanItemRefusal> {
        self.read_with(true)
    }

    fn read_with(&self, runs: bool) -> Result<(Snapshot, ReadTelemetry), HumanItemRefusal> {
        let lock_path = self.index_lock();
        std::fs::create_dir_all(self.dir()).map_err(|e| storage("create", &self.dir(), e))?;
        let _lock = crate::sys::open_locked_append(&lock_path)
            .map_err(|e| storage("lock", &lock_path, e))?;
        let mut telemetry = ReadTelemetry::new();
        let index = self.caught_up(&mut telemetry, runs)?;
        Ok((
            Snapshot {
                items: index.items,
                events: index.events,
                runs: index.runs,
            },
            telemetry,
        ))
    }

    /// The index brought up to the journal and to every run journal, written
    /// back when it moved. Called with the index lock held.
    fn caught_up(
        &self,
        telemetry: &mut ReadTelemetry,
        runs: bool,
    ) -> Result<Index, HumanItemRefusal> {
        let started = Instant::now();
        let stored: Option<Index> = std::fs::read(self.index())
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok());
        telemetry.index_io_ms += ms(started);
        let (mut index, mut moved) = match stored {
            Some(index) if index.intact() => match self.still_extends(&index.journal) {
                Ok(()) => (index, false),
                Err(why) => {
                    telemetry.mode = ReadMode::Cold;
                    telemetry.rebuilt_because = Some(why);
                    (fresh(), true)
                }
            },
            Some(_) => {
                telemetry.mode = ReadMode::Cold;
                telemetry.rebuilt_because =
                    Some("the index is of another version or its checksum fails".into());
                (fresh(), true)
            }
            None => {
                telemetry.mode = ReadMode::Cold;
                telemetry.rebuilt_because = Some(if self.index().exists() {
                    "the index cannot be parsed".into()
                } else {
                    "there is no index".into()
                });
                (fresh(), true)
            }
        };
        let replay = Instant::now();
        let folded = self.fold_tail(&mut index)?;
        telemetry.replay_ms += ms(replay);
        if folded.0 > 0 {
            moved = true;
            if telemetry.mode == ReadMode::Warm {
                telemetry.mode = ReadMode::Tail;
            }
        }
        telemetry.item_events_replayed += folded.0;
        telemetry.item_bytes_replayed += folded.1;
        if runs {
            moved |= self.refresh_runs(&mut index, telemetry);
        }
        if moved {
            let started = Instant::now();
            let sealed = index.clone().sealed();
            let bytes =
                serde_json::to_vec(&sealed).map_err(|e| storage("serialize", &self.index(), e))?;
            // A projection: the journal rebuilds it, so it is renamed into place
            // without the syncs a record takes.
            // Best effort: what this read answers is the journal's, and a host
            // with no room for the index — a full disk among them — costs the
            // next reader a replay, never this reader its answer.
            if let Err(why) = crate::ledger::write_atomic(
                &self.index(),
                &bytes,
                crate::ledger::Durability::Projection,
            ) {
                telemetry.index_unwritten = Some(why.to_string());
            }
            telemetry.index_io_ms += ms(started);
            index = sealed;
        }
        Ok(index)
    }

    /// Whether the journal still holds, unchanged, what the index folded.
    fn still_extends(&self, position: &Position) -> Result<(), String> {
        if position.consumed == 0 {
            return Ok(());
        }
        let path = self.journal();
        let mut file = std::fs::File::open(&path)
            .map_err(|e| format!("the journal the index folded cannot be opened: {e}"))?;
        let length = file.metadata().map_err(|e| e.to_string())?.len();
        if length < position.consumed {
            return Err(format!(
                "the journal is {length} bytes, shorter than the {} the index folded: it was truncated or replaced",
                position.consumed
            ));
        }
        let mut tail = vec![0u8; usize::try_from(position.tail_len).unwrap_or(0)];
        file.seek(SeekFrom::Start(position.consumed - position.tail_len))
            .and_then(|_| file.read_exact(&mut tail))
            .map_err(|e| e.to_string())?;
        if hex(&tail) != position.tail {
            return Err(
                "the record the index folded last is not where it was: the journal was replaced"
                    .into(),
            );
        }
        let head = first_line(&mut file).map_err(|e| e.to_string())?;
        if hex(&head) != position.head {
            return Err("the journal's first record differs from the one the index folded: the journal was replaced".into());
        }
        Ok(())
    }

    /// Fold every whole record past the index's position. Answers how many
    /// records and bytes were folded.
    fn fold_tail(&self, index: &mut Index) -> Result<(u64, u64), HumanItemRefusal> {
        let path = self.journal();
        let mut file = match std::fs::File::open(&path) {
            Ok(file) => file,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok((0, 0)),
            Err(e) => return Err(storage("open", &path, e)),
        };
        file.seek(SeekFrom::Start(index.journal.consumed))
            .map_err(|e| storage("read", &path, e))?;
        let mut rest = Vec::new();
        file.read_to_end(&mut rest)
            .map_err(|e| storage("read", &path, e))?;
        let mut folded = (0, 0);
        let mut offset = 0usize;
        while let Some(end) = rest[offset..].iter().position(|b| *b == b'\n') {
            let line = &rest[offset..offset + end];
            let at = index.journal.consumed;
            let (seq, payload) = parse_record(line).map_err(|why| {
                conflict(format!(
                    "the human-item journal {} holds a record at byte {at} that cannot be read ({why}); \
                     it is never skipped, so nothing is listed until it is repaired",
                    path.display()
                ))
            })?;
            // One writer at a time allocates each sequence above the last, so a
            // record at or below it is a journal no writer made: refused, never folded.
            if index.journal.records > 0 && seq < index.journal.next_seq {
                return Err(conflict(format!(
                    "the human-item journal {} holds a record at byte {at} numbered {seq}, not \
                     above the {} before it; it is never folded, so nothing is listed until it \
                     is repaired",
                    path.display(),
                    index.journal.next_seq - 1
                )));
            }
            if index.journal.records == 0 {
                index.journal.head = hex(line);
            }
            index.journal.consumed += end as u64 + 1;
            index.journal.records += 1;
            index.journal.tail = hex(&rest[offset..offset + end + 1]);
            index.journal.tail_len = end as u64 + 1;
            let after = seq.checked_add(1).ok_or_else(|| {
                conflict(format!(
                    "the human-item journal {} holds a record at byte {at} numbered {seq}, past \
                     the last a journal can number; it is never folded",
                    path.display()
                ))
            })?;
            index.journal.next_seq = index.journal.next_seq.max(after);
            index.events.insert(payload.item_id.clone(), event_id(seq));
            index.items.insert(payload.item_id.clone(), payload.item);
            folded.0 += 1;
            folded.1 += end as u64 + 1;
            offset += end + 1;
        }
        Ok(folded)
    }

    /// Bring every discovered run's DAG up to its journal. Answers whether any
    /// entry changed.
    fn refresh_runs(&self, index: &mut Index, telemetry: &mut ReadTelemetry) -> bool {
        let started = Instant::now();
        let runs = crate::ledger::all_runs(&self.root);
        telemetry.runs_discovered = runs.runs.len() as u64;
        let mut moved = false;
        let mut seen = BTreeSet::new();
        for paths in &runs.runs {
            seen.insert(paths.run.clone());
            let journal = paths.journal();
            let Ok((length, tail)) = run_key(&journal) else {
                continue;
            };
            if index
                .runs
                .get(&paths.run)
                .is_some_and(|dag| dag.journal_len == length && dag.tail == tail)
            {
                continue;
            }
            let replay = Instant::now();
            let before = crate::loopstats::records_folded_total();
            if let Some(dag) = derive_dag(paths, length, tail) {
                index.runs.insert(paths.run.clone(), dag);
                moved = true;
            }
            telemetry.run_events_replayed +=
                crate::loopstats::records_folded_total().saturating_sub(before);
            telemetry.runs_rederived += 1;
            telemetry.replay_ms += ms(replay);
        }
        let before = index.runs.len();
        index.runs.retain(|run, _| seen.contains(run));
        moved |= index.runs.len() != before;
        telemetry.discovery_ms += ms(started);
        moved
    }
}

/// Where each host condition is probed, kept beside the journal.
fn probes_path(store: &HumanItems) -> PathBuf {
    store.dir().join("probes.json")
}

/// How one open host condition is probed: by the capability that failed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "probe", rename_all = "kebab-case", deny_unknown_fields)]
pub(crate) enum Probe {
    /// A file written in the directory a dispatch could not write.
    Space {
        /// The directory.
        dir: PathBuf,
    },
    /// The harness's zero-turn usage read of the refused identities.
    Identities {
        /// Where the harness configuration is discovered from.
        cwd: Option<PathBuf>,
        /// The identities refused, as the chain named them.
        identities: Vec<String>,
        /// When they were last probed, in epoch milliseconds.
        #[serde(default)]
        probed_at: Option<u64>,
    },
}

/// The probe of each open condition. A document that is not one is said and
/// read as none, so each outage it held is probed again once it is raised again.
fn probes(store: &HumanItems) -> BTreeMap<String, Probe> {
    let path = probes_path(store);
    let Ok(bytes) = std::fs::read(&path) else {
        return BTreeMap::new();
    };
    // llmlint: ignore[changed_behavior_has_e2e] `keep_probes` replaces this record whole through `write_atomic`, so an unreadable one is a hand edit; reading it as none, said, leaves the outage's item open for a person to dismiss, and the next occurrence of the condition records its probe again through the path `a_dispatch_refused_for_space_raises_one_host_item_that_its_probe_resolves` drives.
    serde_json::from_slice(&bytes).unwrap_or_else(|why| {
        eprintln!(
            "onepipeline: {} is not a probe record ({why}); its outages are not probed until \
             they are raised again",
            path.display()
        );
        BTreeMap::new()
    })
}

fn keep_probes(store: &HumanItems, known: &BTreeMap<String, Probe>) {
    if let Err(why) = crate::ledger::write_atomic(
        &probes_path(store),
        &serde_json::to_vec(known).unwrap_or_default(),
        crate::ledger::Durability::Record,
    ) {
        eprintln!("onepipeline: the human-item probe record could not be written: {why}");
    }
}

/// Append one telemetry line to the file [`TELEMETRY_ENV`] names, if it names one.
pub(crate) fn record_telemetry(line: &serde_json::Value) {
    let Some(path) = std::env::var_os(TELEMETRY_ENV).filter(|v| !v.is_empty()) else {
        return;
    };
    if let Err(error) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .and_then(|mut file| file.write_all(format!("{line}\n").as_bytes()))
    {
        eprintln!(
            "onepipeline: cannot record human-item telemetry to {}: {error}",
            Path::new(&path).display()
        );
    }
}

fn fresh() -> Index {
    Index {
        version: INDEX_VERSION,
        ..Index::default()
    }
}

fn first_line(file: &mut std::fs::File) -> std::io::Result<Vec<u8>> {
    file.seek(SeekFrom::Start(0))?;
    let mut line = Vec::new();
    let mut buffer = [0u8; 4096];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            return Ok(line);
        }
        if let Some(end) = buffer[..read].iter().position(|b| *b == b'\n') {
            line.extend_from_slice(&buffer[..end]);
            return Ok(line);
        }
        line.extend_from_slice(&buffer[..read]);
    }
}

fn event_id(seq: u64) -> String {
    format!("{STREAM}:{seq}")
}

/// The item as the journal first recorded it with a tracker bound: the content
/// its projection was first written with. Read only when the projection record
/// that keeps that content is lost.
pub(crate) fn first_tracked(journal: &Path, id: &str) -> Option<HumanItem> {
    let text = std::fs::read(journal).ok()?;
    text.split(|b| *b == b'\n')
        .filter_map(|line| parse_record(line).ok())
        .map(|(_, payload)| payload.item)
        .find(|item| item.id == id && item.tracker.is_some())
}

/// One record of the host journal: its sequence and the item event it carries.
pub(crate) fn parse_record(line: &[u8]) -> Result<(u64, HumanItemEventPayload), String> {
    let envelope: Envelope = serde_json::from_slice(line).map_err(|e| e.to_string())?;
    if envelope.stream != STREAM {
        return Err(format!("it is on stream {:?}", envelope.stream));
    }
    if !crate::event::written_at_a_known_version(&envelope) {
        return Err(format!(
            "it is written at envelope version {}, which this build does not read",
            envelope.v
        ));
    }
    let payload: HumanItemEventPayload =
        serde_json::from_value(serde_json::Value::Object(envelope.payload))
            .map_err(|e| e.to_string())?;
    let kind = serde_json::to_value(payload.action)
        .ok()
        .and_then(|v| v.as_str().map(str::to_owned))
        .unwrap_or_default();
    if envelope.kind.0 != kind {
        return Err(format!(
            "its kind {} differs from its payload's action {kind}",
            envelope.kind.0
        ));
    }
    Ok((envelope.seq, payload))
}

/// A run journal's cache key: its length and a digest of its final bytes.
fn run_key(journal: &Path) -> std::io::Result<(u64, String)> {
    let mut file = std::fs::File::open(journal)?;
    let length = file.metadata()?.len();
    let from = length.saturating_sub(RUN_TAIL);
    file.seek(SeekFrom::Start(from))?;
    let mut tail = Vec::new();
    file.read_to_end(&mut tail)?;
    Ok((length, hex(&tail)))
}

/// A run's DAG off its checkpoint and what was appended past it.
fn derive_dag(paths: &crate::ledger::RunPaths, journal_len: u64, tail: String) -> Option<RunDag> {
    let launch: crate::ledger::LaunchRecord = crate::ledger::read_json(&paths.launch()).ok()?;
    let state = crate::checkpoint::current(paths);
    let statuses = state.statuses();
    let nodes = state
        .graph
        .iter()
        .map(|node| {
            let status = statuses
                .get(&node.id)
                .copied()
                .unwrap_or(crate::graph::NodeStatus::Pending);
            DagNode {
                id: node.id.clone(),
                title: node
                    .title
                    .clone()
                    .filter(|t| !t.trim().is_empty())
                    .or_else(|| node.human_item.as_ref().map(|m| m.title.clone()))
                    .unwrap_or_else(|| node.id.clone()),
                deps: node.deps.clone(),
                human: node.kind == crate::plan::NodeKind::Human,
                undispatched: matches!(
                    status,
                    crate::graph::NodeStatus::Pending
                        | crate::graph::NodeStatus::Ready
                        | crate::graph::NodeStatus::Waiting
                        | crate::graph::NodeStatus::Blocked
                ),
            }
        })
        .collect();
    Some(RunDag {
        journal_len,
        tail,
        plan: launch.project,
        nodes,
    })
}

/// The undispatched steps a work hold on `node` keeps from running: the node
/// itself unless it is the human step the item stands for, and everything that
/// depends on it, in plan order.
fn blocked_by_work<'a>(dag: &'a RunDag, node: &str) -> Vec<&'a DagNode> {
    let mut reached = BTreeSet::new();
    let mut frontier = vec![node.to_owned()];
    while let Some(at) = frontier.pop() {
        for dependent in dag.nodes.iter().filter(|n| n.deps.contains(&at)) {
            if reached.insert(dependent.id.clone()) {
                frontier.push(dependent.id.clone());
            }
        }
    }
    dag.nodes
        .iter()
        .filter(|n| n.undispatched)
        .filter(|n| reached.contains(&n.id) || (n.id == node && !n.human))
        .collect()
}

/// Every step an item holds, grouped by plan in plan order.
fn blocked_nodes(snapshot: &Snapshot, item: &HumanItem) -> Vec<(String, Vec<HumanItemNode>)> {
    if !item.is_blocking() {
        return Vec::new();
    }
    let mut groups: Vec<(String, Vec<HumanItemNode>)> = Vec::new();
    let mut push = |plan: &str, run: &str, node: &DagNode| {
        let entry = HumanItemNode {
            run: run.to_owned(),
            node: node.id.clone(),
            title: node.title.clone(),
        };
        match groups.iter_mut().find(|(p, _)| p == plan) {
            Some((_, nodes)) => nodes.push(entry),
            None => groups.push((plan.to_owned(), vec![entry])),
        }
    };
    match &item.block {
        HumanItemBlock::Nothing => {}
        HumanItemBlock::Work { run, node } => {
            if let Some(dag) = snapshot.runs.get(run) {
                for n in blocked_by_work(dag, node) {
                    push(&dag.plan, run, n);
                }
            }
        }
        HumanItemBlock::Plan { plan } => {
            for (run, dag) in snapshot.runs.iter().filter(|(_, d)| d.plan == *plan) {
                for n in dag.nodes.iter().filter(|n| n.undispatched) {
                    push(plan, run, n);
                }
            }
        }
        HumanItemBlock::Host => {
            for (run, dag) in &snapshot.runs {
                for n in dag.nodes.iter().filter(|n| n.undispatched) {
                    push(&dag.plan, run, n);
                }
            }
        }
    }
    groups
}

fn impact(groups: &[(String, Vec<HumanItemNode>)]) -> HumanItemBlocked {
    HumanItemBlocked {
        total: groups.iter().map(|(_, n)| n.len() as u64).sum(),
        plans: groups
            .iter()
            .map(|(plan, nodes)| HumanItemBlockedPlan {
                plan: plan.clone(),
                count: nodes.len() as u64,
                preview: nodes.iter().take(3).cloned().collect(),
            })
            .collect(),
    }
}

pub(crate) fn view(snapshot: &Snapshot, item: &HumanItem) -> HumanItemView {
    HumanItemView {
        item: item.clone(),
        blocked: impact(&blocked_nodes(snapshot, item)),
    }
}

pub(crate) fn detail(snapshot: &Snapshot, item: &HumanItem) -> HumanItemDetail {
    let groups = blocked_nodes(snapshot, item);
    HumanItemDetail {
        item: item.clone(),
        blocked: impact(&groups),
        nodes: groups.into_iter().flat_map(|(_, nodes)| nodes).collect(),
    }
}

fn rank(priority: Priority) -> u8 {
    match priority {
        Priority::Urgent => 0,
        Priority::High => 1,
        Priority::Medium => 2,
        Priority::Low => 3,
        Priority::None => 4,
    }
}

/// Whether one item is selected by a filter; state defaults to open.
pub(crate) fn selected(item: &HumanItem, filter: &HumanItemFilter) -> bool {
    filter.kind.is_none_or(|kind| item.kind == kind)
        && filter
            .blocking
            .is_none_or(|blocking| item.is_blocking() == blocking)
        && filter.priority.is_none_or(|p| item.priority == p)
        && item.state == filter.state.unwrap_or(HumanItemState::Open)
        && filter
            .session
            .as_ref()
            .is_none_or(|session| &item.owner.session == session)
        && filter
            .run
            .as_ref()
            .is_none_or(|run| item.owner.run.as_ref() == Some(run))
        && filter
            .plan
            .as_ref()
            .is_none_or(|plan| item.owner.plan.as_ref() == Some(plan))
        && filter
            .host
            .is_none_or(|host| (item.block == HumanItemBlock::Host) == host)
}

fn query(snapshot: &Snapshot, filter: &HumanItemFilter) -> Vec<HumanItemView> {
    let mut items: Vec<&HumanItem> = snapshot
        .items
        .values()
        .filter(|item| selected(item, filter))
        .collect();
    items.sort_by_key(|item| {
        (
            !item.is_blocking(),
            rank(item.priority),
            item.created_at.clone(),
            item.id.clone(),
        )
    });
    items.into_iter().map(|item| view(snapshot, item)).collect()
}

/// One serialized writer: the journal's append lock held from the read the
/// action is validated against to the synced append it makes.
struct Writer<'a> {
    store: &'a HumanItems,
    file: std::fs::File,
    snapshot: Snapshot,
    next_seq: u64,
    last: Option<String>,
    /// Whether an append this writer made went into the emergency reserve.
    reserve_used: bool,
}

impl<'a> Writer<'a> {
    fn open(store: &'a HumanItems) -> Result<Self, HumanItemRefusal> {
        let path = store.journal();
        std::fs::create_dir_all(store.dir()).map_err(|e| storage("create", &store.dir(), e))?;
        // The append lock first, so no other writer appends between the read
        // the action is judged against and the record it makes.
        let file = crate::ledger::open_healed_for_append(&path)
            .map_err(|e| storage("open for appending", &path, e))?;
        let (snapshot, telemetry) = store.read()?;
        store.report(&telemetry);
        // The reserve is allocated while there is room; its allocation is also
        // the one probe of a journal outage, so a full disk that has cleared is
        // resolved here — by the capability that failed, and nothing else.
        let reserved = super::durable::ensure_reserve(&path).is_ok();
        let next_seq = snapshot
            .events
            .values()
            .filter_map(|id| id.rsplit(':').next()?.parse::<u64>().ok())
            .max()
            .map_or(0, |seq| seq + 1);
        let mut writer = Self {
            store,
            file,
            snapshot,
            next_seq,
            last: None,
            reserve_used: false,
        };
        if reserved {
            writer.clear(
                JOURNAL_NO_SPACE,
                "the human-item journal's filesystem has space again",
            )?;
        }
        writer.probe_dispatch_space()?;
        writer.probe_identities()?;
        Ok(writer)
    }

    /// Probe an open identity outage by the one capability that failed: the
    /// harness's own zero-turn usage read of exactly the identities that were
    /// refused — never a dispatch, never a model turn — at most every ten seconds.
    /// It clears when every one of them reports headroom.
    fn probe_identities(&mut self) -> Result<(), HumanItemRefusal> {
        if self.open_condition(IDENTITIES_REFUSED).is_none() {
            return Ok(());
        }
        let mut known = probes(self.store);
        let Some(Probe::Identities {
            cwd,
            identities,
            probed_at,
        }) = known.get_mut(IDENTITIES_REFUSED.as_str())
        else {
            return Ok(());
        };
        let now = crate::sys::now_millis();
        if probed_at.is_some_and(|at| now.saturating_sub(at) < IDENTITY_PROBE_EVERY_MS) {
            return Ok(());
        }
        *probed_at = Some(now);
        let (identities, cwd) = (identities.clone(), cwd.clone());
        keep_probes(self.store, &known);
        if super::runtime::identities_have_headroom(&identities, cwd) {
            self.clear(
                IDENTITIES_REFUSED,
                &format!("{} report headroom again", identities.join(", ")),
            )?;
        }
        Ok(())
    }

    /// Probe each open dispatch outage by the one capability that failed: a file
    /// created and removed in the directory a dispatch could not write.
    fn probe_dispatch_space(&mut self) -> Result<(), HumanItemRefusal> {
        if self.open_condition(DISPATCH_NO_SPACE).is_none() {
            return Ok(());
        }
        let Some(Probe::Space { dir }) = probes(self.store).remove(DISPATCH_NO_SPACE.as_str())
        else {
            return Ok(());
        };
        let probe = dir.join(format!(".onepipeline-space-probe-{}", crate::sys::pid()));
        let written = std::fs::write(&probe, b"probe");
        let _ = std::fs::remove_file(&probe);
        if written.is_ok() {
            self.clear(
                DISPATCH_NO_SPACE,
                &format!("{} accepts writes again", dir.display()),
            )?;
        }
        Ok(())
    }

    /// Bring the index up to what this writer appended, best effort: the
    /// journal already holds it, so a failure here costs the next reader a
    /// tail read and nothing else.
    fn finish(self) {
        drop(self.file);
        if self.last.is_some() {
            let _ = self.store.read();
        }
        self.store.project_pending();
    }

    fn append(
        &mut self,
        item: &HumanItem,
        action: HumanItemEventAction,
    ) -> Result<String, HumanItemRefusal> {
        let payload = HumanItemEventPayload {
            schema_version: 1,
            item_id: item.id.clone(),
            owner: item.owner.clone(),
            action,
            item: item.clone(),
            resolution: item.resolution.clone(),
        };
        let serde_json::Value::Object(payload) = serde_json::to_value(&payload)
            .map_err(|e| conflict(format!("the item event cannot be serialized: {e}")))?
        else {
            return Err(conflict("the item event is not an object"));
        };
        let mut labels = crate::event::Labels::default();
        if let Some(run) = &item.owner.run {
            labels.run_id = Some(run.clone());
        }
        let kind = serde_json::to_value(action)
            .ok()
            .and_then(|v| v.as_str().map(str::to_owned))
            .unwrap_or_default();
        let envelope = Envelope {
            v: ENVELOPE_VERSION,
            ts: crate::sys::now_rfc3339(),
            stream: STREAM.to_owned(),
            seq: self.next_seq,
            source: Source::Pipeline,
            kind: kind.as_str().into(),
            dimensions: Default::default(),
            labels,
            payload,
            artifacts: Vec::new(),
        };
        let line = serde_json::to_string(&envelope)
            .map_err(|e| conflict(format!("the item event cannot be serialized: {e}")))?;
        let path = self.store.journal();
        let appended = super::durable::append(&path, &mut self.file, &line)?;
        let from_reserve = appended == super::durable::Appended::FromReserve;
        if from_reserve {
            self.reserve_used = true;
        }
        let id = event_id(self.next_seq);
        self.next_seq += 1;
        self.snapshot.items.insert(item.id.clone(), item.clone());
        self.snapshot.events.insert(item.id.clone(), id.clone());
        self.last = Some(id.clone());
        super::delivery::deliver(&self.store.root, &envelope);
        if from_reserve {
            // The record that tells the host its journal is full is written into
            // the space the reserve released for exactly this.
            self.condition(
                JOURNAL_NO_SPACE,
                &item.owner.session,
                "Free space for the human-item journal",
                &format!(
                    "The filesystem holding {} has no space left. Human items are being \
                     recorded into its emergency reserve, which runs out; free space on it. \
                     This resolves itself once the journal can allocate its reserve again.",
                    path.display()
                ),
            )?;
        }
        Ok(id)
    }

    /// The open item a host condition raised, if one is open.
    fn open_condition(&self, condition: HostCondition) -> Option<HumanItem> {
        let prefix = format!("{HOST_CONDITION_PREFIX}{}:", condition.as_str());
        self.snapshot
            .items
            .values()
            .find(|item| item.state == HumanItemState::Open && item.dedup_key.starts_with(&prefix))
            .cloned()
    }

    /// Raise one urgent host item for a condition, unless one is open for it.
    fn condition(
        &mut self,
        condition: HostCondition,
        session: &str,
        title: &str,
        body: &str,
    ) -> Result<HumanItemReceipt, HumanItemRefusal> {
        if let Some(open) = self.open_condition(condition) {
            let event = self
                .snapshot
                .events
                .get(&open.id)
                .cloned()
                .unwrap_or_default();
            return Ok(self.receipt(&open, event));
        }
        self.raise(HumanItemInput {
            schema_version: 1,
            dedup_key: format!(
                "{HOST_CONDITION_PREFIX}{}:{}",
                condition.as_str(),
                self.next_seq
            ),
            kind: HumanItemKind::Task,
            title: title.to_owned(),
            body: body.to_owned(),
            priority: Priority::Urgent,
            owner: super::HumanItemOwner {
                session: session.to_owned(),
                run: None,
                plan: None,
            },
            block: HumanItemBlock::Host,
            approval: None,
        })
    }

    /// Resolve the open item a condition raised, if any, as cleared.
    fn clear(&mut self, condition: HostCondition, reason: &str) -> Result<bool, HumanItemRefusal> {
        let Some(open) = self.open_condition(condition) else {
            return Ok(false);
        };
        self.resolve(
            &open,
            HumanItemState::Dismissed,
            HumanItemEventAction::AutoResolved,
            &Actor {
                actor: "engine".into(),
                on_behalf_of: None,
            },
            None,
            Some(reason.to_owned()),
        )?;
        Ok(true)
    }

    fn receipt(&self, item: &HumanItem, event_id: String) -> HumanItemReceipt {
        HumanItemReceipt {
            item: view(&self.snapshot, item),
            event_id,
        }
    }

    fn raise(&mut self, input: HumanItemInput) -> Result<HumanItemReceipt, HumanItemRefusal> {
        validate_input(&input)?;
        let id = item_id_for(&input.dedup_key);
        let now = crate::sys::now_rfc3339();
        if let Some(existing) = self.snapshot.items.get(&id).cloned() {
            if existing.kind != input.kind
                || existing.owner != input.owner
                || existing.block != input.block
            {
                return Err(conflict(format!(
                    "dedup_key {} already names item {} with another kind, owner or block",
                    input.dedup_key, existing.id
                )));
            }
            let unchanged = existing.title == input.title
                && existing.body == input.body
                && existing.priority == input.priority
                && existing.approval == input.approval;
            if unchanged || existing.state != HumanItemState::Open {
                let event = self.snapshot.events.get(&id).cloned().unwrap_or_default();
                return Ok(self.receipt(&existing, event));
            }
            // The same open item, raised again with what its phase knows now:
            // updated in place, never a second item.
            let mut updated = existing;
            updated.title = input.title;
            updated.body = input.body;
            updated.priority = input.priority;
            updated.approval = input.approval;
            updated.updated_at = now;
            let event = self.append(&updated, HumanItemEventAction::Raised)?;
            return Ok(self.receipt(&updated, event));
        }
        let item = HumanItem {
            schema_version: 1,
            id,
            dedup_key: input.dedup_key,
            kind: input.kind,
            title: input.title,
            body: input.body,
            priority: input.priority,
            owner: input.owner,
            block: input.block,
            approval: input.approval,
            state: HumanItemState::Open,
            resolution: None,
            tracker: None,
            created_at: now.clone(),
            updated_at: now,
        };
        let event = self.append(&item, HumanItemEventAction::Raised)?;
        Ok(self.receipt(&item, event))
    }

    fn resolve(
        &mut self,
        item: &HumanItem,
        state: HumanItemState,
        action: HumanItemEventAction,
        actor: &Actor,
        text: Option<String>,
        reason: Option<String>,
    ) -> Result<HumanItemReceipt, HumanItemRefusal> {
        let mut resolved = item.clone();
        resolved.state = state;
        resolved.resolution = Some(HumanItemResolution {
            actor: actor.actor.clone(),
            on_behalf_of: actor.on_behalf_of.clone(),
            text,
            reason,
        });
        resolved.updated_at = crate::sys::now_rfc3339();
        let event = self.append(&resolved, action)?;
        Ok(self.receipt(&resolved, event))
    }

    fn apply(
        &mut self,
        action: HumanItemAction,
        complete: &mut Approve<'_>,
    ) -> Result<HumanItemReceipt, HumanItemRefusal> {
        let (id, actor, state, text, reason) = match action {
            HumanItemAction::Raise(input) => return self.raise(input),
            HumanItemAction::Answer {
                id,
                text,
                actor,
                on_behalf_of,
            } => {
                if text.trim().is_empty() {
                    return Err(conflict("answer text must not be blank"));
                }
                (
                    id,
                    Actor {
                        actor,
                        on_behalf_of,
                    },
                    HumanItemState::Answered,
                    Some(text),
                    None,
                )
            }
            HumanItemAction::Complete {
                id,
                actor,
                on_behalf_of,
            } => (
                id,
                Actor {
                    actor,
                    on_behalf_of,
                },
                HumanItemState::Completed,
                None,
                None,
            ),
            HumanItemAction::Dismiss {
                id,
                reason,
                actor,
                on_behalf_of,
            } => (
                id,
                Actor {
                    actor,
                    on_behalf_of,
                },
                HumanItemState::Dismissed,
                None,
                reason
                    .map(|r| r.trim().to_owned())
                    .filter(|r| !r.is_empty()),
            ),
        };
        if actor.actor.trim().is_empty() {
            return Err(conflict("an action names its actor"));
        }
        let item = self
            .snapshot
            .items
            .get(&id)
            .cloned()
            .ok_or_else(|| conflict(format!("no such human item {id}")))?;
        if (state == HumanItemState::Answered && item.kind != HumanItemKind::Question)
            || (state == HumanItemState::Completed && item.kind != HumanItemKind::Task)
        {
            let verb = if state == HumanItemState::Answered {
                "answered; complete or dismiss it"
            } else {
                "completed; answer or dismiss it"
            };
            return Err(conflict(format!(
                "{} is a {}, which cannot be {verb}",
                item.id,
                match item.kind {
                    HumanItemKind::Task => "task",
                    HumanItemKind::Question => "question",
                }
            )));
        }
        if item.state != HumanItemState::Open {
            let held = item.resolution.as_ref();
            let identical =
                item.state == state && held.is_some_and(|r| r.text == text && r.reason == reason);
            if identical {
                let event = self.snapshot.events.get(&id).cloned().unwrap_or_default();
                return Ok(self.receipt(&item, event));
            }
            return Err(conflict(format!(
                "{} is already {}; a different resolution is refused",
                item.id,
                state_word(item.state)
            )));
        }
        if state == HumanItemState::Completed && item.approval.is_some() {
            if let Some(replacement) = complete(&item, &actor)? {
                // The replacement first: a process stopped between the two records
                // leaves the obsolete task open beside it, and completing that task
                // again finds the same changed version, re-raises the replacement as
                // the one item its key names, and dismisses the obsolete one.
                self.raise(replacement)?;
                self.resolve(
                    &item,
                    HumanItemState::Dismissed,
                    HumanItemEventAction::Dismissed,
                    &actor,
                    None,
                    Some(super::approval::REREAD.to_owned()),
                )?;
                return Err(conflict(super::approval::REREAD));
            }
        }
        let action = match state {
            HumanItemState::Answered => HumanItemEventAction::Answered,
            HumanItemState::Completed => HumanItemEventAction::Completed,
            _ => HumanItemEventAction::Dismissed,
        };
        self.resolve(&item, state, action, &actor, text, reason)
    }
}

pub(crate) fn state_word(state: HumanItemState) -> &'static str {
    match state {
        HumanItemState::Open => "open",
        HumanItemState::Answered => "answered",
        HumanItemState::Completed => "completed",
        HumanItemState::Dismissed => "dismissed",
    }
}

fn validate_input(input: &HumanItemInput) -> Result<(), HumanItemRefusal> {
    input.validate().map_err(conflict)?;
    if input.dedup_key.trim().is_empty() {
        return Err(conflict("dedup_key must not be blank"));
    }
    if input.owner.session.trim().is_empty() {
        return Err(conflict("owner.session must not be blank"));
    }
    match &input.block {
        HumanItemBlock::Work { run, node } if run.trim().is_empty() || node.trim().is_empty() => {
            Err(conflict("a work block names its run and node"))
        }
        HumanItemBlock::Plan { plan } if plan.trim().is_empty() => {
            Err(conflict("a plan block names its plan"))
        }
        _ => Ok(()),
    }
}
