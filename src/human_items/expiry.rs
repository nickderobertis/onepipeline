//! Expiry: the conditions an asker declares under which an open item stops mattering,
//! and the one sweep that retires an item once one of them holds.
//!
//! **Time, run and node conditions are read in process**, off the snapshot a read
//! already holds and the run states its index already folds; nothing is spawned for
//! them. **A check is a program**, run as argv with no shell, and is the only
//! condition that costs a process: each is claimed in a private record beside the
//! journal before it runs, under the store's index lock, so however many readers sweep
//! at once a check runs at most once per interval on the host. Its runs and outcomes
//! live in that record and never in the journal: only an expiry is journalled.
//!
//! **Only a well-formed answer expires anything.** A check reports by exiting 0 and
//! printing one JSON object with a boolean `expired`; every other ending — any other
//! exit, a signal, a program that cannot start, a lost working directory, a timeout,
//! output that is not that object — is a failure, recorded on the item, which stays
//! open.

use std::collections::{BTreeMap, BTreeSet};
use std::io::Read;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::live::{HumanItems, RunDag};
use super::{
    HumanItem, HumanItemCheckOutcome, HumanItemCheckStatus, HumanItemExpiryCondition as Condition,
    HumanItemExpirySweep, HumanItemRunEnding, HumanItemState,
};

/// The most of a check's stdout the engine reads; more is a malformed answer.
const STDOUT_LIMIT: usize = 64 * 1024;
/// How long past its timeout a claim is still taken as live: the claimant's own stop and
/// record. Older, its claimant is taken to have died and the check is claimed again.
const CLAIM_GRACE: Duration = Duration::from_secs(5);
/// How long a check's output is waited for once it has ended, for a process it left
/// behind holding its pipes.
const OUTPUT_GRACE: Duration = Duration::from_secs(1);
/// How often a running check is looked at.
const POLL: Duration = Duration::from_millis(20);
/// The check record's own format version; any other is read as empty.
const RECORD_VERSION: u32 = 1;
/// What a check that answers anything but the protocol is failed with.
const MALFORMED: &str = "its output was not one JSON object with a boolean \"expired\"";

/// One condition found holding, and the reason its expiry records.
pub(super) struct Held {
    pub(super) id: String,
    pub(super) condition: Condition,
    pub(super) reason: String,
}

/// What one sweep did: the summary a caller is answered with, the items it expired as
/// the journal now holds them, and how many conditions it read.
pub(super) struct Swept {
    pub(super) sweep: HumanItemExpirySweep,
    pub(super) expired: Vec<HumanItem>,
    pub(super) evaluated: u64,
}

/// How a node settled, once it has a settlement of any outcome.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(super) enum Settlement {
    Done,
    Failed,
    Skipped,
}

impl Settlement {
    /// The node's settlement, where it has one.
    pub(super) fn of(status: crate::graph::NodeStatus) -> Option<Self> {
        use crate::graph::NodeStatus as S;
        match status {
            S::Done => Some(Self::Done),
            S::Failed => Some(Self::Failed),
            S::Skipped => Some(Self::Skipped),
            _ => None,
        }
    }
    fn word(self) -> &'static str {
        match self {
            Self::Done => "done",
            Self::Failed => "failed",
            Self::Skipped => "skipped",
        }
    }
}

/// How a run ended, judged as the views judge an ending and as a run's end retires its
/// holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(super) enum Ending {
    Settled,
    EndedFailed,
    EndedUnfinished,
    EndedStopped,
}

impl Ending {
    /// How the run has ended, or `None` while it has not.
    pub(super) fn of(state: &crate::projection::RunState) -> Option<Self> {
        use crate::graph::{GraphState, NodeStatus};
        let statuses = state.statuses();
        if state.stop_recorded() {
            return Some(Self::EndedStopped);
        }
        if statuses.is_empty()
            || !crate::graph::is_terminal(&statuses)
            || crate::graph::state_of(&statuses) == GraphState::Waiting
        {
            return None;
        }
        Some(if statuses.values().all(|s| *s == NodeStatus::Done) {
            Self::Settled
        } else if statuses
            .values()
            .any(|s| matches!(s, NodeStatus::Failed | NodeStatus::Skipped))
        {
            Self::EndedFailed
        } else {
            // llmlint: ignore[changed_behavior_has_e2e] the same terminal, not-waiting reading the stopped, failed and settled endings take, which the run journeys drive; staging an unfinished ending needs a planner park over an otherwise settled run, a journey that would prove `graph::state_of` rather than this word.
            Self::EndedUnfinished
        })
    }
    /// The engine's run-state words for it.
    fn word(self) -> &'static str {
        match self {
            Self::Settled => "settled",
            Self::EndedFailed => "ended failed",
            Self::EndedUnfinished => "ended unfinished",
            Self::EndedStopped => "ended stopped",
        }
    }
}

