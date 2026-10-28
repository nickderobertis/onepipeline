//! The per-run **fold checkpoint**: what a reader resumes from instead of
//! replaying a run's whole history.
//!
//! One `checkpoint.json` beside each run's `plan.json` and `summary.json`, holding
//! a fold of a prefix of the journal and a marker saying how much of the journal
//! that fold accounts for. [`RunState`] is *derived* rather than maintained, so
//! before this every view folded the whole merged store per invocation and the
//! reconcile loop re-folded it per applied command: one run here answered `status`
//! in 0.35 s at 860 B and 17.47 s at 22 MB.
//!
//! # The journal stays the authoritative record
//!
//! Nothing is lost by throwing a checkpoint away. Four conditions make one
//! unusable and each falls back to folding the whole store, which is the answer
//! every run had before this document existed: **absent**; unreadable or
//! **unparseable**; at a **format version** this build does not write; or carrying
//! a **coverage marker the journal does not corroborate**. That is also what makes
//! the landing non-breaking both ways — nothing but this module reads the file, and
//! a predecessor's run root simply has none.
//!
//! The fourth condition is asked of the **file** only as far as it can be without
//! reading what the marker covers: the journal is at least as long as the marker
//! claims. The rest is asked of the **document** — its claims and the fold it
//! carries seal to what it says ([`Coverage::corroborated_by`]) — so an edited
//! claim or an edited fold takes a rebuild, and a journal cut back past the marker
//! takes one too.
//!
//! # The hazard the coverage marker answers
//!
//! [`journal::merge_order`] reorders the store before the fold — each stream in
//! its own `seq`, streams interleaved by `ts` — so a marker naming a byte prefix
//! of the *file* could account for a record a later read places **behind** one it
//! also accounts for.
//!
//! The marker chosen is one a reordering **cannot** invalidate: beside the byte
//! count it carries the record count, the greatest `(ts, stream)` covered and the
//! greatest `seq` per stream, and a prefix stays covered only while every record
//! past it sorts at or after all three.
//!
//! Why that suffices: write `P` for the covered records and `T` for the rest. The
//! merge queues each stream in its own `seq` and takes the head with the least
//! `(ts, stream)`. Every record of `T` sorts at or after every record of `P`, so
//! while any of `P` is queued no `T` head can win — except at a tie, which is one
//! stream, where the `seq` condition puts `P`'s record first. Hence
//! `merge_order(P ∪ T) = merge_order(P) ++ merge_order(T)`, so folding the
//! checkpoint's state and then what the store grew by lands on the state the whole
//! store folds to; extending the marker over the front of `T` is that statement
//! again with `P` grown.
//!
//! **The covered records are a prefix of the merge order, not of the file's own.**
//! A run's store has several appenders — the loop's writer, and the relay carrying
//! its dispatches' streams — so a relayed record stamped a millisecond before the
//! record appended in front of it is ordinary. A marker held to the file order
//! stopped at the first of those and never passed it: 6 records of a settled run's
//! store, against 18 for this one. A record the marker cannot be extended over
//! discards the checkpoint, and the marker written after the whole-store fold
//! covers it.
//!
//! # Nothing covered is read again
//!
//! The journal is append-only, and what this document covers is history no pass,
//! append, view or reply of a live run needs to read twice: on a long run that
//! history is hundreds of megabytes, and a reader that went back over it per
//! record made every record cost the whole run. So a reader resumes from this
//! document and the bytes appended past it, and nothing else:
//!
//! - The seal is over the document alone. It does not prove the covered bytes
//!   are still the bytes that were folded, and nothing on a live path reads them
//!   to ask.
//! - A record arriving past the marker that the merge order would place **in
//!   front of** a covered one — a worker's record relayed late, stamped when the
//!   worker wrote it — is folded **after** what is covered, in the merge order of
//!   what arrived with it. The covered fold is a barrier the merge does not
//!   reach back across. Nothing in the fold a node's settlement, its dispatch or
//!   its graph turns on is a relayed sibling record, and those are the records a
//!   driver writes in its own stream's order, so moving a late relayed record to
//!   the barrier moves only what that record says about its node's activity —
//!   and it says the same thing either side of the barrier.
//! - A document that cannot be used — absent, unreadable, at another version, of
//!   another run, refused by its seal, or longer than the journal — is rebuilt by
//!   **one streaming read** of the journal from its first byte, folded a bounded
//!   window at a time, so a rebuild costs one pass over the store and memory
//!   bounded by the window rather than by the run.
//!
//! # One order, for both readers
//!
//! One document holding one fold means one order: [`journal::merge_order`], which
//! [`crate::views`], [`crate::summary`] and [`crate::telemetry`] already read the
//! store in and which is the only ordering promise an envelope carries — within
//! what one read takes, with the covered fold as the barrier above.

// llmlint: ignore-file[invalid_states_unrepresentable] serialized fields an older build
// wrote, on `src/summary.rs`'s terms and carrying that file's own suppression. The shape a
// type could not exclude either — a state that is not the fold of the prefix it claims — is
// excluded only by folding that prefix, which is the cost this removes; what a type cannot
// say, `Coverage::sealed_with` says instead, by refusing any part of the document that has
// moved since a writer sealed it.
use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::event::Envelope;
use crate::journal;
use crate::ledger::{self, RunPaths};
use crate::projection::{self, RunState};

/// The schema version of the checkpoint document, refused where it is not this
/// one: a reader that folded from fields meaning something else would report a
/// state nobody recorded. See [`crate::summary`], which states the same.
///
/// Moved by a change to what the **fold** computes as well as by one to the
/// document's shape, because the document is a cache of the fold and its seal
/// covers the state a writer computed, not the state this build would. Version
/// 2 has version 1's fields; it was cut when a settlement started ending a
/// node's park (`projection::settlement_ends_the_park`), so a cached fold that
/// still holds the park is refolded rather than resumed from. Version 3 was cut
/// when the fold started recording the driver that let go of a run to fire its
/// run-end hook (`RunState::let_go_by`): a cached fold from before it read that
/// record as nothing, so it is refolded rather than resumed from. Version 4 was cut
/// when the fold started reading a settle's stated landing as the node's landing
/// (`RunState::stated_landings`): a cached fold from before it holds no such
/// landing, and resumed from it would go on reporting that work unlanded. Version
/// 5 was cut when `driver-adopted` started clearing the run's recorded stop
/// (`RunState::stop`): a cached fold of a stopped-then-adopted run from before it
/// still carries the stop, and resumed from it would go on reporting the
/// adopting driver dead and the run settled. Version 6 was cut when the fold
/// started keeping a stated change request past a later stated commit
/// (`RunState::stated_change_urls`), the fallback a version-5 fold does not hold.
/// Version 7 was cut when the fold started keeping the latest journalled run-wide
/// node overrides (`RunState::run_node_sets`): a version-6 fold does not hold
/// them, and resumed from it would dispatch the next node with the launch's list.
/// Version 8 was cut when the fold started keeping the closure rule's facts
/// (`RunState::closure`): a version-7 fold holds none of them, and resumed from it
/// would read a run driven under that rule as one that never was. Version 9 was
/// cut when the fold started keeping each node's base and the branches its
/// driver's passes retired (`RunState::bases`, `RunState::retired_branches`): a
/// version-8 fold holds neither, and resumed from it would go on telling a reader
/// that a retired branch may carry finished work. Version 10 was cut when the
/// fold started keeping what putting each kept branch on its origin found to do
/// (`RunState::remotes`): a version-9 fold holds none of it, and resumed from it
/// would report a node settled `preserved` without saying whether its branch
/// reached anywhere. Version 11 was cut when a `reparent` of a waiting human
/// approval started clearing its recorded wait: a version-10 fold over a journal
/// that holds one still carries the wait, and resumed from it would offer the
/// approval for attestation ahead of the prerequisites it was reparented onto.
/// Version 12 restores a continued lineage's rework provenance; an older fold
/// would forget which comments that lineage is already reworking. Version 13
/// carries that provenance's reason, instruction and superseded head (entry 130):
/// a version-12 fold holds none of them, and resumed from it a host would draft a
/// lineage's replies with no reason and none of the commits its rework added.
/// Version 14 keeps a node's last activity as the `transcript` reference it
/// recorded rather than as a copy, resolved when the readout is drawn: a
/// version-13 fold holds no reference, and resumed from it a node whose dispatch
/// references its history would read as still doing whatever its last copy said.
/// Version 15 is the document a reader resumes from **without reading what it
/// covers**: its seal is taken over its own claims and the fold alone, it carries
/// the summary's running [`Tally`] beside the state, and a record arriving after
/// what it covers is folded after it whatever its stamp says — see the module
/// note's *Nothing covered is read again*. An earlier version's seal is over the
/// covered journal bytes and it carries no tally, so it is rebuilt by one
/// streaming read rather than resumed from.
// llmlint: ignore[changed_behavior_has_e2e] a version-1 document is one the build before
// this one wrote, which no invocation of this build can produce: what a user can reach —
// a checkpoint at this version being resumed from, and one at a version this build does
// not write being refolded — is driven end to end in `tests/e2e/checkpoint.rs`. The
// refusal of the checked-in version-1 document itself is held by
// `the_document_the_build_before_this_one_wrote_is_refused_rather_than_read` below, over
// the real reader and the real file.
pub(crate) const CHECKPOINT_SCHEMA_VERSION: u32 = 15;

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
    pub(crate) ts: String,
    pub(crate) stream: String,
}

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
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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
    /// reason [`journal::finished_records_after`] pairs a record it could not read with its
    /// size: a line this build cannot read is still a line the file holds.
    pub(crate) records: u64,
    /// The greatest [`Placed`] among the records folded, or absent where none
    /// were.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) at: Option<Placed>,
    /// The greatest `seq` folded per stream.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub(crate) streams: BTreeMap<String, u64>,
    /// **The seal: this marker, over the document it is written in.**
    ///
    /// Every field above says something a reader would otherwise have to take on
    /// trust — how many bytes, how many records, how far the order reached — and so
    /// does the fold beside it. This is FNV-1a over those fields and then over the
    /// fold and the tally, so a reader recomputes it from the document and refuses
    /// one any part of which has moved since a writer sealed it. See
    /// [`sealed_with`](Self::sealed_with) for why the covered bytes are not in it.
    ///
    /// Hex, because a persisted 128-bit integer is one a consumer reading JSON
    /// numbers as doubles rounds.
    #[serde(serialize_with = "as_hex", deserialize_with = "of_hex")]
    pub(crate) digest: u128,
}

