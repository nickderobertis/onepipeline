//! The per-run **fold checkpoint**: what a reader resumes from instead of
//! replaying a run's whole history.
//!
//! One `checkpoint.json` beside each run's `plan.json` and `summary.json`,
//! holding a fold of a prefix of the journal and a marker saying exactly how much
//! of the journal that fold accounts for. It exists because [`RunState`] is
//! *derived* rather than maintained: every view folded the entire merged store on
//! every invocation, and the reconcile loop re-folded it after **every applied
//! command** — so the cost of knowing where a run had got to was the cost of its
//! whole history, and history only grows. One run measured on this host answered
//! `status` in 0.35 s at 860 B and in 17.47 s at 22 MB.
//!
//! # The journal stays the authoritative record
//!
//! A checkpoint is a cache of a prefix of the journal and nothing is ever lost by
//! throwing one away. Four conditions make one unusable, and every one of them
//! falls back to a fold of the whole store, which is the answer every run had
//! before this document existed: it is **absent**; it cannot be **read or
//! parsed**; its **format version** is not the one this build writes; or its
//! **coverage marker is not corroborated** by the journal in front of it.
//!
//! That is also what makes this landing non-breaking in both directions. A build
//! that writes no checkpoint reads a run root exactly as it always did whether or
//! not one is sitting there — nothing reads the file but this module — and a
//! build that writes one reads a predecessor's run root by finding no checkpoint
//! and folding, which is what it would have done anyway.
//!
//! # The hazard the coverage marker answers
//!
//! [`journal::merge_order`] reorders the store before the fold — each stream in
//! its own `seq`, streams interleaved by `ts` — so a marker naming a byte prefix
//! of the *file* would let a checkpoint account for a record that a later read
//! places **behind** one it also accounts for, and the state would differ from a
//! full fold's by however much that reordering moved.
//!
//! The marker chosen is one a later reordering **cannot** invalidate, rather than
//! one discarded whenever a reordering could have happened. It carries three
//! things beside the byte count: how many records those bytes hold, the greatest
//! `(ts, stream)` among them, and the greatest `seq` folded per stream. A prefix
//! is only ever *extended* over a record that sorts at or after both — so the
//! covered records, in the order they were appended, are exactly the order
//! [`journal::merge_order`] puts them in — and a checkpoint is only ever *used*
//! when every record the store has grown by since also sorts at or after both.
//!
//! Those two conditions are the whole proof, and it is short. Write `P` for the
//! covered records and `T` for the rest. The merge is a k-way one: each stream is
//! queued in its own `seq`, and each pass takes the queue head with the least
//! `(ts, stream)`. Every record of `T` sorts at or after every record of `P`, so
//! while any record of `P` is still queued no head belonging to `T` can win —
//! except at a tie, which is one stream, where the `seq` condition puts `P`'s
//! record first. So the merge emits all of `P`, in the order it emits `P` alone,
//! and then all of `T`. Folding the checkpoint's state and then the records the
//! store has grown by therefore lands on the state the whole store folds to.
//!
//! Where a record arrives that the marker cannot be extended over — a producer
//! whose clock runs behind this host's, an `oneharness-session` published out of
//! band and stamped when its session opened — the coverage simply stops there.
//! The state is still right: what is lost is the saving on the records past it,
//! and never the answer.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::event::Envelope;
use crate::journal;
use crate::ledger::{self, RunPaths};
use crate::projection::{self, RunState};

/// The schema version of the checkpoint document.
///
/// The whole compatibility statement, and it is [`crate::summary`]'s: a reader
/// that met a document it does not understand and folded from it anyway would
/// report a run's state out of fields that mean something else. A version this
/// build does not write is **refused**, and a refused checkpoint is not an error
/// — it is a run that folds.
pub(crate) const CHECKPOINT_SCHEMA_VERSION: u32 = 1;

/// Read the version, refusing a document this build cannot honestly read.
fn this_version<'de, D: serde::Deserializer<'de>>(reader: D) -> Result<u32, D::Error> {
    let found = u32::deserialize(reader)?;
    if found != CHECKPOINT_SCHEMA_VERSION {
        return Err(serde::de::Error::custom(format!(
            "checkpoint schema_version {found}, and this build reads {CHECKPOINT_SCHEMA_VERSION}"
        )));
    }
    Ok(found)
}