/// Every run a time-free condition of an open item names: the runs a sweep must read.
pub(super) fn referenced_runs<'a>(items: impl Iterator<Item = &'a HumanItem>) -> BTreeSet<String> {
    items
        .filter(|item| item.state == HumanItemState::Open)
        .flat_map(|item| &item.expiry)
        .filter_map(|condition| match condition {
            Condition::Run { run, .. } | Condition::Node { run, .. } => Some(run.clone()),
            _ => None,
        })
        .collect()
}

/// Whether `condition` holds now, read in process, and the engine's own words for it.
/// A check is never decided here.
fn holds(condition: &Condition, runs: &BTreeMap<String, RunDag>, now: u64) -> Option<String> {
    match condition {
        Condition::Time { at: Some(at), .. } => super::rfc3339_millis(at)
            .is_some_and(|due| now >= due)
            .then(|| format!("its expiry time {at} was reached")),
        Condition::Run { run, on, .. } => {
            let dag = runs.get(run)?;
            match on {
                HumanItemRunEnding::Settled => (!dag.nodes.is_empty()
                    && dag
                        .nodes
                        .iter()
                        .all(|n| n.settled == Some(Settlement::Done)))
                .then(|| format!("run {run} settled")),
                HumanItemRunEnding::Ended => Some(format!("run {run} {}", dag.ending?.word())),
            }
        }
        Condition::Node { run, node, .. } => {
            let outcome = runs
                .get(run)?
                .nodes
                .iter()
                .find(|n| n.id == *node)?
                .settled?;
            Some(format!(
                "node {node} of run {run} settled {}",
                outcome.word()
            ))
        }
        Condition::Time { at: None, .. } | Condition::Check { .. } => None,
    }
}

/// The reason an expiry by `condition` records: a check's printed words, else the
/// condition's own `reason`, else the engine's.
fn reason_for(condition: &Condition, printed: Option<String>, engine: String) -> String {
    printed
        .or_else(|| condition.reason().map(str::to_owned))
        .unwrap_or(engine)
}

/// One check of one item a sweep may run.
#[derive(Clone)]
struct Candidate {
    id: String,
    index: usize,
    condition: Condition,
}

impl Candidate {
    fn key(&self) -> String {
        key(&self.id, self.index)
    }
    fn timeout(&self) -> u32 {
        match &self.condition {
            Condition::Check {
                timeout_seconds, ..
            } => timeout_seconds.unwrap_or(super::DEFAULT_CHECK_TIMEOUT_SECONDS),
            _ => super::DEFAULT_CHECK_TIMEOUT_SECONDS,
        }
    }
    fn interval(&self) -> u32 {
        match &self.condition {
            Condition::Check {
                interval_seconds, ..
            } => interval_seconds.unwrap_or(super::DEFAULT_CHECK_INTERVAL_SECONDS),
            _ => super::DEFAULT_CHECK_INTERVAL_SECONDS,
        }
    }
}

fn key(id: &str, index: usize) -> String {
    format!("{id}#{index}")
}

/// A condition's fingerprint, so a record kept for a condition an item no longer
/// declares at that place is never read as its replacement's.
fn digest(condition: &Condition) -> String {
    Sha256::digest(serde_json::to_vec(condition).unwrap_or_default())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// The private record of every check's claim and last run, beside the journal.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Record {
    version: u32,
    checks: BTreeMap<String, Entry>,
}

/// One check's claim and its last run.
// llmlint: ignore-block[invalid_states_unrepresentable] a private record this module alone writes, under the index lock, and reads back; a claim's three fields are set and cleared together by `claim` and `record`, and a run's four by `record`, so the halves only ever stand whole.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Entry {
    digest: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    claimed_at_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    claimed_by: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    claim_lapses_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    ran_at_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    ran_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    outcome: Option<HumanItemCheckOutcome>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    detail: Option<String>,
}
// llmlint: ignore-end[invalid_states_unrepresentable]

