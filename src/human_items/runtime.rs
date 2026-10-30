//! Human items inside a run: the item every `kind: human` node gets when it
//! becomes eligible, the dispatch an open block holds, and what a resolution
//! does to the node it stands for.
//!
//! **Only open blocks hold dispatch, and only dispatch.** A work block holds the
//! node it names and everything downstream of it; a plan block every node of
//! that plan; a host block every node on the host. Work already running is never
//! stopped. Overlapping holds compose: a node is held while any one holds it.
//!
//! **A resolution acts on its node through the run's own commands**, exactly as
//! a person would: an answer or a completion is one `attest`, and a dismissal is
//! one `drop` whose dependents are detached, carrying the reason when one was
//! given — never a skip and never an attestation.

use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, Instant};

use super::live::{item_id_for, HumanItems};
use super::{
    derive_legacy, HumanItem, HumanItemBlock, HumanItemInput, HumanItemOwner, HumanItemState,
};
use crate::graph::NodeStatus;
use crate::ledger::{LaunchRecord, RunPaths};
use crate::plan::NodeKind;
use crate::projection::RunState;

/// The dedup key of the item a run's human node gets: its run/node identity.
pub(crate) fn node_key(run: &str, node: &str) -> String {
    format!("run:{run}#{node}")
}

/// The run and node a human node's item stands for: an item under the engine's
/// `run:` key whose work block names that same run and node, the run a valid id.
/// Anything else — a key no engine minted — stands for no step.
pub(crate) fn node_of(item: &HumanItem) -> Option<(&str, &str)> {
    let (run, node) = item.dedup_key.strip_prefix("run:")?.split_once('#')?;
    let bound =
        matches!(&item.block, HumanItemBlock::Work { run: r, node: n } if r == run && n == node);
    (bound && crate::ledger::is_valid_run_id(run) && item.owner.run.as_deref() == Some(run))
        .then_some((run, node))
}

/// The store and session of the run this process drives, for the dispatch
/// threads that report a host condition and know no launch.
static DRIVING: std::sync::Mutex<Option<(HumanItems, String)>> = std::sync::Mutex::new(None);

/// A dispatch the host refused before any work began: one refused for want of
/// space raises the host's one dispatch-space item, owned by the driving
/// session, probed afterwards by the directory that could not be written.
/// Anything else — and any refusal not definitively a full filesystem — raises
/// nothing.
pub(crate) fn dispatch_refused(error: &crate::error::Error) {
    let crate::error::Error::Ledger { path, source } = error else {
        return;
    };
    if !super::durable::is_no_space(source) {
        return;
    }
    let driving = DRIVING
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone();
    let Some((store, session)) = driving else {
        return;
    };
    let probe = super::live::Probe::Space {
        dir: path.parent().unwrap_or(path).to_path_buf(),
    };
    if let Err(why) = store.raise_condition(
        super::live::DISPATCH_NO_SPACE,
        &session,
        "Free space where dispatches start",
        &format!(
            "A dispatch could not start: {} has no space left ({source}). No work starts on \
             this host until it does; this resolves itself once that directory accepts \
             writes again.",
            path.display()
        ),
        probe,
    ) {
        eprintln!(
            "onepipeline: a dispatch found no space, and its host item was not raised: {why}"
        );
    }
}

/// The failure kinds that say an identity cannot serve any turn until a person
/// acts: out of quota, refused its credentials, or the provider unavailable.
const DEFINITIVE: [oneharness_core::domain::signals::FailureKind; 3] = [
    oneharness_core::domain::signals::FailureKind::Quota,
    oneharness_core::domain::signals::FailureKind::Auth,
    oneharness_core::domain::signals::FailureKind::ServerOverloaded,
];

/// Whether a candidate's failure kind, in the producer's wire word, is one of
/// [`DEFINITIVE`]: read through the harness's own vocabulary, so a word it does
/// not define is never definitive.
fn definitive(kind: &str) -> bool {
    serde_json::from_value::<oneharness_core::domain::signals::FailureKind>(
        serde_json::Value::String(kind.to_owned()),
    )
    .is_ok_and(|kind| DEFINITIVE.contains(&kind))
}