/// Where one record sorts **between** streams: its timestamp, then its stream.
///
/// The whole of [`journal::merge_order`]'s between-stream rule, as one
/// comparable value — timestamp first, stream id as the tie-break that makes the
/// order deterministic rather than meaningful. Ordered by `derive`, in the field
/// order the merge compares them in.
#[derive(Debug, Clone, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Placed {
    /// The record's own timestamp, as its producer stamped it.
    pub(crate) ts: String,
    /// The stream that wrote it.
    pub(crate) stream: String,
}

/// Where one record sorts, as the merge order compares it.
fn placed(event: &Envelope) -> Placed {
    Placed {
        ts: event.ts.clone(),
        stream: event.stream.clone(),
    }
}

/// **How much of a run's journal a folded state accounts for.**
///
/// Not a byte count alone: the module note above says why a byte prefix of the
/// file is not by itself a prefix of the order the fold is applied in, and what
/// the other three fields are for.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Coverage {
    /// The bytes of the journal, from its first, this state accounts for.
    ///
    /// Always a **record boundary**, and only ever a boundary between *finished*
    /// records: a marker inside a line whose writer had not finished it would
    /// lose that record the moment the writer did.
    pub(crate) bytes: u64,
    /// How many records those bytes hold, this build's unreadable lines
    /// included.
    ///
    /// What "folded only the records the checkpoint does not account for" is
    /// counted against, and it counts lines rather than foldable ones for the
    /// reason [`journal::read_after`] pairs a record it could not read with its
    /// size: a line this build cannot read is still a line the file holds.
    pub(crate) records: u64,
    /// The greatest [`Placed`] among the records folded, or absent where none
    /// were.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) at: Option<Placed>,
    /// The greatest `seq` folded per stream.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub(crate) streams: BTreeMap<String, u64>,
}

impl Coverage {
    /// Whether the merge order puts every record this accounts for **in front
    /// of** `event`.
    ///
    /// The one comparison the module note's proof rests on, asked in both
    /// directions of the same fact: extending a coverage over a record, and
    /// deciding that a coverage is still corroborated by a store that has grown.
    fn is_in_front_of(&self, event: &Envelope) -> bool {
        let placed = placed(event);
        self.at.as_ref().is_none_or(|at| *at <= placed)
            && self
                .streams
                .get(&event.stream)
                .is_none_or(|reached| *reached <= event.seq)
    }

    /// Account for one more record.
    ///
    /// A record this build could not read moves the byte count and the record
    /// count and **nothing else**: it folds to nothing, so where the merge order
    /// would place it decides nothing either, and holding the later records
    /// against a line with no timestamp of its own would stop a coverage that has
    /// lost no accuracy at all.
    fn absorb(&mut self, event: Option<&Envelope>, bytes: u64) {
        self.bytes += bytes;
        self.records += 1;
        let Some(event) = event else {
            return;
        };
        let placed = placed(event);
        if self.at.as_ref().is_none_or(|at| *at < placed) {
            self.at = Some(placed);
        }
        let reached = self.streams.entry(event.stream.clone()).or_default();
        *reached = (*reached).max(event.seq);
    }
}

/// One run's fold, and the marker saying how much of its journal it accounts
/// for.
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Checkpoint {
    /// The document's own version, so a reader can refuse one it does not
    /// understand. See [`CHECKPOINT_SCHEMA_VERSION`].
    #[serde(deserialize_with = "this_version")]
    schema_version: u32,
    /// The run this is a fold of.
    ///
    /// Checked against the run root it was found in, exactly as
    /// [`crate::summary::RunSummary`] checks its own: a document copied between
    /// run roots describes a run nobody is asking about.
    run_id: String,
    /// How much of that run's journal [`state`](Self::state) accounts for.
    coverage: Coverage,
    /// The fold itself.
    state: RunState,
}

/// Which order the records a store has grown by are folded in.
///
/// The two readers of a run disagree about this today and this type is where
/// that disagreement is *stated* rather than removed: [`crate::views`] folds the
/// merged store, and the reconcile loop folds the store as it was appended. A
/// checkpoint serves both because the records it accounts for are in the one
/// order both agree on — see the module note — so what each reader is choosing
/// here is only how to fold the records past it, which is the same choice each
/// made about the whole store before.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Order {
    /// Fold them in the order they were appended.
    Appended,
    /// Fold them in [`journal::merge_order`].
    Merged,
}