impl Entry {
    /// Whether this is an entry [`claim`] and [`record`] write: a claim whole or absent, a
    /// run whole or absent, its time a moment, and a detail exactly on a failure.
    fn well_formed(&self) -> bool {
        let claim = [
            self.claimed_at_ms.is_some(),
            self.claimed_by.is_some(),
            self.claim_lapses_ms.is_some(),
        ];
        let run = [
            self.ran_at_ms.is_some(),
            self.ran_at.is_some(),
            self.outcome.is_some(),
        ];
        let whole = |parts: [bool; 3]| parts.iter().all(|p| *p) || parts.iter().all(|p| !*p);
        whole(claim)
            && whole(run)
            && self
                .ran_at
                .as_deref()
                .is_none_or(|at| super::rfc3339_millis(at).is_some())
            && (self.outcome == Some(HumanItemCheckOutcome::Failed)) == self.detail.is_some()
    }
}

fn record_path(store: &HumanItems) -> PathBuf {
    store.store_dir().join("checks.json")
}

/// The record as it stands. One that cannot be read is said and read as empty: every
/// check is then due, and runs once more.
fn read_record(store: &HumanItems) -> Record {
    let path = record_path(store);
    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(why) if why.kind() == std::io::ErrorKind::NotFound => return Record::default(),
        Err(why) => {
            eprintln!(
                "onepipeline: the check record {} cannot be read ({why}); every check is due",
                path.display()
            );
            return Record::default();
        }
    };
    // llmlint: ignore[changed_behavior_has_e2e] `keep` replaces this record whole through `write_atomic`, so one that does not parse is a hand edit or another version; reading it as empty only makes every check due, which runs each once more through the path the check journeys drive, and costs a person nothing.
    match serde_json::from_slice::<Record>(&bytes) {
        Ok(mut record) if record.version == RECORD_VERSION => {
            // An entry this module would never write is dropped, said: its check is then
            // due, and its next run writes it whole.
            let held = record.checks.len();
            record.checks.retain(|_, entry| entry.well_formed());
            if record.checks.len() != held {
                eprintln!(
                    "onepipeline: {} holds {} check entries this build never writes; their \
                     checks are due again",
                    path.display(),
                    held - record.checks.len()
                );
            }
            record
        }
        Ok(_) | Err(_) => {
            eprintln!(
                "onepipeline: {} is not a check record this build reads; every check is due",
                path.display()
            );
            Record::default()
        }
    }
}

fn keep(store: &HumanItems, record: &Record) -> Result<(), String> {
    let mut record = record.clone();
    record.version = RECORD_VERSION;
    crate::ledger::write_atomic(
        &record_path(store),
        &serde_json::to_vec(&record).map_err(|e| e.to_string())?,
        crate::ledger::Durability::Record,
    )
    .map_err(|e| e.to_string())
}

/// Each item's check statuses: one per check condition of its `expiry` that has run.
pub(super) fn statuses(
    store: &HumanItems,
    items: &BTreeMap<String, HumanItem>,
) -> BTreeMap<String, Vec<HumanItemCheckStatus>> {
    if !items.values().any(|item| {
        item.expiry
            .iter()
            .any(|c| matches!(c, Condition::Check { .. }))
    }) {
        return BTreeMap::new();
    }
    let record = read_record(store);
    let mut out = BTreeMap::new();
    for item in items.values() {
        let mut checks = Vec::new();
        for (index, condition) in item.expiry.iter().enumerate() {
            if !matches!(condition, Condition::Check { .. }) {
                continue;
            }
            let Some(entry) = record
                .checks
                .get(&key(&item.id, index))
                .filter(|e| e.digest == digest(condition))
            else {
                continue;
            };
            let (Some(ran_at), Some(outcome)) = (&entry.ran_at, entry.outcome) else {
                continue;
            };
            checks.push(HumanItemCheckStatus {
                condition: u32::try_from(index).unwrap_or(u32::MAX),
                last_run_at: ran_at.clone(),
                outcome,
                detail: entry
                    .detail
                    .clone()
                    .filter(|_| outcome == HumanItemCheckOutcome::Failed),
            });
        }
        if !checks.is_empty() {
            out.insert(item.id.clone(), checks);
        }
    }
    out
}

/// When a sweep next has anything to do for `items`: the earliest pending time
/// condition, and the earliest a check becomes due or its live claim lapses.
pub(super) fn next_wake(
    store: &HumanItems,
    items: &BTreeMap<String, HumanItem>,
    now: u64,
) -> Option<u64> {
    let open = items.values().filter(|i| i.state == HumanItemState::Open);
    let mut wake: Option<u64> = None;
    let mut sooner = |at: u64| wake = Some(wake.map_or(at, |w| w.min(at)));
    let mut record = None;
    for item in open {
        for (index, condition) in item.expiry.iter().enumerate() {
            match condition {
                Condition::Time { at: Some(at), .. } => {
                    if let Some(due) = super::rfc3339_millis(at) {
                        sooner(due);
                    }
                }
                Condition::Check { .. } => {
                    let record = record.get_or_insert_with(|| read_record(store));
                    let candidate = Candidate {
                        id: item.id.clone(),
                        index,
                        condition: condition.clone(),
                    };
                    let entry = record
                        .checks
                        .get(&candidate.key())
                        .filter(|e| e.digest == digest(condition));
                    let due = match entry {
                        Some(Entry {
                            claim_lapses_ms: Some(lapses),
                            ..
                        }) if *lapses > now => *lapses,
                        Some(Entry {
                            ran_at_ms: Some(ran),
                            ..
                        }) => ran.saturating_add(u64::from(candidate.interval()) * 1000),
                        _ => now,
                    };
                    sooner(due);
                }
                _ => {}
            }
        }
    }
    wake
}