/// FNV-1a's 128-bit offset basis: the digest of no bytes at all.
pub(crate) const NOTHING_DIGESTED: u128 = 0x6c62_272e_07bb_0142_62b8_2175_6295_c58d;

/// FNV-1a's 128-bit prime, `2^88 + 0x13b`, written as the arithmetic rather than
/// as a constant nobody can check by eye.
const FNV_PRIME: u128 = (1 << 88) | 0x13b;

/// Digest more bytes, continuing from a digest already taken.
///
/// **FNV-1a is chosen because its state is its output**: a marker is extended
/// every time the store grows, so the digest of a longer prefix has to be reachable
/// from the one already taken plus the bytes that arrived. One whose streaming state
/// could not be carried would re-hash the whole prefix per applied command, which is
/// the cost this document removes.
///
/// An **integrity check and not a security boundary**, and what it detects is exactly
/// the accidents: a covered record truncated, half-written, or rewritten by a heal, a
/// copy, or an editor, where a rewrite landing on the same 128-bit value is not a
/// failure mode anyone here meets.
///
/// What it does **not** detect is a rewrite crafted to match, and no digest would.
/// This document is unauthenticated and sits in the run root beside the journal it
/// summarises, so anything able to rewrite covered bytes is equally able to rewrite
/// the seal over them; a cryptographic digest would move the cost of colliding by
/// accident and nothing else, for a dependency `AGENTS.md` guards. That is the right
/// boundary because the journal is the authoritative record and this is a discardable
/// cache of a prefix of it: the fallback either way is the whole-store fold every
/// reader did before this document existed.
pub(crate) fn digested(from: u128, bytes: &[u8]) -> u128 {
    bytes.iter().fold(from, |digest, byte| {
        (digest ^ u128::from(*byte)).wrapping_mul(FNV_PRIME)
    })
}

pub(crate) fn as_hex<S: serde::Serializer>(digest: &u128, writer: S) -> Result<S::Ok, S::Error> {
    writer.serialize_str(&format!("{digest:032x}"))
}

/// Read a digest, refusing anything [`as_hex`] would not have written.
///
/// Shared with the channel's queue projection, which seals its own claims the
/// same way and for the same reason.
///
/// The shape is checked here rather than left to `from_str_radix`, which takes a
/// leading sign, an upper-case digit, and any number of digits at all: each of
/// those reads back a value **no writer here produced**, and a document with one in
/// it is one nothing on this side wrote — a truncated value most of all, which
/// `from_str_radix` would otherwise hand back as a smaller number that seals to
/// nothing.
pub(crate) fn of_hex<'de, D: serde::Deserializer<'de>>(reader: D) -> Result<u128, D::Error> {
    let written = String::deserialize(reader)?;
    let refuse = |why: &str| serde::de::Error::custom(format!("digest '{written}': {why}"));
    let a_digit = |digit: &u8| matches!(digit, b'0'..=b'9' | b'a'..=b'f');
    if written.len() != 32 || !written.as_bytes().iter().all(a_digit) {
        return Err(refuse("not 32 lower-case hex digits"));
    }
    u128::from_str_radix(&written, 16).map_err(|e| refuse(&e.to_string()))
}

/// A marker over nothing, which is what a fold with no checkpoint starts from.
///
/// Spelled out rather than derived, because the digest of no bytes is FNV-1a's
/// offset basis and not zero — and a zero there would make the empty marker one
/// no journal corroborates.
impl Default for Coverage {
    fn default() -> Self {
        Self {
            bytes: 0,
            records: 0,
            at: None,
            streams: BTreeMap::new(),
            digest: NOTHING_DIGESTED,
        }
    }
}

impl Coverage {
    /// Whether the merge order puts every record this accounts for **in front
    /// of** `event`.
    ///
    /// The one comparison the module note's proof rests on, asked in both
    /// directions of the same fact: extending a coverage over a record, and
    /// deciding that a coverage is still corroborated by a store that has grown.
    pub(crate) fn is_in_front_of(&self, event: &Envelope) -> bool {
        let placed = placed(event);
        self.at.as_ref().is_none_or(|at| *at <= placed)
            && self
                .streams
                .get(&event.stream)
                .is_none_or(|reached| *reached <= event.seq)
    }

    /// This marker's seal: its own claims, then the fold and the tally written
    /// beside it.
    ///
    /// Everything the document asserts goes in, in a fixed order, so **anything
    /// edited no longer seals to what the document carries** — a claim, or the fold
    /// itself. [`digest`](Self::digest) does not go in, which is what stops it
    /// sealing over itself.
    ///
    /// The covered journal bytes do not go in either, and that is the point of
    /// version 15: a reader that had to digest them to trust this document read the
    /// whole history it exists to spare — see the module note's *Nothing covered is
    /// read again*.
    fn sealed_with(&self, state: &RunState, tally: &Tally) -> u128 {
        let mut sealed = digested(NOTHING_DIGESTED, &self.bytes.to_le_bytes());
        sealed = digested(sealed, &self.records.to_le_bytes());
        if let Some(at) = &self.at {
            sealed = digested(sealed, at.ts.as_bytes());
            sealed = digested(sealed, at.stream.as_bytes());
        }
        for (stream, reached) in &self.streams {
            sealed = digested(sealed, stream.as_bytes());
            sealed = digested(sealed, &reached.to_le_bytes());
        }
        // The fold as the document carries it. Through its own serialization rather
        // than field by field, so a field added to `RunState` is sealed by existing
        // rather than by somebody remembering to add it here — and the crate's
        // `float_roundtrip` is what makes a value read back the value written, so
        // the two sides digest the same bytes.
        match (serde_json::to_vec(state), serde_json::to_vec(tally)) {
            (Ok(state), Ok(tally)) => digested(digested(sealed, &state), &tally),
            // A state that will not serialize is one no writer could have sealed,
            // so nothing may match it.
            _ => sealed.wrapping_add(1),
        }
    }

    /// Whether the journal and the document together corroborate every claim this
    /// marker makes, short of reading what it covers.
    ///
    /// Two questions: the file is at least as long as the marker claims — a
    /// journal cut back, or replaced by a shorter one, holds none of what was
    /// folded — and everything the document asserts seals to what it carries.
    fn corroborated_by(&self, journal: &std::path::Path, state: &RunState, tally: &Tally) -> bool {
        length_of(journal) >= self.bytes && self.sealed_with(state, tally) == self.digest
    }

    /// Account for one more record.
    ///
    /// A record this build could not read moves the byte count and the record
    /// count and **nothing else**: it folds to nothing, so where the merge order
    /// would place it decides nothing either, and holding the later records
    /// against a line with no timestamp of its own would stop a coverage that has
    /// lost no accuracy at all.
    pub(crate) fn absorb(&mut self, event: Option<&Envelope>, bytes: u64) {
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
///
/// The one journal-derived snapshot of a run, and the type its writer and every
/// reader of `checkpoint.json` share: regenerable from `events.jsonl`, and safe to
/// delete.
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Checkpoint {
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
    coverage: Coverage,
    state: RunState,
    /// What the summary document is derived from beside the state, over the same
    /// records. See [`Tally`].
    tally: Tally,
}

/// **What a run's summary counts beside its state**, over the same records the
/// state was folded from.
///
/// Carried in the checkpoint so the journal writer that keeps `summary.json`
/// current resumes it from this document rather than by walking the store: the
/// state alone does not say how many records there were, which kind the order
/// ended on, how the run's clock was spent, or which nodes a judge turned down.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Tally {
    /// The run's clock and usage, a record at a time.
    pub(crate) aggregate: crate::telemetry::Aggregate,
    /// How many records were folded.
    pub(crate) events: u64,
    /// The kind of the record folded last.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) last_event_kind: Option<String>,
    /// The nodes some folded record carries a **failing judge verdict** for. A
    /// verdict is on the settlement that carried it and nothing later withdraws
    /// one, so the set only grows.
    #[serde(default, skip_serializing_if = "std::collections::BTreeSet::is_empty")]
    pub(crate) judged: std::collections::BTreeSet<String>,
    /// How many lines read were no record this build can read: a driver that meets
    /// one reports that the graph it drives may be missing an edit.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub(crate) unreadable: u64,
    /// Every line read that a [`journal::Integrity`] counts as lost, in file order,
    /// each where it is: what a view says of a store's own losses, kept here
    /// because the fold already met every one of them and a view asking again
    /// would read the whole store to be told.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) losses: Vec<LineLost>,
}

/// One line [`Tally::losses`] holds: [`journal::Lost`], placed by its line.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct LineLost {
    /// Which class it is lost under.
    pub(crate) kind: journal::LossKind,
    /// The 1-based line it is on, counted from the journal's first byte.
    pub(crate) line: u64,
    /// Where in the file the line begins.
    pub(crate) offset: u64,
    /// How many of its bytes are not a record this build can read.
    pub(crate) bytes: u64,
}

fn is_zero(count: &u64) -> bool {
    *count == 0
}

impl Tally {
    /// Count one record that belongs at the end of what has been folded.
    pub(crate) fn take(&mut self, paths: &RunPaths, event: &Envelope) {
        self.aggregate.fold(paths, event);
        self.events += 1;
        self.last_event_kind = Some(event.kind.0.clone());
        if let Some(node) = crate::report::a_judge_failed(event) {
            self.judged.insert(node.to_string());
        }
    }
}

/// Whether a [`Projected`] writes what it folds back to the run's checkpoint.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Persist {
    /// Written whenever the marker moves: the reconcile loop, and a reader that
    /// folds once and leaves the next reader what it folded.
    Yes,
    /// Held in memory only: a reader that folds on every append — a journal
    /// writer's summary, a driver's own rendering — and would otherwise rewrite
    /// the whole document per record.
    No,
}