/// A run's folded state, and how much of its journal that state accounts for.
///
/// Held by a caller that folds a run more than once — the reconcile loop, which
/// re-folds after every applied command — so each of those folds costs the
/// records that arrived since the last one rather than the run's whole history.
/// A caller that folds once takes [`state`](Self::state) and drops it.
#[derive(Debug)]
pub(crate) struct Projected {
    state: RunState,
    coverage: Coverage,
    /// How many journal records the last fold took.
    ///
    /// Kept on the value rather than read back off the process-wide counter this
    /// also reports to, so a check can hold one fold to what the store grew by
    /// without a fold running beside it moving the number.
    took: u64,
}

impl std::ops::Deref for Projected {
    type Target = RunState;

    fn deref(&self) -> &RunState {
        &self.state
    }
}

impl std::ops::DerefMut for Projected {
    fn deref_mut(&mut self) -> &mut RunState {
        &mut self.state
    }
}

impl Projected {
    /// Fold a run, resuming from its checkpoint where there is a usable one.
    pub(crate) fn open(paths: &RunPaths, order: Order) -> Self {
        let mut projected = match stored(paths) {
            Some(checkpoint) => Self {
                state: checkpoint.state,
                coverage: checkpoint.coverage,
                took: 0,
            },
            None => Self::empty(),
        };
        projected.refresh(paths, order);
        projected
    }

    /// Fold what the run's journal has grown by since this state last accounted
    /// for it.
    ///
    /// The state this leaves is the state a fold of the whole store leaves. Where
    /// the store cannot be placed against what is already folded — it is shorter
    /// than the marker, or it has grown by a record the marker's own records do
    /// not all sort in front of — the whole store is folded again, which is the
    /// same answer more slowly.
    pub(crate) fn refresh(&mut self, paths: &RunPaths, order: Order) {
        let journal = paths.journal();
        let mut grown = journal::finished_records_after(&journal, self.coverage.bytes);
        if length_of(&journal) < self.coverage.bytes || !self.corroborated(&grown) {
            *self = Self::empty();
            grown = journal::finished_records_after(&journal, 0);
        }
        self.took = grown.len() as u64;
        self.take(paths, grown, order);
        // Not folded from the journal and therefore not part of what a fold of it
        // produces: `crate::crossdag` fills this in **after** a fold, from another
        // run's ledger, and a caller that had done so already must find it exactly
        // as empty here as it would after re-folding the whole store. Cleared
        // rather than left standing, so this state and a full fold's cannot
        // differ by an answer one of them happens to remember.
        self.state.cross_dag = BTreeMap::new();
    }

    /// The state itself, for a caller that folded once.
    pub(crate) fn into_state(self) -> RunState {
        self.state
    }

    /// The fold of a store with nothing in it, as [`projection::fold`] starts
    /// from.
    fn empty() -> Self {
        Self {
            state: RunState {
                strict: true,
                ..RunState::default()
            },
            coverage: Coverage::default(),
            took: 0,
        }
    }

    /// How many journal records the last fold took.
    #[cfg(test)]
    pub(crate) fn took(&self) -> u64 {
        self.took
    }

    /// How much of the journal this state accounts for.
    #[cfg(test)]
    pub(crate) fn coverage(&self) -> &Coverage {
        &self.coverage
    }

    /// Whether every record the store has grown by sorts at or after every
    /// record this state accounts for.
    ///
    /// The fourth of the four conditions in the module note, and the one that
    /// cannot be decided from the document alone: it is a question about the
    /// journal in front of the marker, so it is asked of the journal.
    fn corroborated(&self, grown: &[(Option<Envelope>, u64)]) -> bool {
        grown
            .iter()
            .filter_map(|(event, _)| event.as_ref())
            .all(|event| self.coverage.is_in_front_of(event))
    }