/// Sweep every open item of `snapshot`: retire each whose time, run or node condition
/// holds, run the due checks of the rest, and retire each whose check answered that it
/// holds. Answers what it did and the items as their expiry left them.
///
/// # Errors
///
/// The journal's refusal of an expiry a condition that holds owes: the item is still
/// open, and a caller answering as though the sweep had run would hide that.
pub(super) fn sweep(
    store: &HumanItems,
    items: &BTreeMap<String, HumanItem>,
    runs: &BTreeMap<String, RunDag>,
) -> Result<Swept, super::HumanItemRefusal> {
    let now = crate::sys::now_millis();
    let mut held = Vec::new();
    let mut candidates = Vec::new();
    let mut evaluated = 0;
    for item in items.values().filter(|i| i.state == HumanItemState::Open) {
        evaluated += item.expiry.len() as u64;
        let found = item
            .expiry
            .iter()
            .find_map(|c| holds(c, runs, now).map(|words| (c, words)));
        if let Some((condition, words)) = found {
            held.push(Held {
                id: item.id.clone(),
                condition: condition.clone(),
                reason: reason_for(condition, None, words),
            });
            continue;
        }
        for (index, condition) in item.expiry.iter().enumerate() {
            if matches!(condition, Condition::Check { .. }) {
                candidates.push(Candidate {
                    id: item.id.clone(),
                    index,
                    condition: condition.clone(),
                });
            }
        }
    }
    let mut sweep = HumanItemExpirySweep::default();
    // Most sweeps find every check fresh: read without the lock, and take it only to
    // claim what looks due, which the claim then decides again under it.
    if !candidates.is_empty() {
        let record = read_record(store);
        candidates.retain(|candidate| claimable(&record, candidate, now));
    }
    if !candidates.is_empty() {
        let claimed = claim(store, items, &candidates, now);
        let ran = run_all(&claimed);
        record(store, &ran);
        let mut answered = BTreeSet::new();
        for (candidate, run) in &ran {
            sweep.checks_run += 1;
            if run.outcome() == HumanItemCheckOutcome::Failed {
                sweep.checks_failed += 1;
            }
            let Answer::Holds { reason } = &run.answer else {
                continue;
            };
            if answered.insert(candidate.id.clone()) {
                held.push(Held {
                    id: candidate.id.clone(),
                    condition: candidate.condition.clone(),
                    reason: reason_for(
                        &candidate.condition,
                        reason.clone(),
                        format!(
                            "the check {} answered that the item no longer matters",
                            program_of(&candidate.condition)
                        ),
                    ),
                });
            }
        }
    }
    let expired = if held.is_empty() {
        Vec::new()
    } else {
        store
            .expire(held)
            .map_err(|why| super::HumanItemRefusal::Conflict {
                reason: format!(
                    "an expiry condition holds and the item could not be expired: {why}; the \
                     next sweep expires it"
                ),
            })?
    };
    sweep.expired = expired.iter().map(|item| item.id.clone()).collect();
    Ok(Swept {
        sweep,
        expired,
        evaluated,
    })
}

/// Whether `record` leaves `candidate` free to be claimed at `now`: no live claim holds
/// it, and it has never run or last ran an interval ago.
fn claimable(record: &Record, candidate: &Candidate, now: u64) -> bool {
    let Some(entry) = record
        .checks
        .get(&candidate.key())
        .filter(|e| e.digest == digest(&candidate.condition))
    else {
        return true;
    };
    let live = entry.claim_lapses_ms.is_some_and(|lapses| now < lapses);
    let due = entry
        .ran_at_ms
        .is_none_or(|ran| now >= ran.saturating_add(u64::from(candidate.interval()) * 1000));
    !live && due
}

fn program_of(condition: &Condition) -> String {
    match condition {
        Condition::Check { command, .. } => command.first().cloned().unwrap_or_default(),
        _ => String::new(),
    }
}

