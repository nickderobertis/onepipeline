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
    derive_legacy, HumanItem, HumanItemAction, HumanItemBlock, HumanItemInput, HumanItemOwner,
    HumanItemState,
};
use crate::graph::NodeStatus;
use crate::ledger::{LaunchRecord, RunPaths};
use crate::plan::NodeKind;
use crate::projection::RunState;

/// The dedup key of the item a run's human node gets: its run/node identity.
pub(crate) fn node_key(run: &str, node: &str) -> String {
    format!("run:{run}#{node}")
}

/// The run and node a human node's item stands for.
pub(crate) fn node_of(item: &HumanItem) -> Option<(&str, &str)> {
    item.dedup_key.strip_prefix("run:")?.split_once('#')
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
    let probe = path.parent().unwrap_or(path);
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

/// How often a driver looks at the human-item store.
const EVERY: Duration = Duration::from_millis(250);
/// How long a resolution is left to the process that recorded it before the
/// driver acts on its node itself.
const GRACE: Duration = Duration::from_secs(10);

/// A run driver's view of the human items that concern its run.
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
}

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
        self.reconcile(paths, state, statuses, &items);
        self.held = held(&self.run, &self.plan, state, &items);
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
        let body = if metadata.body.trim().is_empty() {
            format!("Complete the human step {}.", node.id)
        } else {
            metadata.body
        };
        self.store
            .apply(HumanItemAction::Raise(HumanItemInput {
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
            }))
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
                HumanItemState::Open if !waiting && self.raised.contains(&node.id) => {
                    let _ = self
                        .store
                        .auto_resolve_item(&item.id, "the step was settled outside its human item");
                }
                HumanItemState::Open => {}
                _ if waiting && !self.released.contains(&node.id) => {
                    let recorded = crate::sys::now_millis();
                    let at = parse_millis(&item.updated_at).unwrap_or(recorded);
                    if recorded.saturating_sub(at) >= GRACE.as_millis() as u64 {
                        self.released.insert(node.id.clone());
                        let item = item.clone();
                        let paths = paths.clone();
                        std::thread::spawn(move || {
                            if let Err(why) = release_at(&paths, &item) {
                                eprintln!("onepipeline: {why}");
                            }
                        });
                    }
                }
                _ => {}
            }
        }
    }

    fn length(&self) -> u64 {
        std::fs::metadata(self.store.journal()).map_or(0, |m| m.len())
    }

    /// Whether the host journal moved since the last pass read it: one `stat`,
    /// which is what the loop's idle wait can afford to ask.
    pub(crate) fn moved(&self) -> bool {
        self.length() != self.seen
    }

    /// Retire every open item holding this run's work, because the run ended.
    pub(crate) fn run_ended(&self) {
        ended(&self.store, &self.run);
    }
}

fn parse_millis(stamp: &str) -> Option<u64> {
    let parsed = time_from_rfc3339(stamp)?;
    Some(parsed)
}

/// Milliseconds since the epoch of an RFC 3339 UTC stamp this crate wrote.
fn time_from_rfc3339(stamp: &str) -> Option<u64> {
    // `YYYY-MM-DDTHH:MM:SS.mmmZ`, the one shape `sys::now_rfc3339` writes.
    let bytes = stamp.as_bytes();
    if bytes.len() < 20 {
        return None;
    }
    let n = |range: std::ops::Range<usize>| stamp.get(range)?.parse::<i64>().ok();
    let (year, month, day) = (n(0..4)?, n(5..7)?, n(8..10)?);
    let (hour, minute, second) = (n(11..13)?, n(14..16)?, n(17..19)?);
    let millis = if bytes.get(19) == Some(&b'.') {
        n(20..23).unwrap_or(0)
    } else {
        0
    };
    // Days from the civil date (Howard Hinnant's algorithm).
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (month + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    let seconds = days * 86_400 + hour * 3_600 + minute * 60 + second;
    u64::try_from(seconds * 1000 + millis).ok()
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
        Ok(config) if config.human_items.enabled => Some(HumanItems::at(root, config)),
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
    items: &BTreeMap<String, HumanItem>,
) -> BTreeSet<String> {
    let mut held = BTreeSet::new();
    for item in items.values().filter(|i| i.is_blocking()) {
        match &item.block {
            HumanItemBlock::Host => held.extend(state.graph.ids().cloned()),
            HumanItemBlock::Plan { plan: p } if p == plan => {
                held.extend(state.graph.ids().cloned());
            }
            HumanItemBlock::Work { run: r, node } if r == run => {
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
pub(crate) fn release(item: &HumanItem) -> Result<(), String> {
    let Some((run, _)) = node_of(item) else {
        return Ok(());
    };
    let paths = RunPaths::new(run);
    release_at(&paths, item)
}

fn release_at(paths: &RunPaths, item: &HumanItem) -> Result<(), String> {
    let Some((run, node)) = node_of(item) else {
        return Ok(());
    };
    let resolution = item.resolution.as_ref();
    let receipt = match item.state {
        HumanItemState::Answered | HumanItemState::Completed => crate::verbs::attest(paths, node),
        HumanItemState::Dismissed => {
            let reason = resolution.and_then(|r| r.reason.clone());
            // A run that ended retired its own holds; there is nothing left to drop.
            if reason.as_deref() == Some(super::RUN_ENDED)
                || resolution.is_some_and(|r| r.actor == "engine")
            {
                return Ok(());
            }
            let envelope = crate::channel::Reply {
                version: Some(crate::channel::REPLY_ENVELOPE_VERSION),
                commands: vec![crate::channel::Command::Drop {
                    id: node.to_owned(),
                    dependents: crate::channel::Dependents::Detach,
                    reason,
                }],
                ..crate::channel::Reply::default()
            };
            let text = serde_json::to_string(&envelope).map_err(|e| e.to_string())?;
            crate::verbs::reply(paths, None, &text)
        }
        HumanItemState::Open => return Ok(()),
    };
    receipt.map(|_| ()).map_err(|why| {
        format!(
            "the human item {} is {}, and run '{run}''s step '{node}' was not released: {why}; \
             the run's driver releases it when it next runs",
            item.id,
            super::live::state_word(item.state)
        )
    })
}