/// A run's folded state, and how much of its journal that state accounts for.
///
/// Held by a caller that folds a run more than once — the reconcile loop, which
/// re-folds after every applied command — so each of those folds costs the
/// records that arrived since the last one rather than the run's whole history.
/// A caller that folds once takes [`into_state`](Self::into_state) and drops it.
#[derive(Debug)]
pub(crate) struct Projected {
    /// The fold of exactly what [`coverage`](Self::coverage) accounts for, which
    /// is the only state the marker may be written beside.
    covered: RunState,
    /// The tally over the same records.
    covered_tally: Tally,
    coverage: Coverage,
    /// That, with the records past the marker folded onto it: the whole store.
    ///
    /// Kept apart from the one above rather than derived from it on demand,
    /// because the records past the marker are re-folded on every pass — they are
    /// the store's own open instant, held back so an arrival beside them can still
    /// sort in front — and folding them onto a state that already carried them
    /// would count each of them twice.
    state: RunState,
    /// The tally over the whole store.
    tally: Tally,
    /// The byte every record read so far ends at: the marker, and the open instant
    /// past it.
    read_through: u64,
    /// How many lines end at [`read_through`](Self::read_through): what a line read
    /// past it is numbered after.
    lines_through: u64,
    /// The greatest `seq` each stream reached in the records past the marker —
    /// what a writer opening on this run takes its next `seq` above, beside the
    /// marker's own.
    claimed_past: BTreeMap<String, u64>,
    /// How many journal records the last fold took.
    ///
    /// Kept on the value rather than read back off the process-wide counter this
    /// also reports to, so a check can hold one fold to what the store grew by
    /// without a fold running beside it moving the number.
    took: u64,
    persist: Persist,
    /// What this driver's latest readjustment read, kept for the rest of its trigger.
    /// In memory only: a fresh driver reads again at its first trigger.
    pub(crate) readjust: crate::readjust::Readjuster,
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

/// How much of a journal a rebuild folds at a time.
///
/// The bound on what one read holds in memory, whatever the run's length. A window
/// is held parsed and merge-ordered, which takes several times its bytes: at 16 MiB
/// a driver adopting a run whose checkpoint an earlier build wrote peaked about
/// 100 MB above one resuming from its own (`tests/journal_workload`), so the window
/// is kept small enough that the rebuild's memory stays in the driver's budget. A
/// record is a few kilobytes and the largest this host's runs carry is under one
/// megabyte, so a window of this size holds whole records; one that holds none is
/// widened until it does, and a rebuild of a run of any length is one pass over it.
const REBUILD_WINDOW: u64 = 2 * 1024 * 1024;

impl Projected {
    /// Fold a run, resuming from its checkpoint where there is a usable one, and
    /// writing what it folds back.
    pub(crate) fn open(paths: &RunPaths) -> Self {
        Self::open_with(paths, Persist::Yes)
    }

    /// Fold a run, resuming from its checkpoint where there is a usable one.
    pub(crate) fn open_with(paths: &RunPaths, persist: Persist) -> Self {
        let resumed = readable(paths).filter(|checkpoint| {
            checkpoint.coverage.corroborated_by(
                &paths.journal(),
                &checkpoint.state,
                &checkpoint.tally,
            )
        });
        let mut projected = match resumed {
            Some(checkpoint) => Self {
                state: checkpoint.state.clone(),
                covered: checkpoint.state,
                tally: checkpoint.tally.clone(),
                covered_tally: checkpoint.tally,
                read_through: checkpoint.coverage.bytes,
                lines_through: checkpoint.coverage.records,
                coverage: checkpoint.coverage,
                claimed_past: BTreeMap::new(),
                took: 0,
                persist,
                readjust: crate::readjust::Readjuster::default(),
            },
            None => Self::empty(persist),
        };
        // Where there was no document to resume from, the journal is folded from
        // its first byte in windows, and the refresh below reads the last of them.
        let rebuilt = match projected.coverage.bytes {
            0 => projected.rebuild(paths),
            _ => 0,
        };
        projected.refresh(paths);
        projected.took += rebuilt;
        projected
    }

    /// Fold what the run's journal has grown by, leaving the state a fold of the
    /// whole store leaves.
    ///
    /// Reads from the marker and never in front of it: a record past it that the
    /// merge order would have placed in front of a covered one is folded after
    /// what is covered — see the module note. Only a journal **shorter** than the
    /// marker, which no longer holds what was folded, is rebuilt.
    pub(crate) fn refresh(&mut self, paths: &RunPaths) {
        let journal = paths.journal();
        let mut rebuilt = 0;
        if length_of(&journal) < self.coverage.bytes {
            // What this driver read and planned is not folded from the journal, so the
            // rebuild below cannot put it back: it is kept across it.
            let readjust = std::mem::take(&mut self.readjust);
            *self = Self::empty(self.persist);
            self.readjust = readjust;
            rebuilt = self.rebuild(paths);
        }
        let grown = journal::finished_lines_from(&journal, self.coverage.bytes);
        self.took = rebuilt + grown.len() as u64;
        self.take_with(paths, &grown, OpenInstant::HeldBack, Persist::Yes);
        // Not folded from the journal and therefore not part of what a fold of it
        // produces: `crate::crossdag` fills this in **after** a fold, from another
        // run's ledger, and a caller that had done so already must find it exactly
        // as empty here as it would after re-folding the whole store. Cleared
        // rather than left standing, so this state and a full fold's cannot
        // differ by an answer one of them happens to remember.
        self.state.cross_dag = BTreeMap::new();
    }

    /// Fold all but the last window of the journal, a window at a time, and say
    /// how many records that took.
    ///
    /// Every window this folds is covered whole — the windows are what bounds
    /// memory, and each one is a barrier exactly as the marker is for a resumed
    /// fold. The last window is left to [`refresh`](Self::refresh), which reads it
    /// as it reads any tail: its final instant held open, and the document written
    /// once.
    fn rebuild(&mut self, paths: &RunPaths) -> u64 {
        self.rebuild_in(paths, REBUILD_WINDOW)
    }

    /// [`rebuild`](Self::rebuild), a `window` of bytes at a time.
    fn rebuild_in(&mut self, paths: &RunPaths, window: u64) -> u64 {
        let journal = paths.journal();
        let mut window = window;
        let mut took = 0;
        loop {
            let length = length_of(&journal);
            let from = self.coverage.bytes;
            if window >= length.saturating_sub(from) {
                return took;
            }
            let grown = journal::finished_lines_within(&journal, from, window);
            if grown.is_empty() {
                // A window holding no finished record is one record larger than the
                // window, widened so it is read whole next time.
                window = window.saturating_mul(2);
                continue;
            }
            took += grown.len() as u64;
            self.take_with(paths, &grown, OpenInstant::Covered, Persist::No);
        }
    }

    pub(crate) fn into_state(self) -> RunState {
        self.state
    }

    /// The tally over the whole store, as [`Deref`](std::ops::Deref) gives the
    /// state.
    pub(crate) fn tally(&self) -> &Tally {
        &self.tally
    }

    /// The byte the marker stops at: everything in front of it is covered.
    pub(crate) fn covered_bytes(&self) -> u64 {
        self.coverage.bytes
    }

    /// The byte every record this fold has read ends at.
    pub(crate) fn read_through(&self) -> u64 {
        self.read_through
    }

    /// The greatest `seq` any record of `stream` claims, of every record this fold
    /// has read — what a writer opening on the run takes its next one above.
    pub(crate) fn seq_claimed(&self, stream: &str) -> Option<u64> {
        let covered = self.coverage.streams.get(stream).copied();
        let past = self.claimed_past.get(stream).copied();
        covered.max(past)
    }

    #[cfg(test)]
    pub(crate) fn took(&self) -> u64 {
        self.took
    }

    #[cfg(test)]
    pub(crate) fn coverage(&self) -> &Coverage {
        &self.coverage
    }

    /// The fold of a store with nothing in it, as [`projection::fold`] starts
    /// from.
    fn empty(persist: Persist) -> Self {
        let empty = RunState {
            strict: true,
            ..RunState::default()
        };
        Self {
            state: empty.clone(),
            covered: empty,
            tally: Tally::default(),
            covered_tally: Tally::default(),
            coverage: Coverage::default(),
            read_through: 0,
            lines_through: 0,
            claimed_past: BTreeMap::new(),
            took: 0,
            persist,
            readjust: crate::readjust::Readjuster::default(),
        }
    }

    /// Fold the records the store has grown by, and write what may be accounted
    /// for.
    ///
    /// Two folds, split where the marker will stop: the records it may account
    /// for go onto the covered state, and the ones past it onto a copy of it.
    /// Both runs are folded in [`journal::merge_order`].
    fn take_with(
        &mut self,
        paths: &RunPaths,
        grown: &[journal::FinishedLine],
        open: OpenInstant,
        write: Persist,
    ) {
        crate::loopstats::records_folded(grown.len() as u64);
        let accountable = extent(grown, open);
        let (covered, ahead) = grown.split_at(accountable);

        let first = self.coverage.records;
        fold_in_merge_order(
            paths,
            &mut self.covered,
            &mut self.covered_tally,
            covered,
            first,
        );
        for line in covered {
            self.coverage.absorb(line.record.as_ref(), line.bytes);
        }
        self.read_through = self.coverage.bytes + ahead.iter().map(|line| line.bytes).sum::<u64>();
        self.lines_through = self.coverage.records + ahead.len() as u64;
        self.claimed_past = BTreeMap::new();
        for event in ahead.iter().filter_map(|line| line.record.as_ref()) {
            let reached = self.claimed_past.entry(event.stream.clone()).or_default();
            *reached = (*reached).max(event.seq);
        }
        // Only where the marker actually moved. A store whose every record the
        // marker cannot be extended over would otherwise rewrite an unchanged
        // document on every read, and a reader that saved nothing would pay a
        // write to say so.
        if !covered.is_empty() {
            self.coverage.digest = self
                .coverage
                .sealed_with(&self.covered, &self.covered_tally);
            if self.persist == Persist::Yes && write == Persist::Yes {
                self.write(paths);
            }
        }
        self.state = self.covered.clone();
        self.tally = self.covered_tally.clone();
        let first = self.coverage.records;
        fold_in_merge_order(paths, &mut self.state, &mut self.tally, ahead, first);
    }