/// Claim every due candidate under the store's index lock: one whose check has never
/// run or last ran an interval ago, and that no live claim holds. A record the lock or
/// the write refuses claims nothing, so nothing runs that two readers could both run.
fn claim(
    store: &HumanItems,
    items: &BTreeMap<String, HumanItem>,
    candidates: &[Candidate],
    now: u64,
) -> Vec<Candidate> {
    let lock_path = store.index_lock_path();
    // llmlint: ignore-block[changed_behavior_has_e2e] every sweep follows a read that took this same lock moments before, so a lock that cannot be taken refuses that read and no sweep reaches here; only a lock file replaced by something unopenable between the two does, which no journey can time from outside. What it guarantees — nothing claimed runs unclaimed — is the claim the concurrent-readers journey holds.
    let _lock = match crate::sys::open_locked_append(&lock_path) {
        Ok(lock) => lock,
        Err(why) => {
            eprintln!(
                "onepipeline: no check is run, because {} could not be locked: {why}",
                lock_path.display()
            );
            return Vec::new();
        }
    };
    // llmlint: ignore-end[changed_behavior_has_e2e]
    let mut record = read_record(store);
    let held = record.checks.len();
    // A record only ever speaks for conditions items still declare where it says.
    record.checks.retain(|key, entry| {
        key.rsplit_once('#')
            .and_then(|(id, index)| Some((items.get(id)?, index.parse::<usize>().ok()?)))
            .and_then(|(item, index)| item.expiry.get(index))
            .is_some_and(|condition| digest(condition) == entry.digest)
    });
    let pruned = record.checks.len() != held;
    let mut claimed = Vec::new();
    for candidate in candidates {
        if !claimable(&record, candidate, now) {
            continue;
        }
        let fingerprint = digest(&candidate.condition);
        let entry = record.checks.entry(candidate.key()).or_default();
        // Another condition's runs never stand for this one's.
        if entry.digest != fingerprint {
            *entry = Entry {
                digest: fingerprint,
                ..Entry::default()
            };
        }
        let window = Duration::from_secs(u64::from(candidate.timeout())) + CLAIM_GRACE;
        entry.claimed_at_ms = Some(now);
        entry.claimed_by = Some(crate::sys::pid());
        entry.claim_lapses_ms =
            Some(now.saturating_add(u64::try_from(window.as_millis()).unwrap_or(u64::MAX)));
        claimed.push(candidate.clone());
    }
    if claimed.is_empty() && !pruned {
        return claimed;
    }
    if let Err(why) = keep(store, &record) {
        eprintln!("onepipeline: no check is run, because its claim could not be recorded: {why}");
        return Vec::new();
    }
    claimed
}

/// What one run of a check came to, and when it was claimed.
struct CheckRun {
    ran_at_ms: u64,
    answer: Answer,
}

/// A check's answer, in the result protocol's terms.
enum Answer {
    /// It printed `"expired": true`, with the nonblank reason it printed, if any.
    Holds { reason: Option<String> },
    /// It printed `"expired": false`.
    NotYet,
    /// Anything else, and why.
    Failed { detail: String },
}

impl CheckRun {
    fn outcome(&self) -> HumanItemCheckOutcome {
        match self.answer {
            Answer::Holds { .. } => HumanItemCheckOutcome::Holds,
            Answer::NotYet => HumanItemCheckOutcome::NotYet,
            Answer::Failed { .. } => HumanItemCheckOutcome::Failed,
        }
    }
    fn detail(&self) -> Option<String> {
        match &self.answer {
            Answer::Failed { detail } => Some(detail.clone()),
            Answer::Holds { .. } | Answer::NotYet => None,
        }
    }
}

/// Run every claimed check at once, and wait for the last: a sweep is held at most as
/// long as its longest timeout, never the sum of them.
fn run_all(claimed: &[Candidate]) -> Vec<(Candidate, CheckRun)> {
    let handles: Vec<_> = claimed
        .iter()
        .cloned()
        .map(|candidate| {
            std::thread::Builder::new()
                .name(format!("check-{}", candidate.key()))
                .spawn(move || {
                    let run = run_check(&candidate);
                    (candidate, run)
                })
        })
        .collect();
    handles
        .into_iter()
        .zip(claimed)
        .map(|(handle, candidate)| {
            // llmlint: ignore[changed_behavior_has_e2e] a thread the host refuses to start, or a panic inside one check's run, is a host or engine fault no journey can stage from outside; it is recorded as the same `failed` outcome every failing check is, which the failing-checks journey drives, so the item stays open with its failure shown.
            handle
                .map_err(|e| e.to_string())
                .and_then(|h| h.join().map_err(|_| "its thread panicked".to_owned()))
                .unwrap_or_else(|why| {
                    (
                        candidate.clone(),
                        failed(
                            crate::sys::now_millis(),
                            format!("the check could not be started: {why}"),
                        ),
                    )
                })
        })
        .collect()
}