    /// Fold the records the store has grown by, and write what may be accounted
    /// for.
    fn take(&mut self, paths: &RunPaths, grown: Vec<(Option<Envelope>, u64)>, order: Order) {
        if grown.is_empty() {
            return;
        }
        // What this fold cost, in the unit the saving is stated in: the records
        // the store has grown by, rather than every record the run has written.
        crate::loopstats::records_folded(grown.len() as u64);
        let accountable = extent(&self.coverage, &grown);
        let accounted_before = self.coverage.bytes;
        let mut ahead = Vec::new();
        for (index, (event, bytes)) in grown.into_iter().enumerate() {
            if index < accountable {
                if let Some(event) = &event {
                    projection::fold_one(&mut self.state, event);
                }
                self.coverage.absorb(event.as_ref(), bytes);
            } else if let Some(event) = event {
                ahead.push(event);
            }
        }
        // The state as of the marker, which is the only state the marker may be
        // written beside. Taken only where there is something past it: a coverage
        // that reached the end of the store is already that state, and cloning it
        // to say so would be the one copy this exists to avoid.
        let covered = (!ahead.is_empty()).then(|| self.state.clone());
        if order == Order::Merged {
            journal::merge_order(&mut ahead);
        }
        for event in &ahead {
            projection::fold_one(&mut self.state, event);
        }
        // Only where the marker actually moved. A store whose every record the
        // marker cannot be extended over — the shape the module note ends on —
        // would otherwise rewrite an unchanged document on every read, and a
        // reader that saved nothing would pay a write to say so.
        if self.coverage.bytes > accounted_before {
            self.write(paths, covered.as_ref().unwrap_or(&self.state));
        }
    }

    /// Write the checkpoint.
    ///
    /// Best effort, and its failure is never reported — [`crate::summary`]'s own
    /// cache is written on the same terms and for the same reasons: a read-only
    /// runs root, or a directory this reader may not write, costs the next reader
    /// a fold and costs this one nothing. Written atomically, so a reader beside
    /// a writer sees one whole document or the one before it, and two writers
    /// racing leave a document each of them would have written.
    fn write(&self, paths: &RunPaths, state: &RunState) {
        let _ = ledger::write_json(
            &paths.checkpoint(),
            &Checkpoint {
                schema_version: CHECKPOINT_SCHEMA_VERSION,
                run_id: paths.run.clone(),
                coverage: self.coverage.clone(),
                state: state.clone(),
            },
        );
    }
}

/// A run's whole state, folded from its checkpoint where there is a usable one.
///
/// What a caller that folds a run **once** takes — every view — spelled as one
/// call so it reads where `projection::fold(&journal::read(..))` used to.
pub(crate) fn fold(paths: &RunPaths, order: Order) -> RunState {
    Projected::open(paths, order).into_state()
}

/// The stored checkpoint, where there is one this build may fold from.
///
/// Three of the four conditions in the module note are decided here and answered
/// the same way: absent, unreadable or unparseable, and written at a version this
/// build does not read all hand back `None`, which is a full fold.
fn stored(paths: &RunPaths) -> Option<Checkpoint> {
    ledger::read_json_opt::<Checkpoint>(&paths.checkpoint())
        .filter(|checkpoint| checkpoint.run_id == paths.run)
}

/// How long the journal is, where there is one.
///
/// A journal that is not there is zero bytes long, which is what a run with a
/// directory and a launch record and no first record is.
fn length_of(journal: &std::path::Path) -> u64 {
    std::fs::metadata(journal).map_or(0, |about| about.len())
}

/// **How many of the records a store has grown by a coverage may account for.**
///
/// The leading ones that keep the marker's two conditions true, which is what
/// makes the covered records a prefix of the merge order rather than of the file
/// — see the module note.
///
/// Two passes rather than one, because a record may be accountable on its own and
/// still not be accountable *here*. The first takes the run of records each of
/// which sorts at or after everything accounted for before it. The second stops
/// that run wherever a record **past** it does not sort at or after what the run
/// had reached: extending over that one would be writing a marker the very next
/// reader could not corroborate, and the coverage would fall straight back to a
/// full fold. Only records past the first pass's run can do that — the ones
/// inside it sort at or after every earlier one by construction — so the second
/// pass is a walk of two lists rather than a comparison of every pair.
fn extent(coverage: &Coverage, grown: &[(Option<Envelope>, u64)]) -> usize {
    let mut running = coverage.clone();
    let mut ordered = 0;
    for (event, bytes) in grown {
        if let Some(event) = event {
            if !running.is_in_front_of(event) {
                break;
            }
        }
        running.absorb(event.as_ref(), *bytes);
        ordered += 1;
    }
    let barrier = Barrier::of(&grown[ordered..]);
    let mut accountable = 0;
    for (event, _) in &grown[..ordered] {
        // The record itself rather than the coverage it would leave: everything
        // absorbed before it already sorts in front of the barrier — the coverage
        // handed in did, or it would not have been usable, and every record after
        // that passed this same test — so the one record being added is the only
        // thing that can newly reach past it.
        if event
            .as_ref()
            .is_some_and(|event| !barrier.is_behind(event))
        {
            break;
        }
        accountable += 1;
    }
    accountable
}