/// An exhausted chain whose every candidate was refused for a definitive reason
/// raises the host's one identity item; one candidate refused, a mix that
/// includes a transient or unclassified failure, or a chain that stopped at one
/// candidate raises nothing.
pub(crate) fn chain_exhausted(
    store: &HumanItems,
    session: &str,
    cwd: &std::path::Path,
    stopped: &crate::engine::ChainStopped,
) {
    let attempted = &stopped.attempted;
    if attempted.len() < 2
        || !attempted
            .iter()
            .all(|c| c.failure_kind.as_deref().is_some_and(definitive))
    {
        return;
    }
    let identities: Vec<&str> = attempted.iter().map(|c| c.identity.as_str()).collect();
    let refusals = attempted
        .iter()
        .map(|c| {
            format!(
                "- {} [{}]: {}",
                c.identity,
                c.failure_kind.as_deref().unwrap_or_default(),
                c.detail
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    if let Err(why) = store.raise_condition(
        super::live::IDENTITIES_REFUSED,
        session,
        "Restore an identity dispatches can run as",
        &format!(
            "Every identity of member '{}' was refused, so no dispatch on this host can run:\n\
             {refusals}\nRestore one of them; this resolves itself once they report headroom.",
            stopped.member
        ),
        super::live::Probe::Identities {
            cwd: Some(cwd.to_path_buf()),
            identities: identities.iter().map(|i| (*i).to_owned()).collect(),
            probed_at: None,
        },
    ) {
        eprintln!(
            "onepipeline: every identity was refused, and its host item was not raised: {why}"
        );
    }
}

/// Whether every one of `identities` reports headroom, read by the harness's own
/// zero-turn usage probe — what `oneharness usage` answers, in process.
pub(crate) fn identities_have_headroom(
    identities: &[String],
    cwd: Option<std::path::PathBuf>,
) -> bool {
    if identities.is_empty() {
        return false;
    }
    let request = oneharness_core::io::usage::UsageRequest {
        harness: identities.to_vec(),
        cwd,
        timeout: Some(Duration::from_secs(20)),
        ..Default::default()
    };
    let Ok(report) = oneharness_core::io::usage::report(&request) else {
        return false;
    };
    identities.iter().all(|wanted| {
        let (harness, variant) = match wanted.split_once(':') {
            Some((h, v)) => (h, Some(v)),
            None => (wanted.as_str(), None),
        };
        report.identities.iter().any(|seen| {
            seen.harness == harness
                && seen.variant.as_ref().map(ToString::to_string).as_deref() == variant
                && matches!(
                    seen.availability,
                    oneharness_core::domain::usage::UsageAvailability::Available { .. }
                )
        })
    })
}

/// How often a driver looks at the human-item store.
const EVERY: Duration = Duration::from_millis(250);
/// How long a resolution is left to the process that recorded it before the
/// driver acts on its node itself.
const GRACE: Duration = Duration::from_secs(10);

/// A run driver's view of the human items that concern its run.
// llmlint: ignore-block[invalid_states_unrepresentable] private copies of the run's validated `RunPaths` id, its launch record's session and project, and the graph's own node ids, written once in `for_launch` or read off the graph and only compared; every item they build is validated as every raise is.
pub(crate) struct Gate {
    store: HumanItems,
    run: String,
    session: String,
    plan: String,
    raised: BTreeSet<String>,
    released: BTreeSet<String>,
    held: BTreeSet<String>,
    looked: Option<Instant>,
    /// The host journal's length when the last pass read it.
    seen: u64,
    /// When the earliest release this driver is giving its resolver the chance
    /// to make falls due, in epoch milliseconds.
    due: Option<u64>,
} // llmlint: ignore-end[invalid_states_unrepresentable]

impl Gate {
    /// The gate for a launch whose effective configuration enables human items.
    pub(crate) fn for_launch(paths: &RunPaths, launch: &LaunchRecord) -> Option<Self> {
        let store = store_for(paths, launch)?;
        *DRIVING
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) =
            Some((store.clone(), launch.session.clone()));
        Some(Self {
            store,
            run: paths.run.clone(),
            session: launch.session.clone(),
            plan: launch.project.clone(),
            raised: BTreeSet::new(),
            released: BTreeSet::new(),
            held: BTreeSet::new(),
            looked: None,
            seen: 0,
            due: None,
        })
    }

    /// Raise the item of every eligible human node that has none, and answer
    /// every node an open block holds.
    pub(crate) fn pass(
        &mut self,
        paths: &RunPaths,
        state: &RunState,
        statuses: &BTreeMap<String, NodeStatus>,
    ) -> BTreeSet<String> {
        if self.looked.is_some_and(|at| at.elapsed() < EVERY) && !self.moved() {
            return self.held.clone();
        }
        self.looked = Some(std::time::Instant::now());
        self.seen = self.length();
        for node in state.graph.iter().filter(|n| n.kind == NodeKind::Human) {
            if statuses.get(&node.id) != Some(&NodeStatus::Waiting)
                || self.raised.contains(&node.id)
            {
                continue;
            }
            match self.raise(node) {
                Ok(()) => {
                    self.raised.insert(node.id.clone());
                }
                Err(why) => eprintln!(
                    "onepipeline: run '{}' could not raise the human item for '{}': {why}",
                    self.run, node.id
                ),
            }
        }
        let items = match self.store.items() {
            Ok(items) => items,
            Err(why) => {
                eprintln!(
                    "onepipeline: run '{}' could not read its human items, so the holds it last \
                     read stand: {why}",
                    self.run
                );
                return self.held.clone();
            }
        };
        if items.values().any(|i| {
            i.state == HumanItemState::Open
                && i.dedup_key.starts_with(super::live::HOST_CONDITION_PREFIX)
        }) {
            self.store.probe();
        }
        self.due = None;
        self.reconcile(paths, state, statuses, &items);
        self.held = held(&self.run, &self.plan, state, statuses, &items);
        self.held.clone()
    }

    fn raise(&self, node: &crate::plan::Node) -> Result<(), String> {
        let metadata = node.human_item.clone().unwrap_or_else(|| {
            let record = node.task_record.as_ref();
            derive_legacy(
                &node.id,
                record.map_or("", |r| r.title.as_str()),
                node.task.as_deref().unwrap_or(""),
                record.and_then(|r| r.priority),
            )
        });
        // A human node states its action prose (plan loading refuses one that does not),
        // and that prose is the item's body.
        let body = metadata.body;
        self.store
            .raise_engine(HumanItemInput {
                schema_version: 1,
                dedup_key: node_key(&self.run, &node.id),
                kind: metadata.kind,
                title: metadata.title,
                body,
                priority: metadata.priority,
                owner: HumanItemOwner {
                    session: self.session.clone(),
                    run: Some(self.run.clone()),
                    plan: Some(self.plan.clone()),
                },
                block: HumanItemBlock::Work {
                    run: self.run.clone(),
                    node: node.id.clone(),
                },
                approval: None,
            })
            .map(|_| ())
            .map_err(|e| e.to_string())
    }

    /// Act on this run's human nodes as their items stand: a resolution whose
    /// node is still waiting is applied once its recorder has had its chance,
    /// and an open item whose node moved on without it is resolved as such.
    fn reconcile(
        &mut self,
        paths: &RunPaths,
        state: &RunState,
        statuses: &BTreeMap<String, NodeStatus>,
        items: &BTreeMap<String, HumanItem>,
    ) {
        for node in state.graph.iter().filter(|n| n.kind == NodeKind::Human) {
            let Some(item) = items.get(&item_id_for(&node_key(&self.run, &node.id))) else {
                continue;
            };
            let waiting = statuses.get(&node.id) == Some(&NodeStatus::Waiting);
            match item.state {
                // The step is done without its item — attested another way — so the
                // item stands for nothing: whichever driver sees that resolves it.
                HumanItemState::Open if statuses.get(&node.id) == Some(&NodeStatus::Done) => {
                    let _ = self
                        .store
                        .auto_resolve_item(&item.id, "the step was settled outside its human item");
                }
                HumanItemState::Open => {}
                _ if waiting && !self.released.contains(&node.id) => {
                    let recorded = crate::sys::now_millis();
                    let at = crate::projection::millis_of(&item.updated_at).unwrap_or(recorded);
                    let due = at.saturating_add(GRACE.as_millis() as u64);
                    if recorded >= due {
                        self.queue(&crate::channel::ChannelState::new(paths), item);
                    } else {
                        self.due = Some(self.due.map_or(due, |earlier| earlier.min(due)));
                    }
                }
                _ => {}
            }
        }
    }

    /// Queue the release of every human step whose item was resolved and whose
    /// step still waits, onto the run's own command queue, which this driver's
    /// first pass applies: a resolver that could not release it — or died first —
    /// leaves it here. Called with the run held, before the loop starts.
    pub(crate) fn queue_owed(
        &mut self,
        channel: &crate::channel::ChannelState,
        state: &RunState,
        statuses: &BTreeMap<String, NodeStatus>,
    ) {
        let Ok(items) = self.store.items() else {
            return;
        };
        for node in state.graph.iter().filter(|n| n.kind == NodeKind::Human) {
            if statuses.get(&node.id) != Some(&NodeStatus::Waiting) {
                continue;
            }
            let Some(item) = items.get(&item_id_for(&node_key(&self.run, &node.id))) else {
                continue;
            };
            self.queue(channel, item);
        }
    }

    fn queue(&mut self, channel: &crate::channel::ChannelState, item: &HumanItem) {
        let Some(command) = release_command(item) else {
            return;
        };
        let Some((_, node)) = node_of(item) else {
            return;
        };
        if !self.released.insert(node.to_owned()) {
            return;
        }
        if let Err(why) = channel.submit(crate::channel::Author::planner(), &[command]) {
            // llmlint: ignore[changed_behavior_has_e2e] the run's own queue refusing an append is the store refusing a write under a driver that holds the run, which no portable journey arranges; it is said, and the next driver queues it again because its `released` set starts empty.
            self.released.remove(node);
            eprintln!(
                "onepipeline: run '{}' could not queue the release of '{node}' its item {} \
                 records: {why}; its next driver queues it again",
                self.run, item.id
            );
        }
    }

    fn length(&self) -> u64 {
        std::fs::metadata(self.store.journal()).map_or(0, |m| m.len())
    }

    /// Whether the host journal moved since the last pass read it: one `stat`,
    /// which is what the loop's idle wait can afford to ask.
    pub(crate) fn moved(&self) -> bool {
        self.length() != self.seen || self.due.is_some_and(|due| crate::sys::now_millis() >= due)
    }

    /// An exhausted identity chain one of this run's dispatches met.
    pub(crate) fn chain_exhausted(
        &self,
        cwd: &std::path::Path,
        stopped: &crate::engine::ChainStopped,
    ) {
        chain_exhausted(&self.store, &self.session, cwd, stopped);
    }

    /// Retire every open item holding this run's work, because the run ended.
    pub(crate) fn run_ended(&self) {
        ended(&self.store, &self.run);
    }
}

/// The store a run acts on, where its launch directory's effective
/// configuration enables human items.
fn store_for(paths: &RunPaths, launch: &LaunchRecord) -> Option<HumanItems> {
    let root = paths.dir.parent()?.to_path_buf();
    let dir = if launch.dir.as_os_str().is_empty() {
        std::env::current_dir().ok()?
    } else {
        launch.dir.clone()
    };
    match super::config::effective(&dir) {
        Ok(config) if config.human_items.enabled => Some(HumanItems::at(root, config).in_dir(dir)),
        Ok(_) => None,
        Err(why) => {
            eprintln!(
                "onepipeline: run '{}' reads no human items, because its configuration is \
                 malformed: {why}",
                paths.run
            );
            None
        }
    }
}

fn ended(store: &HumanItems, run: &str) {
    if let Err(why) = store.run_ended(run) {
        eprintln!(
            "onepipeline: run '{run}' ended, and the human items holding its work could not be \
             dismissed: {why}"
        );
    }
}

/// Retire the open items holding a run's work because the run ended, where its
/// configuration enables human items. For the paths that end a run without a
/// driver's gate: a stop.
pub(crate) fn run_ended(paths: &RunPaths) {
    let Ok(launch) = crate::ledger::read_json::<LaunchRecord>(&paths.launch()) else {
        return;
    };
    if let Some(store) = store_for(paths, &launch) {
        ended(&store, &paths.run);
    }
}

/// Every node of this run an open block holds.
fn held(
    run: &str,
    plan: &str,
    state: &RunState,
    statuses: &BTreeMap<String, NodeStatus>,
    items: &BTreeMap<String, HumanItem>,
) -> BTreeSet<String> {
    let mut held = BTreeSet::new();
    for item in items.values().filter(|i| i.is_blocking()) {
        match &item.block {
            HumanItemBlock::Host => held.extend(state.graph.ids().cloned()),
            HumanItemBlock::Plan { plan: p } if p == plan => {
                held.extend(state.graph.ids().cloned());
            }
            // A work hold binds a step that has not settled; once its step is
            // done it holds nothing, whatever the item says.
            HumanItemBlock::Work { run: r, node }
                if r == run && statuses.get(node) != Some(&NodeStatus::Done) =>
            {
                if state
                    .graph
                    .get(node)
                    .is_some_and(|n| n.kind != NodeKind::Human)
                {
                    held.insert(node.clone());
                }
                let mut frontier = vec![node.clone()];
                while let Some(at) = frontier.pop() {
                    for dependent in state.graph.dependents_of(&at) {
                        if held.insert(dependent.clone()) {
                            frontier.push(dependent);
                        }
                    }
                }
            }
            _ => {}
        }
    }
    held
}

/// Apply a resolved human node item to its node through the run's own
/// commands, as the person who resolved it would.
pub(crate) fn release(root: &std::path::Path, item: &HumanItem) -> Result<(), String> {
    let Some((run, _)) = node_of(item) else {
        return Ok(());
    };
    if item.state == HumanItemState::Open {
        return Ok(());
    }
    release_at(&RunPaths::under(root, run), item)
}

/// The run command that releases a resolved item's step, if it releases one: an
/// `attest` for an answer or completion, a `drop` detaching its dependents for a
/// person's dismissal. A dismissal the engine made — the run ended, or the step
/// settled without its item — releases nothing.
fn release_command(item: &HumanItem) -> Option<crate::channel::Command> {
    let (_, node) = node_of(item)?;
    let resolution = item.resolution.as_ref();
    match item.state {
        HumanItemState::Answered | HumanItemState::Completed => {
            Some(crate::channel::Command::Attest {
                reference: node.to_owned(),
            })
        }
        HumanItemState::Dismissed if resolution.is_some_and(|r| r.actor != "engine") => {
            Some(crate::channel::Command::Drop {
                id: node.to_owned(),
                dependents: crate::channel::Dependents::Detach,
                reason: resolution.and_then(|r| r.reason.clone()),
            })
        }
        _ => None,
    }
}

fn release_at(paths: &RunPaths, item: &HumanItem) -> Result<(), String> {
    let (Some((run, node)), Some(command)) = (node_of(item), release_command(item)) else {
        return Ok(());
    };
    let envelope = crate::channel::Reply {
        version: Some(crate::channel::REPLY_ENVELOPE_VERSION),
        commands: vec![command],
        ..crate::channel::Reply::default()
    };
    let text = serde_json::to_string(&envelope).map_err(|e| e.to_string())?;
    crate::verbs::reply(paths, None, &text)
        .map(|_| ())
        .map_err(|why| {
            format!(
            "the human item {} is {}, and run '{run}''s step '{node}' was not released: {why}; \
             the run's driver releases it when it next runs",
            item.id,
            super::live::state_word(item.state)
        )
        })
}