fn failed(ran_at_ms: u64, detail: String) -> CheckRun {
    CheckRun {
        ran_at_ms,
        answer: Answer::Failed { detail },
    }
}

/// Record each run's outcome against its check, releasing its claim.
fn record(store: &HumanItems, ran: &[(Candidate, CheckRun)]) {
    if ran.is_empty() {
        return;
    }
    let lock_path = store.index_lock_path();
    // llmlint: ignore-block[changed_behavior_has_e2e] reachable only if the lock the claim took moments before, and the read before it, stops opening while the checks run, which no journey can time from outside; an outcome not recorded leaves the claim to lapse at its timeout and grace and the check to be claimed again, the recovery the dead-claimant journey drives.
    let _lock = match crate::sys::open_locked_append(&lock_path) {
        Ok(lock) => lock,
        Err(why) => {
            eprintln!(
                "onepipeline: the checks that ran are not recorded, because {} could not be \
                 locked: {why}",
                lock_path.display()
            );
            return;
        }
    };
    // llmlint: ignore-end[changed_behavior_has_e2e]
    let mut record = read_record(store);
    for (candidate, run) in ran {
        record.checks.insert(
            candidate.key(),
            Entry {
                digest: digest(&candidate.condition),
                claimed_at_ms: None,
                claimed_by: None,
                claim_lapses_ms: None,
                ran_at_ms: Some(run.ran_at_ms),
                ran_at: Some(crate::sys::rfc3339_from_millis(run.ran_at_ms)),
                outcome: Some(run.outcome()),
                detail: run.detail(),
            },
        );
    }
    // llmlint: ignore[changed_behavior_has_e2e] the claim was written to this same record moments before, so only a host that stops taking it while the checks run reaches here, which no journey can time; the claim then lapses at its timeout and grace and the check is claimed again, the recovery the dead-claimant journey drives.
    if let Err(why) = keep(store, &record) {
        eprintln!("onepipeline: the checks that ran could not be recorded: {why}");
    }
}

/// A buffer a reader thread fills and the caller takes whatever is in it.
type Shared = Arc<Mutex<Vec<u8>>>;

/// Read `from` on a thread of its own into `into`, keeping at most `keep` bytes — the
/// first of them, or with `tail` the last — and draining the rest so the program is
/// never blocked writing. Answers a receiver told only when the stream ends cleanly —
/// never on a read error, nor when the reader could not start — and whether more than
/// `keep` arrived.
fn reader(
    from: Option<impl Read + Send + 'static>,
    keep: usize,
    tail: bool,
) -> (
    Shared,
    Arc<std::sync::atomic::AtomicBool>,
    std::sync::mpsc::Receiver<()>,
) {
    let into: Shared = Arc::default();
    let over = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let (done, ended) = std::sync::mpsc::channel();
    let (kept, past) = (Arc::clone(&into), Arc::clone(&over));
    // llmlint: ignore-block[changed_behavior_has_e2e] a reader thread the host refuses to start, or a pipe that errors mid-read, is a host fault no journey can stage; either leaves the stream unreported as ended, which `run_check` reads as output not read whole — the `failed` outcome the lingering-output journey drives — so a partial answer never decides a check.
    let _ = std::thread::Builder::new()
        .name("check-output".into())
        .spawn(move || {
            let mut buffer = [0_u8; 8192];
            let mut ended_cleanly = from.is_none();
            if let Some(mut from) = from {
                loop {
                    let read = match from.read(&mut buffer) {
                        Ok(0) => {
                            ended_cleanly = true;
                            break;
                        }
                        Ok(read) => read,
                        Err(why) if why.kind() == std::io::ErrorKind::Interrupted => continue,
                        Err(_) => break,
                    };
                    let mut held = kept
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    held.extend_from_slice(&buffer[..read]);
                    if held.len() > keep {
                        past.store(true, std::sync::atomic::Ordering::Relaxed);
                        if tail {
                            let cut = held.len() - keep;
                            held.drain(..cut);
                        } else {
                            held.truncate(keep);
                        }
                    }
                }
            }
            if ended_cleanly {
                let _ = done.send(());
            }
        });
    // llmlint: ignore-end[changed_behavior_has_e2e]
    (into, over, ended)
}