    /// Write the checkpoint.
    ///
    /// Best effort, and its failure is never reported — [`crate::summary`]'s own
    /// cache is written on the same terms and for the same reasons: a read-only
    /// runs root, or a directory this reader may not write, costs the next reader
    /// a fold and costs this one nothing. Written atomically, so a reader beside
    /// a writer sees one whole document or the one before it, and two writers
    /// racing leave a document each of them would have written.
    fn write(&self, paths: &RunPaths) {
        let _ = ledger::write_json(
            &paths.checkpoint(),
            &Checkpoint {
                schema_version: CHECKPOINT_SCHEMA_VERSION,
                run_id: paths.run.clone(),
                coverage: self.coverage.clone(),
                state: self.covered.clone(),
                tally: self.covered_tally.clone(),
            },
        );
    }
}

/// Fold records **onto a state that already accounts for everything in front of
/// them**, which is what lets one fold be split at the marker and resumed.
///
/// The sort belongs here rather than at the read: the file's order is not the
/// fold's, and a record this build cannot read drops out of the run entirely —
/// it folds to nothing, and placing it would need a timestamp it does not have.
///
/// `first` is how many lines are in front of `records`, which is what each loss
/// among them is numbered after.
fn fold_in_merge_order(
    paths: &RunPaths,
    state: &mut RunState,
    tally: &mut Tally,
    records: &[journal::FinishedLine],
    first: u64,
) {
    let mut ordered: Vec<Envelope> = records
        .iter()
        .filter_map(|line| line.record.clone())
        .collect();
    tally.unreadable += (records.len() - ordered.len()) as u64;
    for (at, line) in (first + 1..).zip(records) {
        if let Some(lost) = line.lost {
            tally.losses.push(LineLost {
                kind: lost.kind,
                line: at,
                offset: lost.offset,
                bytes: lost.bytes,
            });
        }
    }
    journal::merge_order(&mut ordered);
    for event in &ordered {
        projection::fold_one(state, event);
        tally.take(paths, event);
    }
}

/// The records of the rare kinds [`RunState::recalled`] keeps, off the run's
/// checkpoint and what was appended past it — what a driver starting on a run
/// reads them back from instead of its whole history.
pub(crate) fn recalled(paths: &RunPaths) -> Vec<Envelope> {
    Projected::open_with(paths, Persist::No).state.recalled
}

/// The greatest `seq` each stream reached across the whole store, off the run's
/// checkpoint and what was appended past it.
pub(crate) fn streams_reached(paths: &RunPaths) -> BTreeMap<String, u64> {
    let projected = Projected::open_with(paths, Persist::No);
    let mut reached = projected.coverage.streams.clone();
    for (stream, seq) in &projected.claimed_past {
        let held = reached.entry(stream.clone()).or_default();
        *held = (*held).max(*seq);
    }
    reached
}

/// A run's folded state off its checkpoint and what was appended past it, writing
/// nothing back: what a reader beside a driver that keeps the run's checkpoint
/// itself folds — another run's record, or a dependency's — once per question.
pub(crate) fn current(paths: &RunPaths) -> RunState {
    Projected::open_with(paths, Persist::No).into_state()
}

/// What a run's journal holds that is not a whole record — the
/// [`journal::Integrity`] a read of the whole file answers — off the losses its
/// checkpoint's fold already met and a read of only what lies past them.
///
/// The fold numbers each loss as it meets it, so every line in front of what it
/// has read is accounted for without being read again; what is past that is the
/// one record a writer may be in the middle of, and the fragments an append healed
/// out of the file are in the log kept beside it, as they always were.
pub(crate) fn integrity(paths: &RunPaths) -> journal::Integrity {
    let projected = Projected::open_with(paths, Persist::No);
    let journal = paths.journal();
    let mut integrity =
        journal::losses_from(&journal, projected.read_through, projected.lines_through);
    let mut read = journal::Integrity {
        healed: ledger::torn_tails(&journal),
        ..journal::Integrity::default()
    };
    for lost in &projected.tally.losses {
        read.count(
            usize::try_from(lost.line).unwrap_or(usize::MAX),
            Some(journal::Lost {
                kind: lost.kind,
                offset: lost.offset,
                bytes: lost.bytes,
            }),
        );
    }
    read.truncated.append(&mut integrity.truncated);
    read.unparseable.append(&mut integrity.unparseable);
    read
}

/// Fold a run from its checkpoint, and leave the checkpoint where this fold
/// reached.
///
/// Both halves are in the name because a caller gets both: a reader that folded
/// from a document and wrote none back would leave the next one paying exactly
/// what this one just paid, which is the whole of what this module is for.
pub(crate) fn fold_and_checkpoint(paths: &RunPaths) -> RunState {
    Projected::open(paths).into_state()
}

/// The byte a reader of what a run records **from now on** starts at: where the
/// run's checkpoint stops, once it has been brought up to the journal as it stands.
///
/// What a watch given no cursor, and a driver's attached stream, start their reads
/// at: everything in front of it is folded, so nothing they decide from is there,
/// and printing it was a read of the whole history per call.
pub(crate) fn resume_point(paths: &RunPaths) -> u64 {
    Projected::open(paths).coverage.bytes
}

/// The fold of a run **proved ended**, with its
/// checkpoint written covering every record — the trailing ones sharing the
/// journal's last timestamp included, which [`fold_and_checkpoint`] holds back.
///
/// The hold-back exists for a run still recording: its next arrival is stamped at
/// or after the open instant and may sort in front of a record there. A run whose
/// driver is proved gone, and whose every recorded process is over, has no next
/// arrival but an adoption, which is stamped later. So covering the final records
/// costs nothing a reader relies on, and it is what lets `onepipeline host` read an
/// ended run's final stop and running set off this document without opening the
/// journal.
pub(crate) fn fold_ended_and_checkpoint(paths: &RunPaths) -> RunState {
    let mut projected = Projected::open(paths);
    let rest = journal::finished_lines_from(&paths.journal(), projected.coverage.bytes);
    if !rest.is_empty() {
        projected.take_with(paths, &rest, OpenInstant::Covered, Persist::Yes);
    }
    projected.into_state()
}

/// An ended run's folded state off its checkpoint alone, where that covers
/// exactly the journal a **current** `summary.json` stamps — `journal_len` bytes
/// and `event_count` records — so the journal is not opened.
///
/// The document is held to its seal here as it is everywhere else: since version
/// 15 the seal is over the document alone, so checking it reads none of the
/// journal. A current summary (its journal length and modification time match the
/// journal's) is what says the journal has not moved since; a checkpoint whose
/// counts differ from it is refused, and the caller then folds through
/// [`fold_ended_and_checkpoint`], as every cold read does.
pub(crate) fn covering_whole(
    paths: &RunPaths,
    journal_len: u64,
    event_count: u64,
) -> Option<RunState> {
    readable(paths)
        .filter(|checkpoint| {
            checkpoint.coverage.bytes == journal_len
                && checkpoint.coverage.records == event_count
                && checkpoint
                    .coverage
                    .sealed_with(&checkpoint.state, &checkpoint.tally)
                    == checkpoint.coverage.digest
        })
        .map(|checkpoint| checkpoint.state)
}

/// Whether a fold may cover the records sharing the journal's last timestamp.
#[derive(Clone, Copy, PartialEq, Eq)]
enum OpenInstant {
    /// Held back, for a run that may still record.
    HeldBack,
    /// Covered, for a run proved ended. See [`fold_ended_and_checkpoint`].
    Covered,
}

/// The checkpoint document, where the run root holds one **this build can read as
/// this run's**.
///
/// Every unusable condition in the module note but the last is decided here, and so
/// is a document naming another run — one copied between run roots. The last, whether
/// the marker still describes the journal, is [`Coverage::corroborated_by`]'s.
fn readable(paths: &RunPaths) -> Option<Checkpoint> {
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
/// Every one but the trailing records sharing the store's last timestamp, which
/// are held back as [`crate::summary`]'s writer holds the same run open and for
/// the same reason: the next arrival is stamped at or after that instant, so
/// leaving them uncovered is what lets an ordinary one sort in front of them in
/// the merge rather than land behind the barrier. A run proved ended has no next
/// arrival, and covers them all.
fn extent(grown: &[journal::FinishedLine], open: OpenInstant) -> usize {
    match open {
        OpenInstant::HeldBack => before_the_open_instant(grown),
        OpenInstant::Covered => grown.len(),
    }
}

/// How many of a store's records may be considered for a marker at all.
///
/// Everything but the trailing run of records carrying the last one's timestamp.
/// Counted from the end rather than by that timestamp, so a store some producer
/// stamped out of order does not hold an earlier instant open too — those records
/// are placed already, and taking them with the tail would move a boundary
/// nothing arriving next can reach.
fn before_the_open_instant(grown: &[journal::FinishedLine]) -> usize {
    let Some(last) = grown
        .iter()
        .rev()
        .find_map(|line| line.record.as_ref().map(|event| event.ts.clone()))
    else {
        // Nothing datable in the whole of it: no instant is open, because no
        // arrival can sort in front of a record that folds to nothing.
        return grown.len();
    };
    grown
        .iter()
        .rposition(|line| line.record.as_ref().is_some_and(|event| event.ts != last))
        .map_or(0, |before| before + 1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::{EventKind, Labels, Source, ENVELOPE_VERSION};
    use crate::journal::{Journal, PipelineKind};
    use crate::ledger::LaunchRecord;
    use crate::plan::{Goal, Node, Plan, PLAN_SCHEMA_VERSION};
    use crate::projection::StopState;
    use serde_json::{json, Value};
    use std::path::{Path, PathBuf};

    /// A scratch runs root of this journey's own.
    ///
    /// Named after the journey **and the moment it opened**: worktrees of this
    /// repository share one host's `/tmp`, so a name only the journey decides is one
    /// another invocation removes out from under this one — which reads as a store
    /// that lost records. Not the process id, which keys on a value this repository
    /// is separately taking out of its tests.
    fn scratch(name: &str) -> PathBuf {
        let opened = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |since| since.as_nanos());
        let root = std::env::temp_dir().join(format!("onepipeline-checkpoint-{name}-{opened}"));
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
            export_edges: Vec::new(),
        }
    }

    /// A run root with a launch record, as `start` leaves one.
    fn a_run(root: &Path, run: &str) -> RunPaths {
        let paths = RunPaths::under(root, run);
        paths.create().expect("the run directory");
        let record = LaunchRecord {
            run_id: run.to_string(),
            project: "plans:demo".into(),
            labels: Default::default(),
            group: None,
            project_origin: None,
            dir: PathBuf::from("/tmp/launch"),
            graph: String::new(),
            graph_run: String::new(),
            observer_runs: Vec::new(),
            observer_ending: String::new(),
            node_graph: "graph".into(),
            pr_author_graph: String::new(),
            review_reply_graph: String::new(),
            node_validator: String::new(),
            envelope_reviewer: String::new(),
            launcher: "checkpoint-journey".into(),
            session: "a-session".into(),
            pid: 0,
            host: String::new(),
            started: String::new(),
            started_at: crate::sys::now_rfc3339(),
            heartbeat_interval: 1_800,
            writeback_item_budget: 0,
            success_hook: String::new(),
            failure_hook: String::new(),
            hook_timeout: 0,
            dispatch_env_hook: String::new(),
            dispatch_env_hook_timeout: 0,
            dag_sets: Vec::new(),
            node_sets: Vec::new(),
            adoptions: 0,
            filters: crate::filter::Filters::default(),
            bus_config: Default::default(),
            maintenance_config: None,
            branch_template: String::new(),
            template_root: String::new(),
            require_rendered: false,
            oneharness_sessions: None,
            flow: String::new(),
            envelope_reviewer_bar: Default::default(),
            public_id: String::new(),
            name_withheld: false,
        };
        crate::ledger::write_json(&paths.launch(), &record).expect("a launch record");
        paths
    }

    /// Let the clock reach the next millisecond.
    ///
    /// A run records over time and a fixture records as fast as the host will
    /// take it, so without this every record a journey writes can carry one
    /// stamp — and a store that is a single instant is one a marker holds
    /// entirely open, which is a different shape from the run these journeys are
    /// about. Two milliseconds rather than one, because the stamp is truncated to
    /// the millisecond.
    fn an_instant_later() {
        std::thread::sleep(std::time::Duration::from_millis(2));
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
        an_instant_later();
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
        an_instant_later();
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
            dimensions: Default::default(),
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
    fn without_a_checkpoint(paths: &RunPaths) -> Value {
        let held = std::fs::read(paths.checkpoint()).ok();
        let _ = std::fs::remove_file(paths.checkpoint());
        let whole = folded_as(&fold_and_checkpoint(paths));
        match held {
            Some(bytes) => std::fs::write(paths.checkpoint(), bytes).expect("put back"),
            None => {
                let _ = std::fs::remove_file(paths.checkpoint());
            }
        }
        whole
    }

    /// Seal a document as a writer would have sealed it, and write it.
    ///
    /// What lets a journey plant a document whose *content* the journal does not
    /// support while leaving it one a reader accepts — which is the only way to
    /// observe which records that reader consumed. The seal is taken over the
    /// document **as parsed**, because that is what a reader digests — so a document
    /// this build cannot parse is one no seal can be taken over at all.
    fn seal_and_write(paths: &RunPaths, document: &mut Value) {
        let parsed: Checkpoint =
            serde_json::from_value(document.clone()).expect("a document this build reads");
        let sealed = parsed.coverage.sealed_with(&parsed.state, &parsed.tally);
        document["coverage"]["digest"] = json!(format!("{sealed:032x}"));
        crate::ledger::write_json(&paths.checkpoint(), document).expect("written");
    }

    fn document(paths: &RunPaths) -> Value {
        crate::ledger::read_json_opt(&paths.checkpoint()).expect("the checkpoint this run carries")
    }

    /// One fold of a run, and how many journal records it took.
    ///
    /// The count comes off the fold itself rather than off the process-wide
    /// counter it also reports to: this repository runs a test per process, and
    /// a check that only holds where that is true is a check that stops holding
    /// the day it is not.
    fn folding(paths: &RunPaths) -> (Value, u64) {
        let projected = Projected::open(paths);
        (folded_as(&projected), projected.took())
    }

    /// A run stopped after dispatching `build`, through the real writer: the
    /// `run-stopped` is the store's last record, as on every registry-less run
    /// a long-ended engine left behind.
    fn a_stopped_run(root: &Path, run: &str) -> RunPaths {
        let paths = a_recorded_run(root, run);
        an_instant_later();
        Journal::open(&paths)
            .emit(
                PipelineKind::RunStopped,
                crate::journal::labels(run, None),
                crate::journal::payload(&[
                    ("owner", json!("[mine]")),
                    ("forced", json!(false)),
                    (crate::journal::STOP_TEARDOWN, json!("signalled")),
                ]),
            )
            .expect("appended");
        paths
    }

    fn journal_len(paths: &RunPaths) -> u64 {
        std::fs::metadata(paths.journal()).expect("a journal").len()
    }

    /// An ended run's checkpoint covers its final stop, so a reader can take
    /// the stop and the running set off it without opening the journal; and a
    /// reader that holds the open instant back — every older build — reads it,
    /// agrees, and leaves it byte for byte as it was.
    #[test]
    fn an_ended_runs_checkpoint_covers_its_final_records_and_a_hold_back_reader_leaves_it() {
        let root = scratch("ended-covers-all");
        let paths = a_stopped_run(&root, "r-ended");
        let len = journal_len(&paths);
        let records = crate::ledger::read_records(&paths.journal()).len() as u64;

        // The reader every build runs today holds the stop back.
        let _ = fold_and_checkpoint(&paths);
        let held = document(&paths);
        assert!(held["coverage"]["bytes"].as_u64() < Some(len));
        assert_eq!(held["state"]["stop"], json!("not-stopped"));
        assert!(covering_whole(&paths, len, records).is_none());

        let ended = folded_as(&fold_ended_and_checkpoint(&paths));
        let covered = document(&paths);
        assert_eq!(covered["coverage"]["bytes"].as_u64(), Some(len));
        assert_eq!(covered["state"]["stop"], json!("workers-signalled"));
        let whole = covering_whole(&paths, len, records).expect("the whole journal is covered");
        assert_eq!(whole.stop, StopState::WorkersSignalled);
        assert_eq!(folded_as(&whole), ended);
        assert_eq!(ended, without_a_checkpoint(&paths));

        let written = std::fs::read(paths.checkpoint()).expect("the document");
        let (reread, took) = folding(&paths);
        assert_eq!(
            took, 0,
            "a hold-back reader folded records the marker covers"
        );
        assert_eq!(reread, ended);
        assert_eq!(
            std::fs::read(paths.checkpoint()).expect("the document"),
            written,
            "a hold-back reader rewrote a checkpoint covering the whole journal"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// What a later append does to a checkpoint covering an ended run's
    /// final records. An adoption is stamped after them and folds onto the
    /// document, clearing the stop; a record stamped at the covered instant on a
    /// stream sorting in front of the marker's discards the document and the whole
    /// store is folded — the same answer either way.
    #[test]
    fn a_record_appended_past_an_extended_checkpoint_folds_to_the_whole_store() {
        let root = scratch("ended-then-appended");
        let paths = a_stopped_run(&root, "r-appended");
        let _ = fold_ended_and_checkpoint(&paths);
        let len = journal_len(&paths);
        an_instant_later();
        Journal::open(&paths)
            .emit(
                PipelineKind::DriverAdopted,
                crate::journal::labels(&paths.run, None),
                crate::journal::payload(&[("adoption", json!(1)), ("pid", json!(0))]),
            )
            .expect("appended");
        let (adopted, took) = folding(&paths);
        assert_eq!(
            took, 1,
            "the adoption was not folded onto the covered state"
        );
        assert_eq!(adopted["stop"], json!("not-stopped"));
        assert_eq!(adopted, without_a_checkpoint(&paths));
        // What a summary current for the grown journal asks for is not what the
        // document covers, so an ended run's verdict is not read off it.
        assert!(len < journal_len(&paths));
        assert!(covering_whole(&paths, journal_len(&paths), 4).is_none());

        // Covered again, then a record at the covered instant that sorts in front.
        let _ = fold_ended_and_checkpoint(&paths);
        let last = crate::ledger::read_records(&paths.journal())
            .last()
            .and_then(|record| serde_json::from_str::<Value>(&record.text).ok())
            .and_then(|record| record["ts"].as_str().map(str::to_owned))
            .expect("a stamped record");
        relayed(&paths, "!-sorts-first", 0, &last, "build");
        let resumed = folded_as(&fold_and_checkpoint(&paths));
        assert_eq!(resumed, without_a_checkpoint(&paths));
        let _ = std::fs::remove_dir_all(&root);
    }

    /// The point of the document: the state is the state a full fold produces.
    #[test]
    fn a_resumed_fold_lands_on_the_state_the_whole_store_folds_to() {
        let root = scratch("resumed-is-whole");
        let paths = a_recorded_run(&root, "r-resumed");
        // One read writes the checkpoint; the store then grows past it.
        let _ = fold_and_checkpoint(&paths);
        settle(&paths, "build", "done");
        relayed(&paths, "graph-1", 0, "2099-01-01T00:00:01.000Z", "ship");
        settle(&paths, "ship", "done");
        assert!(paths.checkpoint().is_file(), "no checkpoint was written");

        let resumed = folded_as(&fold_and_checkpoint(&paths));
        assert_eq!(resumed, without_a_checkpoint(&paths));
        let _ = std::fs::remove_dir_all(&root);
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
        let _ = fold_and_checkpoint(&paths);

        let held = crate::ledger::read_records(&paths.journal()).len() as u64;
        let stored = super::readable(&paths).expect("the checkpoint this read wrote");
        let covered = stored.coverage.records;
        assert!(
            covered > 0 && covered < held,
            "a marker over a {held}-record store accounts for {covered}"
        );
        let mut planted = document(&paths);
        planted["state"]["outcomes"]["build"] = json!("carried-from-the-checkpoint");
        seal_and_write(&paths, &mut planted);

        settle(&paths, "ship", "done");
        let grew_to = crate::ledger::read_records(&paths.journal()).len() as u64;
        let (resumed, took) = folding(&paths);
        assert_eq!(
            resumed["outcomes"]["build"], "carried-from-the-checkpoint",
            "the records the marker accounts for were folded again: {resumed}"
        );
        assert_eq!(
            resumed["recorded"]["ship"],
            json!({"at": "done"}),
            "the records past the marker were not folded: {resumed}"
        );
        assert_eq!(
            took,
            grew_to - covered,
            "a resumed fold took more than the store grew by"
        );

        let poisoned = std::fs::read(paths.checkpoint()).expect("the poisoned checkpoint");
        std::fs::remove_file(paths.checkpoint()).expect("the checkpoint goes away");
        let (whole, took_whole) = folding(&paths);
        std::fs::write(paths.checkpoint(), poisoned).expect("put back");
        assert_eq!(
            whole["outcomes"].get("build"),
            None,
            "the control fold kept an account only the checkpoint carried"
        );
        assert_eq!(
            took_whole, grew_to,
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
        let _ = fold_and_checkpoint(&paths);
        let mut planted = document(&paths);
        planted["state"]["outcomes"]["build"] = json!("carried-from-the-checkpoint");
        seal_and_write(&paths, &mut planted);
        settle(&paths, "ship", "done");
        let whole = without_a_checkpoint(&paths);
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
        // The version alone: the seal does not cover it, so this document is refused
        // for the one thing this journey is about.
        let mut later = document(&paths);
        later["schema_version"] = json!(CHECKPOINT_SCHEMA_VERSION + 1);
        crate::ledger::write_json(&paths.checkpoint(), &later).expect("written");
        assert_eq!(a_view_of(&paths), whole);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_marker_claiming_more_than_the_journal_holds_folds_the_whole_store() {
        let (root, paths, whole) = a_run_with_a_checkpoint("uncorroborated");
        // A marker claiming bytes the journal does not hold: a store cut back, or
        // replaced by a shorter one, holds none of what was folded. **Sealed as a
        // writer would have sealed it**, so what refuses it is the length rather
        // than the seal — which is the condition this journey is about.
        let mut longer = document(&paths);
        let held = std::fs::metadata(paths.journal()).expect("a journal").len();
        longer["coverage"]["bytes"] = json!(held + 4_096);
        seal_and_write(&paths, &mut longer);
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
        let _ = fold_and_checkpoint(&paths);
        let covered = super::readable(&paths).expect("a checkpoint").coverage;
        assert!(covered.records > 0, "nothing was accounted for");

        relayed(&paths, "graph-0", 0, "1999-01-01T00:00:00.000Z", "build");
        let resumed = folded_as(&fold_and_checkpoint(&paths));
        assert_eq!(resumed, without_a_checkpoint(&paths));
        let _ = std::fs::remove_dir_all(&root);
    }

    /// **A record arriving behind what is covered is folded after it, and nothing
    /// covered is read to place it.**
    ///
    /// The covered bytes are overwritten with bytes that are no record at all, so a
    /// reader that went back over them — to digest them, or to re-sort the store
    /// around the late arrival — would fold a store with nothing in it. The record
    /// appended is stamped a century before everything covered, on a stream of its
    /// own: the merge order would put it first, and the fold puts it after the
    /// covered history instead.
    #[test]
    fn a_record_arriving_behind_what_is_covered_is_folded_without_reading_what_is_covered() {
        let root = scratch("behind-the-barrier");
        let paths = a_recorded_run(&root, "r-barrier");
        settle(&paths, "build", "done");
        let _ = fold_and_checkpoint(&paths);
        let covered = super::readable(&paths).expect("a checkpoint").coverage;
        assert!(covered.records > 0, "nothing was accounted for");

        let store = paths.journal();
        let mut held = std::fs::read(&store).expect("the run's journal");
        let span = usize::try_from(covered.bytes).expect("a length");
        held[..span].fill(b'#');
        std::fs::write(&store, &held).expect("the covered bytes are overwritten");
        relayed(&paths, "graph-0", 0, "1999-01-01T00:00:00.000Z", "build");

        let mut projected = Projected::open(&paths);
        let past = journal::finished_records_from(&store, covered.bytes).len() as u64;
        assert_eq!(
            projected.took(),
            past,
            "the fold took records in front of the marker"
        );
        assert_eq!(
            projected.statuses().get("build"),
            Some(&crate::graph::NodeStatus::Done),
            "the covered settlement is not in the fold"
        );
        let build = projected
            .activity
            .get("build")
            .and_then(|activity| activity.progress)
            .expect("the late record folded as the node's activity");
        assert_eq!(build.events(), 1);
        // And it stays folded once: the next refresh reads nothing in front of the
        // marker and folds the record no second time.
        projected.refresh(&paths);
        assert!(projected.took() <= past);
        assert_eq!(
            projected
                .activity
                .get("build")
                .and_then(|activity| activity.progress)
                .map(crate::projection::Progress::events),
            Some(1)
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A marker passes a **local** inversion rather than stopping at it.
    ///
    /// The property the whole scheme turns on, and the one a marker that had to
    /// keep the file's own order does not have: a record the merge moves in front
    /// of the one appended before it rules out the boundaries *between the two*
    /// and no others, so the marker sits past both. Measured on a real run, the
    /// difference between the two readings was a marker that stopped at 6 records
    /// of a store and one that reached 18.
    #[test]
    fn a_marker_passes_a_local_inversion_rather_than_stopping_at_it() {
        let root = scratch("past-an-inversion");
        let paths = a_run(&root, "r-inverted");
        for (stream, ts) in [
            ("graph-a", "2099-01-01T00:00:01.000Z"),
            ("graph-c", "2099-01-01T00:00:05.000Z"),
            // Stamped between the two in front of it and appended after them,
            // which is what a relay carrying another process's stream does.
            ("graph-b", "2099-01-01T00:00:03.000Z"),
            ("graph-d", "2099-01-01T00:00:09.000Z"),
            ("graph-e", "2099-01-01T00:00:11.000Z"),
        ] {
            relayed(&paths, stream, 0, ts, "build");
        }
        let state = folded_as(&fold_and_checkpoint(&paths));
        let covered = super::readable(&paths)
            .expect("a checkpoint")
            .coverage
            .records;
        assert_eq!(
            covered, 4,
            "a marker stopped at an inversion instead of passing it"
        );
        assert_eq!(state, without_a_checkpoint(&paths));
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A line this build cannot read is accounted for by the marker and placed by
    /// nothing.
    ///
    /// Both halves matter and they pull opposite ways: the byte count has to
    /// include it, or the boundary is short for ever and every later read
    /// re-reads the store from there; and where the merge would put it must
    /// decide nothing, because it folds to nothing and holding the records after
    /// it against a line with no stamp of its own would stop a marker that has
    /// lost no accuracy at all.
    #[test]
    fn a_line_this_build_cannot_read_is_accounted_for_and_placed_by_nothing() {
        let root = scratch("unreadable-line");
        let paths = a_recorded_run(&root, "r-unreadable");
        an_instant_later();
        crate::ledger::append_line(&paths.journal(), "this is not a record").expect("appended");
        settle(&paths, "build", "done");
        settle(&paths, "ship", "done");

        let held = crate::ledger::read_records(&paths.journal()).len() as u64;
        let state = folded_as(&fold_and_checkpoint(&paths));
        let covered = super::readable(&paths).expect("a checkpoint").coverage;
        assert_eq!(
            covered.bytes,
            crate::ledger::read_records(&paths.journal())
                .iter()
                .take(covered.records as usize)
                .map(|record| record.bytes + 1)
                .sum::<u64>(),
            "the marker's bytes and the records it counts describe different stores"
        );
        assert!(
            covered.records > 2 && covered.records < held,
            "a {held}-record store with a line this build cannot read accounts for \
             {}",
            covered.records
        );
        assert_eq!(state, without_a_checkpoint(&paths));
        let _ = std::fs::remove_dir_all(&root);
    }

    /// **What a run's store lost is said off the fold that met it**, the same as a
    /// read of the whole file says it.
    ///
    /// Every class [`journal::integrity`] tells apart — a blank line it passes
    /// over, a line this build cannot read, a fragment with a record glued after
    /// it, and a record whose writer has not finished it — each numbered and
    /// placed where the file holds it: in front of the marker, past it, and in the
    /// unfinished tail a fold never reads as a record. And then again once the
    /// covered bytes are overwritten, which only a reader that never goes back over
    /// them still answers.
    #[test]
    fn what_a_store_lost_is_read_off_the_fold_as_a_whole_read_says_it() {
        let root = scratch("losses");
        let paths = a_recorded_run(&root, "r-losses");
        let store = paths.journal();
        an_instant_later();
        crate::ledger::append_line(&store, "").expect("appended");
        crate::ledger::append_line(&store, "this is not a record").expect("appended");
        settle(&paths, "build", "done");
        let glued = crate::ledger::read_records(&store)
            .last()
            .map(|record| record.text.clone())
            .expect("a settlement");
        crate::ledger::append_line(&store, &format!("{{\"v\":1,\"torn\"{glued}"))
            .expect("appended");
        an_instant_later();
        settle(&paths, "ship", "done");
        let _ = fold_and_checkpoint(&paths);
        crate::ledger::append_line(&store, "past the marker").expect("appended");
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(&store)
            .expect("the journal");
        std::io::Write::write_all(&mut file, b"{\"v\":1,\"half").expect("a torn tail");

        let whole = journal::integrity(&store);
        assert_eq!(whole.unparseable.len(), 2, "{whole:?}");
        assert_eq!(whole.truncated.len(), 2, "{whole:?}");
        assert_eq!(integrity(&paths), whole);

        let covered = super::readable(&paths).expect("a checkpoint").coverage;
        let mut held = std::fs::read(&store).expect("the run's journal");
        held[..usize::try_from(covered.bytes).expect("a length")].fill(b'#');
        std::fs::write(&store, &held).expect("the covered bytes are overwritten");
        assert_eq!(integrity(&paths).phrase(), whole.phrase());
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Every value the state carries goes back through the check that made it,
    /// rather than straight into the field.
    ///
    /// A checkpoint is a file, so a value in it arrives from outside exactly as a
    /// stream's record does — and one this crate would have refused off a stream is
    /// refused off a document, which is one more full fold. A park stating a reason
    /// that says nothing and a session handle that is no handle are both that.
    #[test]
    fn a_state_read_back_goes_through_the_checks_that_made_it() {
        let root = scratch("checked-on-the-way-back");
        let paths = a_recorded_run(&root, "r-checked");
        settle(&paths, "build", "done");
        let _ = fold_and_checkpoint(&paths);
        let whole = without_a_checkpoint(&paths);

        for (planting, named) in [
            (
                json!({"parks": {"build": {"by": "planner", "reason": "   "}}}),
                "reason",
            ),
            (
                json!({"sessions": {"build": {"token": "../somewhere-else", "branch": "work"}}}),
                "session handle",
            ),
        ] {
            let mut planted = document(&paths);
            for (field, value) in planting.as_object().expect("one field to plant") {
                planted["state"][field] = value.clone();
            }
            // Refused where the document is **parsed**, which is in front of the
            // seal — the seal is taken over the state as parsed, so a value that
            // never parses is one no document could have been sealed with. That is
            // what makes this the check the planting is about rather than a marker
            // the journal stopped corroborating when the value was planted.
            let refusal = serde_json::from_value::<Checkpoint>(planted.clone())
                .expect_err("a value this crate refuses off a stream is refused off a document");
            assert!(
                refusal.to_string().contains(named),
                "the refusal of {planting} does not name what it refused: {refusal}"
            );
            // And through the real reader: one more unusable checkpoint, folding
            // the state the whole store folds to.
            crate::ledger::write_json(&paths.checkpoint(), &planted).expect("written");
            assert_eq!(
                folded_as(&fold_and_checkpoint(&paths)),
                whole,
                "a document carrying {planting} was folded from"
            );
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A store this build can read **nothing** in holds no instant open: there is
    /// no arrival that could sort in front of a record that folds to nothing, so
    /// every line of it is accounted for.
    #[test]
    fn a_store_with_nothing_readable_in_it_holds_no_instant_open() {
        let root = scratch("nothing-readable");
        let paths = a_run(&root, "r-illegible");
        for line in ["not a record", "nor is this"] {
            crate::ledger::append_line(&paths.journal(), line).expect("appended");
        }
        let state = folded_as(&fold_and_checkpoint(&paths));
        let covered = super::readable(&paths).expect("a checkpoint").coverage;
        assert_eq!(
            covered.records, 2,
            "a line the fold skipped was left unaccounted"
        );
        assert_eq!(covered.at, None, "a line this build cannot read was placed");
        assert_eq!(state, without_a_checkpoint(&paths));
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A rebuild folded a window at a time — windows far smaller than a record
    /// included, which it widens until one fits — lands on the state one read of
    /// the whole store folds to, and covers every record but the last instant.
    #[test]
    fn a_rebuild_in_windows_lands_on_the_whole_stores_fold() {
        let root = scratch("windows");
        let paths = a_recorded_run(&root, "r-windows");
        settle(&paths, "build", "done");
        settle(&paths, "ship", "done");
        let _ = std::fs::remove_file(paths.checkpoint());
        let whole = folded_as(&Projected::open_with(&paths, Persist::No).state);
        for window in [1, 64, 4_096] {
            let mut rebuilt = Projected::empty(Persist::No);
            let took = rebuilt.rebuild_in(&paths, window);
            // A window as long as the store leaves all of it to the refresh below.
            assert_eq!(took > 0, window < 4_096, "window {window} took {took}");
            rebuilt.refresh(&paths);
            assert_eq!(folded_as(&rebuilt.state), whole, "window {window}");
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    /// **A covered record rewritten in place keeps the checkpoint's account of it.**
    ///
    /// The journal is append-only, and what the marker covers is history no reader
    /// goes back over: the document planted here says something the journal does
    /// not, and a reader that resumed from it — rather than re-reading the prefix
    /// to check it — is the one that reports what the document says. A whole-store
    /// fold, with no document to resume from, reads the rewritten bytes and reports
    /// what they say instead.
    #[test]
    fn a_covered_record_changed_in_place_is_never_read_again() {
        let root = scratch("prefix-changed");
        let paths = a_recorded_run(&root, "r-changed");
        settle(&paths, "build", "done");
        let _ = fold_and_checkpoint(&paths);
        let mut planted = document(&paths);
        planted["state"]["outcomes"]["build"] = json!("carried-from-the-checkpoint");
        seal_and_write(&paths, &mut planted);
        settle(&paths, "ship", "done");

        let store = paths.journal();
        let held = std::fs::read(&store).expect("the run's journal");
        let mut lines: Vec<Vec<u8>> = held
            .split_inclusive(|byte| *byte == b'\n')
            .map(<[u8]>::to_vec)
            .collect();
        let changed = lines[0].len() - 1;
        lines[0] = b"#".repeat(changed).into_iter().chain(*b"\n").collect();
        std::fs::write(&store, lines.concat()).expect("the journal is rewritten");

        let read = folded_as(&fold_and_checkpoint(&paths));
        assert_eq!(
            read["outcomes"]["build"],
            json!("carried-from-the-checkpoint"),
            "the reader went back over the covered history: {read}"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// The checked-in shape of a document at [`CHECKPOINT_SCHEMA_VERSION`].
    ///
    /// Read rather than restated, on [`crate::summary`]'s terms: this is the wire a
    /// later build of this crate parses, and the only thing that stops a marker
    /// field being renamed, an absence becoming a zero, the digest turning back
    /// into a number, or the version moving without anyone deciding to move it.
    const GOLDEN: &str = include_str!("../tests/golden/checkpoint-v15.json");

    /// The document the golden pins, built through the types.
    ///
    /// A marker over a real prefix — a byte count, a record count, both ordering
    /// maxima and the digest — beside a state carrying one node's settlement, which
    /// is the smallest document that exercises every part the reader decides on.
    fn a_checkpoint() -> Checkpoint {
        let mut graph = crate::graph::Graph::with_concurrency(4);
        graph.insert(Node {
            id: "build".into(),
            persona: Some("engineer".into()),
            task: Some("## What\ndo it".into()),
            ..Node::default()
        });
        let state = RunState {
            graph,
            run_node_sets: Some(vec!["members.worker.agent.model=edited".into()]),
            recorded: BTreeMap::from([(
                "build".to_string(),
                crate::projection::Recorded::At(crate::graph::NodeStatus::Done),
            )]),
            // Written only when a settle stated a change request, and pinned here so
            // the name it is written under cannot move without the version.
            stated_change_urls: BTreeMap::from([(
                "build".to_string(),
                crate::edits::StatedLanding::parse("https://example.invalid/owner/engine/pull/12")
                    .expect("a change request"),
            )]),
            last_write_at: Some(1_786_000_000_000),
            // Written only for a run driven under the closure rule, and pinned
            // here so the names it is written under cannot move without the
            // version.
            closure: crate::projection::Closure {
                owed_until_closed: Some(crate::payload::Owed),
                completion_requested: true,
                reopened_at: Some(1_785_999_000_000),
            },
            // Written only for a lifecycle node and for a run whose driver retired
            // a branch, and pinned here so the names they are written under cannot
            // move without the version.
            bases: BTreeMap::from([("build".to_string(), "main".to_string())]),
            retired_branches: std::collections::BTreeSet::from(["retired/build".to_string()]),
            // Written only for a node that kept its branch, and pinned for the same
            // reason.
            remotes: BTreeMap::from([("kept".to_string(), onevcs::Preservation::Pushed)]),
            // What a view, a note and the write-back read off the fold rather than
            // the run's history, pinned for the same reason.
            noted: std::collections::BTreeSet::from(["build".to_string()]),
            last_turns: BTreeMap::from([(
                "build".to_string(),
                crate::agentgraph::TurnAddress::of("graph-run-1", "worker").expect("an address"),
            )]),
            dispatched_nodes: std::collections::BTreeSet::from(["build".to_string()]),
            settled_details: BTreeMap::from([(
                "build".to_string(),
                "awaiting the engine's release".to_string(),
            )]),
            settled_awaiting: BTreeMap::from([(
                "build".to_string(),
                vec![crate::views::AwaitedRelease {
                    producer: "github.com/owner/engine".to_string(),
                    target: "crate".to_string(),
                }],
            )]),
            sessions_opened: BTreeMap::from([(
                "build".to_string(),
                crate::projection::SessionOpened {
                    token: Some(onevcs::SessionToken("session-1".into())),
                    base: crate::vcs::BranchName::checked("main").expect("a branch"),
                    at: "2026-09-08T11:00:00.000Z".into(),
                },
            )]),
            board_settlements: BTreeMap::from([(
                "build".to_string(),
                serde_json::Map::from_iter([("status".to_string(), json!("done"))]),
            )]),
            settled_clean: std::collections::BTreeSet::from(["ship".to_string()]),
            hook_marker: Some(crate::hooks::FiredOver::Live),
            // Written only for a node whose last activity referenced its history,
            // and pinned for the same reason.
            activity: BTreeMap::from([(
                "build".to_string(),
                crate::projection::NodeActivity {
                    doing: Some(crate::projection::Doing::Referenced(
                        crate::projection::ActivityReference::new(
                            serde_json::from_value(json!({
                                "history_dir": "/state/oneharness/history",
                                "history_project": "project",
                                "history_session": "session",
                                "history_id": "01a12345-6789-7000-8000-0123456789ab",
                                "index": 0,
                                "tool_call_id": "call-1",
                            }))
                            .expect("a reference"),
                        ),
                    )),
                    ..crate::projection::NodeActivity::default()
                },
            )]),
            strict: true,
            ..RunState::default()
        };
        // Sealed as a writer seals it, so the golden is a document a reader would
        // accept over a journal whose covered bytes are those — rather than a shape
        // with a plausible number where its seal goes.
        let mut coverage = Coverage {
            bytes: 8_192,
            records: 42,
            at: Some(Placed {
                ts: "2026-09-08T12:00:00.000Z".into(),
                stream: "golden-host-1".into(),
            }),
            streams: BTreeMap::from([("golden-host-1".to_string(), 41)]),
            digest: 0,
        };
        // The summary's tally, pinned for the same reason: its names are the
        // document's as much as the state's are.
        let tally = Tally {
            aggregate: crate::telemetry::Aggregate::default(),
            events: 42,
            last_event_kind: Some("node-settled".into()),
            judged: std::collections::BTreeSet::from(["build".to_string()]),
            unreadable: 1,
            losses: vec![LineLost {
                kind: journal::LossKind::Unparseable,
                line: 7,
                offset: 1_024,
                bytes: 33,
            }],
        };
        coverage.digest = coverage.sealed_with(&state, &tally);
        Checkpoint {
            schema_version: CHECKPOINT_SCHEMA_VERSION,
            run_id: "golden".into(),
            coverage,
            state,
            tally,
        }
    }

    /// The documents the builds **before** each fold change wrote, kept exactly as
    /// those builds wrote them: the one before the park-ending fold, the one
    /// before the fold recorded a driver letting go of its run, the one before it
    /// read a settle's stated landing, the one before an adoption cleared the
    /// run's recorded stop, the one before it kept a stated change request past a
    /// later stated commit, the one before it kept the run-wide node overrides,
    /// the one before it kept the closure rule's facts, and the one before it kept
    /// each node's base and the branches its driver retired, the one before it
    /// kept what putting a kept branch on its origin found to do, the one before
    /// a reparent cleared a waiting approval's recorded wait, the one before a
    /// node's activity kept the `transcript` reference it recorded, and the one
    /// before a resumed reader read nothing the document covers.
    ///
    /// What proves each fold change is a version and not a quiet re-reading: each
    /// is a real document, byte-for-byte the shape this build writes, and the
    /// reader has to refuse it rather than resume from a fold it would not have
    /// computed.
    const GOLDEN_EARLIER: [(u32, &str); 14] = [
        (1, include_str!("../tests/golden/checkpoint-v1.json")),
        (2, include_str!("../tests/golden/checkpoint-v2.json")),
        (3, include_str!("../tests/golden/checkpoint-v3.json")),
        (4, include_str!("../tests/golden/checkpoint-v4.json")),
        (5, include_str!("../tests/golden/checkpoint-v5.json")),
        (6, include_str!("../tests/golden/checkpoint-v6.json")),
        (7, include_str!("../tests/golden/checkpoint-v7.json")),
        (8, include_str!("../tests/golden/checkpoint-v8.json")),
        (9, include_str!("../tests/golden/checkpoint-v9.json")),
        (10, include_str!("../tests/golden/checkpoint-v10.json")),
        (11, include_str!("../tests/golden/checkpoint-v11.json")),
        (12, include_str!("../tests/golden/checkpoint-v12.json")),
        (13, include_str!("../tests/golden/checkpoint-v13.json")),
        (14, include_str!("../tests/golden/checkpoint-v14.json")),
    ];

    #[test]
    fn the_documents_earlier_builds_wrote_are_refused_rather_than_read() {
        for (version, earlier) in GOLDEN_EARLIER {
            let refused = serde_json::from_str::<Checkpoint>(earlier).expect_err("it is refused");
            assert!(
                refused
                    .to_string()
                    .contains(&format!("schema_version {version}")),
                "the refusal does not name the version it met: {refused}"
            );
        }
    }

    #[test]
    fn a_schema_15_document_is_the_shape_the_golden_pins() {
        let rendered = serde_json::to_string_pretty(&a_checkpoint()).expect("it serialises");
        if std::env::var_os("ONEPIPELINE_WRITE_CHECKPOINT_GOLDEN").is_some() {
            std::fs::write(
                concat!(
                    env!("CARGO_MANIFEST_DIR"),
                    "/tests/golden/checkpoint-v15.json"
                ),
                format!("{rendered}\n"),
            )
            .expect("write checkpoint golden");
        }
        assert_eq!(
            rendered.trim(),
            GOLDEN.trim(),
            "the checkpoint document changed shape. If that was deliberate, bump \
             CHECKPOINT_SCHEMA_VERSION and update tests/golden/checkpoint-v15.json together"
        );
    }

    #[test]
    fn a_schema_15_document_round_trips_and_a_version_this_build_does_not_read_is_refused() {
        let read: Checkpoint =
            serde_json::from_str(GOLDEN).expect("the golden reads back into the types");
        // Compared through the wire rather than through `PartialEq`, which the
        // folded state does not carry: what has to survive is the document.
        assert_eq!(
            serde_json::to_string_pretty(&read).expect("it serialises"),
            GOLDEN.trim(),
            "a document this build wrote does not read back as itself"
        );
        assert_eq!(read.coverage, a_checkpoint().coverage);
        // The seal is a 128-bit value and the wire is hex, so this is the one field a
        // JSON number would have quietly rounded — and it seals over the marker's own
        // claims, so a claim read back differently would not seal to this.
        assert_eq!(
            read.coverage.digest,
            read.coverage.sealed_with(&read.state, &read.tally),
            "the document read back does not seal to what it carries"
        );

        let mut later: serde_json::Value = serde_json::from_str(GOLDEN).expect("it parses");
        later["schema_version"] = json!(CHECKPOINT_SCHEMA_VERSION + 1);
        let refused =
            serde_json::from_value::<Checkpoint>(later).expect_err("a later version is refused");
        assert!(
            refused.to_string().contains("schema_version"),
            "the refusal does not name what it refused: {refused}"
        );

        // And every digest that is not one this build could have written, which is a
        // document that cannot be corroborated at all. The upper-case and signed
        // spellings are here because they are the ones a hex parser takes and the
        // writer never emits, so a reader that accepted them would corroborate a
        // document from somewhere else.
        for refused in [
            "not a digest",
            // Upper case, a leading sign, and one digit short: the three a hex
            // parser takes and `as_hex` never writes.
            "ABCDEF01234567890123456789ABCDEF",
            "+bcdef01234567890123456789abcdef",
            "bcdef01234567890123456789abcdef",
        ] {
            let mut mangled: serde_json::Value = serde_json::from_str(GOLDEN).expect("it parses");
            mangled["coverage"]["digest"] = json!(refused);
            let refusal = serde_json::from_value::<Checkpoint>(mangled)
                .expect_err("a digest no writer here produced is refused");
            assert!(
                refusal.to_string().contains("digest"),
                "the refusal of '{refused}' does not name what it refused: {refusal}"
            );
        }
    }

    /// A populated session survives the document, through the checks that make one.
    ///
    /// The drift gate over `vcs::DispatchSession`'s two directions: they go through
    /// one declaration, and this is what fails if they ever stop — a document the
    /// writer produced that the reader refuses would take a run's sessions away
    /// without saying so.
    #[test]
    fn a_session_the_writer_produced_is_one_the_reader_accepts() {
        let written = json!({"token": "s-abc", "branch": "work/build"});
        let read: crate::vcs::DispatchSession =
            serde_json::from_value(written.clone()).expect("a session this crate accepts");
        assert_eq!(
            serde_json::to_value(&read).expect("it serialises"),
            written,
            "a session does not read back as the document it was written as"
        );
        // And the two values the checks are about, refused from a document exactly
        // as they are off a stream.
        for refused in [
            json!({"token": "../somewhere-else", "branch": "work"}),
            json!({"token": "s-abc", "branch": "   "}),
            json!({"token": "s-abc", "branch": "work", "extra": 1}),
        ] {
            assert!(
                serde_json::from_value::<crate::vcs::DispatchSession>(refused.clone()).is_err(),
                "{refused} was accepted as a session"
            );
        }
    }

    /// A park the writer produced is one the reader accepts, and the one state the
    /// type must not hold is refused.
    ///
    /// The drift gate over `edits::Park`'s two directions, as the session's is over
    /// `vcs::DispatchSession`'s: a park a run recorded that the reader refused would
    /// take a run's parks away without saying so, and a blank reason accepted would
    /// put back the state `Park::of` exists to keep out.
    #[test]
    fn a_park_the_writer_produced_is_one_the_reader_accepts() {
        let written = crate::edits::Park::of(crate::channel::Author::planner(), Some("waiting"));
        let document = serde_json::to_value(&written).expect("it serialises");
        assert_eq!(
            serde_json::from_value::<crate::edits::Park>(document.clone())
                .expect("a park this crate accepts"),
            written,
            "a park does not read back as the document it was written as"
        );
        // A park with no reason writes none rather than a null, and reads back as
        // the same park.
        let none = serde_json::to_value(crate::edits::Park::of(
            crate::channel::Author::planner(),
            None,
        ))
        .expect("it serialises");
        assert_eq!(none, json!({"by": "planner"}));
        for refused in [
            json!({"by": "planner", "reason": "   "}),
            json!({"by": "planner", "reason": ""}),
            json!({"by": "planner", "extra": 1}),
        ] {
            assert!(
                serde_json::from_value::<crate::edits::Park>(refused.clone()).is_err(),
                "{refused} was accepted as a park"
            );
        }
    }

    /// **A fold edited in the document, without the seal moving with it, takes a
    /// full fold.**
    ///
    /// The counterpart of a rewritten prefix, from the document's side: the journal
    /// is untouched and every count is what the writer wrote, so the seal over the
    /// state is the only thing that refuses it. Without this the cache could assert
    /// anything about a run and be believed.
    #[test]
    fn a_state_edited_without_the_seal_moving_folds_the_whole_store() {
        let root = scratch("state-edited");
        let paths = a_recorded_run(&root, "r-edited");
        settle(&paths, "build", "done");
        let _ = fold_and_checkpoint(&paths);
        settle(&paths, "ship", "done");
        let whole = without_a_checkpoint(&paths);

        // Written straight back, which is what an editor does — no seal taken.
        let mut edited = document(&paths);
        edited["state"]["outcomes"]["build"] = json!("carried-from-the-checkpoint");
        crate::ledger::write_json(&paths.checkpoint(), &edited).expect("written");

        let read = folded_as(&fold_and_checkpoint(&paths));
        assert_eq!(
            read.get("outcomes")
                .and_then(|outcomes| outcomes.get("build")),
            None,
            "a fold edited in the document was served: {read}"
        );
        assert_eq!(read, whole);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A run that has recorded nothing folds to the empty state and leaves no
    /// checkpoint: there is no prefix to cache and nothing to save by caching it.
    #[test]
    fn a_run_that_has_recorded_nothing_leaves_no_checkpoint() {
        let root = scratch("nothing-recorded");
        let paths = a_run(&root, "r-empty");
        let state = fold_and_checkpoint(&paths);
        assert!(state.strict, "the empty fold is not the fold's own start");
        assert!(state.graph.is_empty());
        assert!(
            !paths.checkpoint().exists(),
            "a run with no store was given a checkpoint of it"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A re-fold of the whole store, which a record the merge order moves in front of
    /// what the loop had folded forces, keeps what the driver read and planned: that is
    /// not folded from the journal, so nothing the re-fold takes in could put it back.
    #[test]
    fn a_whole_store_refold_keeps_the_drivers_readjustment() {
        let root = scratch("refold-keeps-readjust");
        let paths = a_recorded_run(&root, "r-refold-readjust");
        settle(&paths, "build", "done");
        let mut projected = Projected::open(&paths);
        projected
            .readjust
            .bodies
            .insert("build".into(), "a reviewer's note".into());
        projected.readjust.tried.insert("ship".into());

        relayed(&paths, "graph-0", 0, "1999-01-01T00:00:00.000Z", "build");
        projected.refresh(&paths);
        assert_eq!(
            folded_as(&projected),
            without_a_checkpoint(&paths),
            "the loop's state and a full fold's disagree"
        );
        assert_eq!(
            projected.readjust.bodies.get("build").map(String::as_str),
            Some("a reviewer's note"),
            "the re-fold forgot the description the driver read"
        );
        assert!(projected.readjust.tried.contains("ship"));
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
        let mut projected = Projected::open(&paths);
        assert_eq!(
            projected.took(),
            22,
            "the loop did not open on the whole store"
        );
        let covered = projected.coverage().records;
        assert!(
            covered > 0,
            "the loop accounted for none of the 22-record store it opened"
        );

        settle(&paths, "build", "failed");
        projected.refresh(&paths);
        let took = projected.took();
        // What the store grew by, plus the instant the marker was holding open —
        // bounded by one timestamp's records rather than by the run's length, and
        // the whole of what a re-fold pays twice.
        assert_eq!(
            took,
            23 - covered,
            "a re-fold took {took} records of a 23-record store"
        );
        assert!(took <= 4, "the open instant is not a bounded run: {took}");
        assert_eq!(
            folded_as(&projected),
            without_a_checkpoint(&paths),
            "the loop's state and a full fold's disagree"
        );
        let _ = std::fs::remove_dir_all(&root);
    }
}