/// The earliest place anything the coverage may **not** reach sorts at.
///
/// One value taken once rather than a comparison per candidate boundary: what
/// stops a coverage extending is the least-sorting record past it, and that
/// record does not change as the boundary moves.
#[derive(Debug, Default)]
struct Barrier {
    at: Option<Placed>,
    streams: BTreeMap<String, u64>,
}

impl Barrier {
    /// Where the records past a candidate coverage begin.
    fn of(ahead: &[(Option<Envelope>, u64)]) -> Self {
        let mut barrier = Self::default();
        for event in ahead.iter().filter_map(|(event, _)| event.as_ref()) {
            let placed = placed(event);
            if barrier.at.as_ref().is_none_or(|at| placed < *at) {
                barrier.at = Some(placed);
            }
            let reached = barrier
                .streams
                .entry(event.stream.clone())
                .or_insert(event.seq);
            *reached = (*reached).min(event.seq);
        }
        barrier
    }

    /// Whether one record a coverage would absorb still sorts in front of
    /// everything past it.
    fn is_behind(&self, event: &Envelope) -> bool {
        let placed = placed(event);
        self.at.as_ref().is_none_or(|at| placed <= *at)
            && self
                .streams
                .get(&event.stream)
                .is_none_or(|barrier| event.seq <= *barrier)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::{EventKind, Labels, Source, ENVELOPE_VERSION};
    use crate::journal::{Journal, PipelineKind};
    use crate::ledger::LaunchRecord;
    use crate::plan::{Goal, Node, Plan, PLAN_SCHEMA_VERSION};
    use serde_json::{json, Value};
    use std::path::{Path, PathBuf};

    /// A scratch runs root of this journey's own.
    ///
    /// Named after the journey rather than after the process, so what a run of
    /// this file leaves behind says which check left it — and so nothing here
    /// keys durable state on a pid, which is a separate defect of this
    /// repository's verification.
    fn scratch(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("onepipeline-checkpoint-{name}"));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("a scratch root");
        root
    }

    fn plan(nodes: &[&str]) -> Plan {
        Plan {
            schema_version: PLAN_SCHEMA_VERSION,
            goal: Some(Goal {
                text: "fold a run without replaying it".into(),
            }),
            name: Some("demo".into()),
            concurrency: 4,
            tasks: nodes
                .iter()
                .map(|id| Node {
                    id: (*id).to_string(),
                    persona: Some("engineer".into()),
                    task: Some("## What\ndo it".into()),
                    ..Node::default()
                })
                .collect(),
        }
    }

    /// A run root with a launch record, as `start` leaves one.
    fn a_run(root: &Path, run: &str) -> RunPaths {
        let paths = RunPaths::under(root, run);
        paths.create().expect("the run directory");
        let record = LaunchRecord {
            run_id: run.to_string(),
            project: "plans:demo".into(),
            dir: PathBuf::from("/tmp/launch"),
            graph: String::new(),
            graph_run: String::new(),
            observer_runs: Vec::new(),
            observer_ending: String::new(),
            node_graph: "graph".into(),
            pr_author_graph: String::new(),
            node_validator: String::new(),
            envelope_reviewer: String::new(),
            launcher: "checkpoint-journey".into(),
            session: "a-session".into(),
            pid: 0,
            host: String::new(),
            started: String::new(),
            started_at: crate::sys::now_rfc3339(),
            heartbeat_interval: 1_800,
            dag_sets: Vec::new(),
            node_sets: Vec::new(),
            adoptions: 0,
            filters: crate::filter::Filters::default(),
        };
        crate::ledger::write_json(&paths.launch(), &record).expect("a launch record");
        paths
    }

    /// A run whose store carries a plan and one node's dispatch, through the
    /// real journal writer.
    fn a_recorded_run(root: &Path, run: &str) -> RunPaths {
        let paths = a_run(root, run);
        let mut journal = Journal::open(&paths);
        journal
            .emit(
                PipelineKind::RunStarted,
                crate::journal::labels(run, None),
                crate::journal::payload(&[("plan", json!(plan(&["build", "ship"])))]),
            )
            .expect("appended");
        journal
            .emit(
                PipelineKind::NodeDispatched,
                crate::journal::labels(run, Some("build")),
                crate::journal::payload(&[("persona", json!("engineer")), ("attempt", json!(1))]),
            )
            .expect("appended");
        paths
    }