/// Run one check to its end: argv with no shell, from its `cwd`, in this process's
/// environment and [`super::CHECK_ITEM_ENV`], its process tree stopped at its timeout.
fn run_check(candidate: &Candidate) -> CheckRun {
    let started = crate::sys::now_millis();
    let Condition::Check { command, cwd, .. } = &candidate.condition else {
        return failed(started, "the condition is not a check".into());
    };
    let timeout = candidate.timeout();
    let cwd = cwd.as_deref().map(PathBuf::from).unwrap_or_default();
    if !cwd.is_dir() {
        return failed(
            started,
            format!("its working directory {} does not exist", cwd.display()),
        );
    }
    let (program, args) = command
        .split_first()
        .expect("a stored check names its program");
    // A process the check leaves behind must not hold this one's own streams open
    // behind it: a reader of this command reads them to their end.
    crate::sys::disown_standard_handles();
    let mut spawning = Command::new(program);
    // Its own group, because the stop at its timeout freezes its tree before it signals
    // it, and a group the kernel finds orphaned while a member is frozen takes `SIGHUP`
    // whole: in this reader's group that is the reader's caller — the shell job, or the
    // test — and never only the check.
    crate::sys::in_own_process_group(&mut spawning);
    let spawned = spawning
        .args(args)
        .current_dir(&cwd)
        .env(super::CHECK_ITEM_ENV, &candidate.id)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn();
    let mut child = match spawned {
        Ok(child) => child,
        Err(why) => return failed(started, format!("the check could not be started: {why}")),
    };
    let (stdout, over, stdout_ended) = reader(child.stdout.take(), STDOUT_LIMIT, false);
    let (stderr, _, stderr_ended) = reader(
        child.stderr.take(),
        crate::commandstep::STDERR_TAIL_BYTES,
        true,
    );
    let deadline = Instant::now() + Duration::from_secs(u64::from(timeout));
    let stop = |child: &mut std::process::Child| {
        let _ = crate::sys::stop(child.id(), crate::sys::Stop::Now);
        let _ = child.kill();
        let _ = child.wait();
    };
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Ok(status),
            Ok(None) if Instant::now() >= deadline => {
                stop(&mut child);
                break Err(format!("the check timed out after {timeout} s"));
            }
            Ok(None) => std::thread::sleep(POLL),
            // llmlint: ignore[changed_behavior_has_e2e] the host refusing to report on a child it started is a fault no journey can stage; the tree is stopped and the check recorded `failed`, the path the timeout journeys drive through the same stop.
            Err(why) => {
                stop(&mut child);
                break Err(format!("the check could not be waited for: {why}"));
            }
        }
    };
    // An answer is only read whole: a process the check left behind holding its stdout
    // open could still be writing the rest of it.
    let whole = stdout_ended.recv_timeout(OUTPUT_GRACE).is_ok();
    let _ = stderr_ended.recv_timeout(OUTPUT_GRACE);
    let taken = |buffer: &Shared| {
        buffer
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    };
    let said = taken(&stderr);
    let failing = |why: String| failed(started, crate::commandstep::with_tail(why, &said));
    let status = match status {
        Ok(status) => status,
        Err(why) => return failing(why),
    };
    if !status.success() {
        return failing(ended_by(status));
    }
    if over.load(std::sync::atomic::Ordering::Relaxed) {
        return failing(format!(
            "{MALFORMED}: it printed more than the {STDOUT_LIMIT} bytes the engine reads"
        ));
    }
    if !whole {
        return failing(format!(
            "{MALFORMED}: a process it started still held its output open after it exited"
        ));
    }
    let answer = match answer(&taken(&stdout)) {
        Some((true, reason)) => Answer::Holds { reason },
        Some((false, _)) => Answer::NotYet,
        None => return failing(MALFORMED.to_owned()),
    };
    CheckRun {
        ran_at_ms: started,
        answer,
    }
}

/// How a check that did not exit 0 ended: its exit status, or the signal that stopped it.
fn ended_by(status: std::process::ExitStatus) -> String {
    if let Some(code) = status.code() {
        return format!("the check exited with status {code}");
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt as _;
        if let Some(signal) = status.signal() {
            return format!("the check was stopped by signal {signal}");
        }
    }
    "the check was stopped by a signal".to_owned()
}

/// A check's answer: exactly one JSON object, surrounding whitespace allowed, with a
/// boolean `expired` and, if any, a string `reason` — a blank one read as none. `None`
/// for anything else.
fn answer(stdout: &[u8]) -> Option<(bool, Option<String>)> {
    let mut values = serde_json::Deserializer::from_slice(stdout).into_iter::<serde_json::Value>();
    let serde_json::Value::Object(object) = values.next()?.ok()? else {
        return None;
    };
    if values.next().is_some() {
        return None;
    }
    let expired = object.get("expired")?.as_bool()?;
    let reason = match object.get("reason") {
        None => None,
        Some(serde_json::Value::String(reason)) => {
            Some(reason.clone()).filter(|r| !r.trim().is_empty())
        }
        Some(_) => return None,
    };
    Some((expired, reason))
}

impl HumanItems {
    /// Sweep every open item's expiry conditions: retire each whose time, run or node
    /// condition holds, run the due checks of the rest — each at most once per interval
    /// on the host, however many readers sweep — and retire each whose check answered
    /// that it holds. A retired human step is released as a dismissal releases it.
    ///
    /// # Errors
    ///
    /// A store that cannot be read, or a journal that will not take an expiry a condition
    /// that holds owes; that item stays open and the next sweep expires it. A check that
    /// fails is no error: it is recorded on its item, which stays open.
    pub fn expire_due(&self) -> Result<HumanItemExpirySweep, super::HumanItemRefusal> {
        let (snapshot, _) = self.read_referenced()?;
        let swept = sweep(self, &snapshot.items, &snapshot.runs)?;
        self.release_expired(&swept.expired);
        Ok(swept.sweep)
    }

    /// Release the human step each expired item stands for, through its run's own
    /// commands; one that cannot be released now is released by its run's driver.
    pub(super) fn release_expired(&self, expired: &[HumanItem]) {
        for item in expired {
            // llmlint: ignore[changed_behavior_has_e2e] the same `runtime::release` a person's resolution makes, whose refusal — a run store that will not take the command — and the driver's later release `a_resolution_its_resolver_could_not_release_is_released_by_the_driver` drives; an expired step's release command is the dismissal's, which the human-step expiry journeys drive.
            if let Err(why) = super::runtime::release(self.root(), item) {
                eprintln!("onepipeline: {why}");
            }
        }
    }
}

/// What a driver's pass reads: every item after its sweep, the items that sweep
/// expired, and when a sweep next has anything to do.
pub(crate) struct DriverSweep {
    pub(crate) items: BTreeMap<String, HumanItem>,
    pub(crate) expired: Vec<HumanItem>,
    pub(crate) wake: Option<u64>,
}

impl HumanItems {
    /// A driver's pass over the store: the sweep every read makes, reading only the runs
    /// an open item's condition names, and the items as it left them. The steps it
    /// expired are the driver's to release.
    pub(crate) fn driver_sweep(&self) -> Result<DriverSweep, super::HumanItemRefusal> {
        let (snapshot, _) = self.read_referenced()?;
        // A driver holds and releases by what the store says even when an expiry it owes
        // cannot be written: that is said, and the next pass sweeps again.
        let swept = sweep(self, &snapshot.items, &snapshot.runs).unwrap_or_else(|why| {
            eprintln!("onepipeline: {why}");
            Swept {
                sweep: HumanItemExpirySweep::default(),
                expired: Vec::new(),
                evaluated: 0,
            }
        });
        let items = if swept.expired.is_empty() {
            snapshot.items
        } else {
            self.items()?
        };
        let wake = next_wake(self, &items, crate::sys::now_millis());
        Ok(DriverSweep {
            items,
            expired: swept.expired,
            wake,
        })
    }
}

/// Where a check's `cwd` is defaulted from: the raising process's working directory.
pub(super) fn raising_dir() -> Option<PathBuf> {
    std::env::current_dir().ok()
}

#[cfg(test)]
mod tests {
    use super::answer;

    #[test]
    fn only_one_object_with_a_boolean_expired_is_an_answer() {
        assert_eq!(answer(b" {\"expired\": true} \n"), Some((true, None)));
        assert_eq!(
            answer(b"{\"expired\": true, \"reason\": \"freed\", \"x\": 1}"),
            Some((true, Some("freed".into())))
        );
        assert_eq!(
            answer(b"{\"expired\": true, \"reason\": \"  \"}"),
            Some((true, None))
        );
        assert_eq!(answer(b"{\"expired\": false}"), Some((false, None)));
        for malformed in [
            &b""[..],
            b"not json",
            b"{\"expired\": true}{\"expired\": true}",
            b"[{\"expired\": true}]",
            b"{\"done\": true}",
            b"{\"expired\": \"yes\"}",
            b"{\"expired\": true, \"reason\": 3}",
            b"{\"expired\": true, \"reason\": null}",
        ] {
            assert_eq!(
                answer(malformed),
                None,
                "{}",
                String::from_utf8_lossy(malformed)
            );
        }
    }
}