    /// Settle one node, through the same writer.
    fn settle(paths: &RunPaths, node: &str, status: &str) {
        Journal::open(paths)
            .emit(
                PipelineKind::NodeSettled,
                crate::journal::labels(&paths.run, Some(node)),
                crate::journal::payload(&[("status", json!(status))]),
            )
            .expect("appended");
    }

    /// One record of a stream of its own, appended straight to the store.
    ///
    /// The merged store interleaves three producers, and only a record carrying
    /// another producer's stream and stamp can put the file's order and the
    /// merge order at odds — which is the hazard the coverage marker answers.
    fn relayed(paths: &RunPaths, stream: &str, seq: u64, ts: &str, node: &str) {
        let envelope = Envelope {
            v: ENVELOPE_VERSION,
            ts: ts.to_string(),
            stream: stream.to_string(),
            seq,
            source: Source::Agentgraph,
            kind: EventKind("turn-activity".into()),
            phase: None,
            labels: Labels {
                run_id: Some(paths.run.clone()),
                node: Some(node.to_string()),
                ..Labels::default()
            },
            payload: crate::journal::payload(&[("tool", json!("Edit"))]),
            artifacts: Vec::new(),
        };
        crate::ledger::append_line(
            &paths.journal(),
            &serde_json::to_string(&envelope).expect("an envelope serializes"),
        )
        .expect("appended");
    }

    /// The state a run folds to, compared field by field.
    ///
    /// Through the fold's own serialization rather than through a hand-written
    /// list of fields: a field added to [`RunState`] and left out of a comparison
    /// here would be one the checkpoint could silently drop.
    fn folded_as(state: &RunState) -> Value {
        serde_json::to_value(state).expect("a folded state serializes")
    }

    /// The state a reader with no checkpoint at all produces.
    ///
    /// Whatever was there is put back afterwards, because the fold this takes
    /// writes a checkpoint of its own: a control that left one behind would hand
    /// the journey a document covering the whole store in place of the one it was
    /// about to make a claim about.
    fn without_a_checkpoint(paths: &RunPaths, order: Order) -> Value {
        let held = std::fs::read(paths.checkpoint()).ok();
        let _ = std::fs::remove_file(paths.checkpoint());
        let whole = folded_as(&fold(paths, order));
        match held {
            Some(bytes) => std::fs::write(paths.checkpoint(), bytes).expect("put back"),
            None => {
                let _ = std::fs::remove_file(paths.checkpoint());
            }
        }
        whole
    }

    /// One fold of a run, and how many journal records it took.
    ///
    /// The count comes off the fold itself rather than off the process-wide
    /// counter it also reports to: this repository runs a test per process, and
    /// a check that only holds where that is true is a check that stops holding
    /// the day it is not.
    fn folding(paths: &RunPaths, order: Order) -> (Value, u64) {
        let projected = Projected::open(paths, order);
        (folded_as(&projected), projected.took())
    }

    /// The point of the document: the state is the state a full fold produces,
    /// through both orders a reader of this crate folds in.
    #[test]
    fn a_resumed_fold_lands_on_the_state_the_whole_store_folds_to() {
        for order in [Order::Merged, Order::Appended] {
            let root = scratch(&format!("resumed-is-whole-{order:?}").to_lowercase());
            let paths = a_recorded_run(&root, "r-resumed");
            // One read writes the checkpoint; the store then grows past it.
            let _ = fold(&paths, order);
            settle(&paths, "build", "done");
            relayed(&paths, "graph-1", 0, "2099-01-01T00:00:01.000Z", "ship");
            settle(&paths, "ship", "done");
            assert!(paths.checkpoint().is_file(), "no checkpoint was written");

            let resumed = folded_as(&fold(&paths, order));
            let whole = without_a_checkpoint(&paths, order);
            assert_eq!(resumed, whole, "{order:?} disagreed with a full fold");
            let _ = std::fs::remove_dir_all(&root);
        }
    }

    /// **Which records each path consumed**, observed rather than timed.
    ///
    /// The checkpoint is given an account of the covered records that the
    /// journal does not support, so the two answers are distinguishable: a
    /// reader that folded the covered records again would overwrite it, and one
    /// that folded only what the marker does not account for hands it back with
    /// the tail folded onto it.
    #[test]
    fn a_resumed_fold_takes_only_the_records_the_checkpoint_does_not_account_for() {
        let root = scratch("takes-only-the-tail");
        let paths = a_recorded_run(&root, "r-tail");
        settle(&paths, "build", "done");
        let _ = fold(&paths, Order::Merged);

        let mut stored = super::stored(&paths).expect("the checkpoint this read wrote");
        let covered = stored.coverage.records;
        assert_eq!(covered, 3, "the whole store was not accounted for");
        stored
            .state
            .outcomes
            .insert("build".into(), "carried-from-the-checkpoint".into());
        crate::ledger::write_json(&paths.checkpoint(), &stored).expect("the checkpoint is written");

        settle(&paths, "ship", "done");
        let (resumed, took) = folding(&paths, Order::Merged);
        assert_eq!(
            resumed["outcomes"]["build"], "carried-from-the-checkpoint",
            "the records the marker accounts for were folded again: {resumed}"
        );
        assert_eq!(
            resumed["recorded"]["ship"],
            json!({"at": "done"}),
            "the records past the marker were not folded: {resumed}"
        );
        assert_eq!(took, 1, "a resumed fold took more than the store grew by");

        let held = std::fs::read(paths.checkpoint()).expect("the poisoned checkpoint");
        std::fs::remove_file(paths.checkpoint()).expect("the checkpoint goes away");
        let (whole, took_whole) = folding(&paths, Order::Merged);
        std::fs::write(paths.checkpoint(), held).expect("put back");
        assert_eq!(
            whole["outcomes"].get("build"),
            None,
            "the control fold kept an account only the checkpoint carried"
        );
        assert_eq!(
            took_whole, 4,
            "the control fold did not take the whole store"
        );
        assert!(
            took < took_whole,
            "a resumed fold took {took} records and a full one took {took_whole}"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// The four conditions that make a checkpoint unusable, each driven through
    /// the real reader over a real run root in that state.
    ///
    /// One journey per condition, and every one of them asserts the same thing:
    /// the state is the state a reader with no checkpoint at all produces. A
    /// build that writes no checkpoint is the third of them — a document at a
    /// version this build does not write is exactly what a predecessor's reader
    /// leaves for it, read from the other side.
    fn a_view_of(paths: &RunPaths) -> Value {
        folded_as(
            &crate::views::RunView::open(paths)
                .expect("the run reads")
                .state,
        )
    }

    /// A run whose checkpoint carries an account of the covered records that the
    /// journal does not support, and the state a reader with no checkpoint at
    /// all produces from the same run.
    ///
    /// The account is planted so that **using** the document is observable: a
    /// checkpoint whose state agreed with the journal would be served and folded
    /// to the same answer, and a journey over it would pass whether or not the
    /// condition it names was honoured. There is a record past the marker too,
    /// because corroboration is a question about the journal in front of it and a
    /// marker with nothing in front of it is corroborated by saying nothing.
    fn a_run_with_a_checkpoint(name: &str) -> (PathBuf, RunPaths, Value) {
        let root = scratch(name);
        let paths = a_recorded_run(&root, "r-fallback");
        settle(&paths, "build", "done");
        let _ = fold(&paths, Order::Merged);
        let mut stored = super::stored(&paths).expect("the checkpoint that read wrote");
        stored
            .state
            .outcomes
            .insert("build".into(), "carried-from-the-checkpoint".into());
        crate::ledger::write_json(&paths.checkpoint(), &stored).expect("written");
        settle(&paths, "ship", "done");
        let whole = without_a_checkpoint(&paths, Order::Merged);
        assert_eq!(
            whole["outcomes"].get("build"),
            None,
            "the control fold carried an account only the checkpoint holds"
        );
        (root, paths, whole)
    }

    #[test]
    fn an_absent_checkpoint_folds_the_whole_store() {
        let (root, paths, whole) = a_run_with_a_checkpoint("absent");
        std::fs::remove_file(paths.checkpoint()).expect("the checkpoint goes away");
        assert_eq!(a_view_of(&paths), whole);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_checkpoint_that_cannot_be_read_folds_the_whole_store() {
        let (root, paths, whole) = a_run_with_a_checkpoint("unreadable");
        std::fs::write(paths.checkpoint(), b"{ this is not a checkpoint")
            .expect("the checkpoint is mangled");
        assert_eq!(a_view_of(&paths), whole);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_checkpoint_this_build_did_not_write_folds_the_whole_store() {
        let (root, paths, whole) = a_run_with_a_checkpoint("another-version");
        let mut document: Value = crate::ledger::read_json_opt(&paths.checkpoint())
            .expect("the checkpoint this run carries");
        document["schema_version"] = json!(CHECKPOINT_SCHEMA_VERSION + 1);
        crate::ledger::write_json(&paths.checkpoint(), &document).expect("written");
        assert_eq!(a_view_of(&paths), whole);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_coverage_the_journal_does_not_corroborate_folds_the_whole_store() {
        let (root, paths, whole) = a_run_with_a_checkpoint("uncorroborated");
        let mut stored = super::stored(&paths).expect("the checkpoint this run carries");
        // A marker claiming to account for records the store in front of it does
        // not sort after: the settlement appended since is stamped now, and this
        // says everything covered was written a century later.
        stored.coverage.at = Some(Placed {
            ts: "2199-01-01T00:00:00.000Z".into(),
            stream: "zzzz".into(),
        });
        crate::ledger::write_json(&paths.checkpoint(), &stored).expect("written");
        assert_eq!(a_view_of(&paths), whole);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A store the fold's own ordering rearranges **across** the marker's
    /// claimed coverage folds to the same state either way.
    ///
    /// The record appended last is stamped before the ones the checkpoint
    /// accounts for and belongs to a stream of its own, so the merge order puts
    /// it in front of them — which is precisely the arrival a byte marker alone
    /// could not answer.
    #[test]
    fn a_store_the_merge_order_rearranges_folds_the_same_state_either_way() {
        let root = scratch("rearranged");
        let paths = a_recorded_run(&root, "r-rearranged");
        settle(&paths, "build", "done");
        let _ = fold(&paths, Order::Merged);
        let covered = super::stored(&paths).expect("a checkpoint").coverage;
        assert!(covered.records > 0, "nothing was accounted for");

        relayed(&paths, "graph-0", 0, "1999-01-01T00:00:00.000Z", "build");
        let resumed = folded_as(&fold(&paths, Order::Merged));
        assert_eq!(resumed, without_a_checkpoint(&paths, Order::Merged));
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A store whose **first** records the merge order rearranges accounts for
    /// nothing rather than for something it cannot place.
    #[test]
    fn a_marker_is_never_extended_over_a_record_a_reordering_would_move() {
        let root = scratch("never-extended");
        let paths = a_run(&root, "r-unplaceable");
        relayed(&paths, "graph-b", 0, "2099-01-01T00:00:03.000Z", "build");
        relayed(&paths, "graph-a", 0, "2099-01-01T00:00:01.000Z", "build");
        let state = folded_as(&fold(&paths, Order::Merged));
        let covered = super::stored(&paths).map(|stored| stored.coverage);
        assert!(
            covered.is_none_or(|covered| covered.records == 0),
            "a record the merge order moves was accounted for"
        );
        assert_eq!(state, without_a_checkpoint(&paths, Order::Merged));
        let _ = std::fs::remove_dir_all(&root);
    }

    /// The reconcile loop's half: a re-fold after a change takes what the store
    /// grew by rather than the whole journal.
    #[test]
    fn a_refold_takes_what_the_store_grew_by_rather_than_the_whole_journal() {
        let root = scratch("refold");
        let paths = a_recorded_run(&root, "r-refold");
        for nth in 0..20 {
            settle(&paths, if nth % 2 == 0 { "build" } else { "ship" }, "done");
        }
        let mut projected = Projected::open(&paths, Order::Appended);
        assert_eq!(
            projected.took(),
            22,
            "the loop did not open on the whole store"
        );
        assert_eq!(
            projected.coverage().records,
            22,
            "the loop did not account for the store it opened"
        );

        settle(&paths, "build", "failed");
        projected.refresh(&paths, Order::Appended);
        let took = projected.took();
        assert_eq!(
            took, 1,
            "a re-fold took {took} records of a 23-record store"
        );
        assert_eq!(
            folded_as(&projected),
            without_a_checkpoint(&paths, Order::Appended),
            "the loop's state and a full fold's disagree"
        );
        let _ = std::fs::remove_dir_all(&root);
    }
}
