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
//! The fourth condition is asked of the **file**: it is at least as long as the
//! marker claims, the marker ends where a record ends, and the covered bytes
//! together with the marker's own claims seal to what the document carries. The
//! seal is what makes the journal outrank the cache — no count can see a covered
//! record rewritten in place at identical length — and it is
//! [`Coverage::corroborated_by`].
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
//! # One order, for both readers
//!
//! One document holding one fold means one order: [`journal::merge_order`], which
//! [`crate::views`], [`crate::summary`] and [`crate::telemetry`] already read the
//! store in and which is the only ordering promise an envelope carries. The
//! reconcile loop folded the file as appended before this; what moves is only where
//! a relayed record sits against its own, since the merge keeps a stream in its own
//! `seq` whatever the stamps say.

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
/// still holds the park is refolded rather than resumed from.
// llmlint: ignore[changed_behavior_has_e2e] a version-1 document is one the build before
// this one wrote, which no invocation of this build can produce: what a user can reach —
// a checkpoint at this version being resumed from, and one at a version this build does
// not write being refolded — is driven end to end in `tests/e2e/checkpoint.rs`. The
// refusal of the checked-in version-1 document itself is held by
// `the_document_the_build_before_this_one_wrote_is_refused_rather_than_read` below, over
// the real reader and the real file.
pub(crate) const CHECKPOINT_SCHEMA_VERSION: u32 = 2;

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
    /// **The seal: this marker, over the journal bytes it claims.**
    ///
    /// What makes the journal authoritative rather than the document. Every field
    /// above says something a reader would otherwise have to take on trust — how
    /// many bytes, how many records, how far the order reached — and none of them
    /// can see a covered record rewritten in place at identical length. This is
    /// FNV-1a over the covered bytes and then over those fields, so a reader
    /// recomputes it from the file plus the claims in front of it and refuses a
    /// prefix or a claim that has moved. See [`sealed_with`](Self::sealed_with).
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

/// The digest of a journal's first `bytes` bytes, or `None` where the file does not
/// hold them.
fn digest_of_prefix(journal: &std::path::Path, bytes: u64) -> Option<u128> {
    ledger::read_range(journal, 0, bytes).map(|prefix| digested(NOTHING_DIGESTED, &prefix))
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
    fn is_in_front_of(&self, event: &Envelope) -> bool {
        let placed = placed(event);
        self.at.as_ref().is_none_or(|at| *at <= placed)
            && self
                .streams
                .get(&event.stream)
                .is_none_or(|reached| *reached <= event.seq)
    }

    /// Whether the merge order puts every record this accounts for in front of
    /// **every** record the store has grown by since.
    ///
    /// One half of what the journal is asked about a marker; the other half is
    /// [`corroborated_by`](Self::corroborated_by), which
    /// asks whether the covered bytes are still the bytes this was folded from.
    fn sorts_in_front_of(&self, grown: &[(Option<Envelope>, u64)]) -> bool {
        grown
            .iter()
            .filter_map(|(event, _)| event.as_ref())
            .all(|event| self.is_in_front_of(event))
    }

    /// This marker's seal: the journal bytes it covers, then its own claims, then
    /// the state written beside it.
    ///
    /// Everything the document asserts goes in, in a fixed order, so **anything
    /// edited no longer seals to what the document carries** — a claim, or the fold
    /// itself. [`digest`](Self::digest) does not go in, which is what stops it
    /// sealing over itself.
    ///
    /// What that leaves is a document no part of which has moved since a writer
    /// sealed it, over a journal prefix that has not moved either. It is still not a proof that the state is the *fold* of those
    /// bytes — only folding them proves that, which is the cost this document
    /// removes — but there is no longer any part of the document a reader takes on
    /// trust separately from the rest.
    fn sealed_with(&self, digested_bytes: u128, state: &RunState) -> u128 {
        let mut sealed = digested(digested_bytes, &self.bytes.to_le_bytes());
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
        match serde_json::to_vec(state) {
            Ok(written) => digested(sealed, &written),
            // A state that will not serialize is one no writer could have sealed,
            // so nothing may match it.
            Err(_) => sealed.wrapping_add(1),
        }
    }

    /// Whether the journal corroborates every claim this marker makes.
    ///
    /// **What makes the journal authoritative.** Three questions of the file rather
    /// than of the document: it is at least as long as the marker claims, the marker
    /// ends where a record ends, and the covered bytes together with everything the
    /// document asserts seal to what it carries. The third is the one nothing else
    /// can answer — a covered record rewritten in place at the same length moves no
    /// count and no maximum — and it is why a truncated or edited prefix, an edited
    /// claim about one, or an edited fold takes a full fold.
    ///
    /// Asked where a marker arrives **from a document**, which is the boundary it is
    /// about. A marker this process established by folding those very bytes, under
    /// the run's single-writer lock over an append-only file, has no document to
    /// corroborate — and re-digesting the prefix per applied command would put back a
    /// cost that grows with the run.
    fn corroborated_by(&self, journal: &std::path::Path, state: &RunState) -> Option<u128> {
        if length_of(journal) < self.bytes {
            return None;
        }
        // A marker inside a line loses the record it lands in for every reader
        // afterwards. Asked of the byte in front of it, which is the terminator the
        // appender wrote.
        if self.bytes > 0 && !ends_a_record(journal, self.bytes) {
            return None;
        }
        digest_of_prefix(journal, self.bytes)
            .filter(|digested_bytes| self.sealed_with(*digested_bytes, state) == self.digest)
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
    coverage: Coverage,
    state: RunState,
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
    coverage: Coverage,
    /// That, with the records past the marker folded onto it: the whole store.
    ///
    /// Kept apart from the one above rather than derived from it on demand,
    /// because the records past the marker are re-folded on every pass — they are
    /// the store's own open instant, held back so an arrival beside them can still
    /// sort in front — and folding them onto a state that already carried them
    /// would count each of them twice.
    state: RunState,
    /// How many journal records the last fold took.
    ///
    /// Kept on the value rather than read back off the process-wide counter this
    /// also reports to, so a check can hold one fold to what the store grew by
    /// without a fold running beside it moving the number.
    took: u64,
    /// The digest of the covered bytes alone, which the marker's seal is taken over.
    ///
    /// In memory rather than on the wire: a reader that verified a seal read those
    /// bytes to do it and so has this already, and one that folded them has it by
    /// construction. Persisting it beside the seal would be one fact twice, with an
    /// edit to either making the pair disagree.
    digested_bytes: u128,
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
    pub(crate) fn open(paths: &RunPaths) -> Self {
        let resumed = readable(paths).and_then(|checkpoint| {
            let digested_bytes = checkpoint
                .coverage
                .corroborated_by(&paths.journal(), &checkpoint.state)?;
            Some((checkpoint, digested_bytes))
        });
        let mut projected = match resumed {
            Some((checkpoint, digested_bytes)) => Self {
                state: checkpoint.state.clone(),
                covered: checkpoint.state,
                coverage: checkpoint.coverage,
                took: 0,
                digested_bytes,
            },
            None => Self::empty(),
        };
        projected.refresh(paths);
        projected
    }

    /// Fold what the run's journal has grown by, leaving the state a fold of the
    /// whole store leaves.
    pub(crate) fn refresh(&mut self, paths: &RunPaths) {
        let journal = paths.journal();
        let mut grown = journal::finished_records_after(&journal, self.coverage.bytes);
        if length_of(&journal) < self.coverage.bytes || !self.coverage.sorts_in_front_of(&grown) {
            *self = Self::empty();
            grown = journal::finished_records_after(&journal, 0);
        }
        self.took = grown.len() as u64;
        self.take(paths, &grown);
        // Not folded from the journal and therefore not part of what a fold of it
        // produces: `crate::crossdag` fills this in **after** a fold, from another
        // run's ledger, and a caller that had done so already must find it exactly
        // as empty here as it would after re-folding the whole store. Cleared
        // rather than left standing, so this state and a full fold's cannot
        // differ by an answer one of them happens to remember.
        self.state.cross_dag = BTreeMap::new();
    }

    pub(crate) fn into_state(self) -> RunState {
        self.state
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
    fn empty() -> Self {
        let empty = RunState {
            strict: true,
            ..RunState::default()
        };
        Self {
            state: empty.clone(),
            covered: empty,
            coverage: Coverage::default(),
            took: 0,
            digested_bytes: NOTHING_DIGESTED,
        }
    }

    /// Fold the records the store has grown by, and write what may be accounted
    /// for.
    ///
    /// Two folds, split where the marker will stop: the records it may account
    /// for go onto the covered state, and the ones past it onto a copy of it.
    /// Both runs are folded in [`journal::merge_order`], and the split holds in
    /// that order as well as in the file's because a covered prefix is one
    /// nothing past it sorts in front of — which is the same statement the marker
    /// is chosen by.
    fn take(&mut self, paths: &RunPaths, grown: &[(Option<Envelope>, u64)]) {
        crate::loopstats::records_folded(grown.len() as u64);
        // Read **raw and before anything is folded**: the digest has to equal what a
        // later reader takes off the file rather than what these records decoded to,
        // and a marker whose seal did not cover what it claims is the one thing this
        // document must never carry. Where those bytes cannot be read the marker
        // takes in nothing this pass, which costs the next reader's saving and never
        // an answer.
        let accountable = extent(&self.coverage, grown);
        let taking: u64 = grown[..accountable].iter().map(|(_, bytes)| bytes).sum();
        let taken = match taking {
            0 => Some(Vec::new()),
            _ => ledger::read_range(&paths.journal(), self.coverage.bytes, taking),
        };
        let (covered, ahead) = grown.split_at(taken.as_ref().map_or(0, |_| accountable));

        fold_in_merge_order(&mut self.covered, covered);
        for (event, bytes) in covered {
            self.coverage.absorb(event.as_ref(), *bytes);
        }
        // Only where the marker actually moved. A store whose every record the
        // marker cannot be extended over would otherwise rewrite an unchanged
        // document on every read, and a reader that saved nothing would pay a
        // write to say so.
        if let Some(taken) = taken.filter(|taken| !taken.is_empty()) {
            self.digested_bytes = digested(self.digested_bytes, &taken);
            self.coverage.digest = self
                .coverage
                .sealed_with(self.digested_bytes, &self.covered);
            self.write(paths);
        }
        self.state = self.covered.clone();
        fold_in_merge_order(&mut self.state, ahead);
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
fn fold_in_merge_order(state: &mut RunState, records: &[(Option<Envelope>, u64)]) {
    let mut ordered: Vec<Envelope> = records
        .iter()
        .filter_map(|(event, _)| event.clone())
        .collect();
    journal::merge_order(&mut ordered);
    for event in &ordered {
        projection::fold_one(state, event);
    }
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

/// Whether a byte offset sits just past a record's own terminator.
///
/// The one thing a marker's byte count claims that a reader can check without
/// reading the prefix, and the claim everything else it says rests on: a tail read
/// from inside a line drops the record it lands in, and every reader afterwards
/// folds a store nobody wrote. A byte that cannot be read at all answers `false`,
/// which is one more unusable checkpoint.
fn ends_a_record(journal: &std::path::Path, at: u64) -> bool {
    use std::io::{Read, Seek, SeekFrom};
    let Ok(mut file) = std::fs::File::open(journal) else {
        return false;
    };
    if file.seek(SeekFrom::Start(at - 1)).is_err() {
        return false;
    }
    let mut byte = [0u8; 1];
    file.read_exact(&mut byte).is_ok() && byte[0] == b'\n'
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
/// The **largest** prefix nothing past it sorts in front of, which is the module
/// note's condition — largest rather than first, because a marker that stopped at
/// the first inversion never passes it.
///
/// Read as spans: a record sorting in front of an earlier one rules out the
/// boundaries *between* the two, so each contributes one span, found by binary
/// search against the running maxima in front of it, and the answer is the largest
/// boundary no span covers. Linear but for those searches, which the whole-store
/// fold needs.
///
/// The trailing records sharing the store's last timestamp are held back, as
/// [`crate::summary`]'s maintainer holds the same run open and for the same
/// reason: the next arrival is stamped at or after that instant, so leaving it
/// uncovered is what lets an ordinary one sort in front of something without
/// making the marker unusable.
fn extent(coverage: &Coverage, grown: &[(Option<Envelope>, u64)]) -> usize {
    let cap = before_the_open_instant(grown);
    // One more than the boundaries there are, so a span ending at the last of
    // them closes inside the array rather than off the end of it.
    let mut ruled_out = vec![0i64; cap + 2];
    let mut rule_out = |from: usize, to: usize| {
        if from <= cap {
            ruled_out[from] += 1;
            ruled_out[to.min(cap) + 1] -= 1;
        }
    };
    // The greatest `(ts, stream)` in front of each boundary, which only ever
    // grows — so the first record to reach past a given place is found by
    // searching this rather than by scanning back through the store.
    let mut reached: Vec<Option<Placed>> = vec![coverage.at.clone()];
    // Each stream's own records: where each sits in the store, and the greatest
    // `seq` that stream had reached there. Growing too, for the same reason, and
    // per stream because the merge keeps a stream in its own `seq` whatever the
    // stamps say.
    let mut per_stream: BTreeMap<&str, Vec<(usize, u64)>> = BTreeMap::new();
    for (at, (event, _)) in grown.iter().enumerate() {
        let front = reached[at].clone();
        let Some(event) = event else {
            // A record this build cannot read folds to nothing, so no boundary is
            // ruled out by where the merge would put it.
            reached.push(front);
            continue;
        };
        let placed = placed(event);
        if front.as_ref().is_some_and(|front| *front > placed) {
            // The merge puts this record in front of one appended before it, so
            // every boundary between the two is one it would be carried across.
            let first =
                reached.partition_point(|reached| reached.as_ref().is_none_or(|at| *at <= placed));
            rule_out(first, at);
        }
        let stream = per_stream.entry(event.stream.as_str()).or_default();
        if stream
            .last()
            .is_some_and(|(_, reached)| *reached > event.seq)
        {
            // The same statement about one stream's own order. The `seq` the
            // marker already carries for this stream cannot be the record in
            // front: a checkpoint is only usable where every record the store has
            // grown by is past it, which `corroborated` has already asked.
            let first = stream.partition_point(|(_, reached)| *reached <= event.seq);
            rule_out(stream[first].0 + 1, at);
        }
        let carried = stream.last().map_or(event.seq, |(_, reached)| *reached);
        stream.push((at, carried.max(event.seq)));
        reached.push(Some(match front {
            Some(front) if front > placed => front,
            _ => placed,
        }));
    }
    let mut covered = 0;
    let mut spanning = 0;
    for (boundary, opened) in ruled_out.iter().enumerate().take(cap + 1) {
        spanning += opened;
        if spanning == 0 {
            covered = boundary;
        }
    }
    covered
}

/// How many of a store's records may be considered for a marker at all.
///
/// Everything but the trailing run of records carrying the last one's timestamp.
/// Counted from the end rather than by that timestamp, so a store some producer
/// stamped out of order does not hold an earlier instant open too — those records
/// are placed already, and taking them with the tail would move a boundary
/// nothing arriving next can reach.
fn before_the_open_instant(grown: &[(Option<Envelope>, u64)]) -> usize {
    let Some(last) = grown
        .iter()
        .rev()
        .find_map(|(event, _)| event.as_ref().map(|event| event.ts.clone()))
    else {
        // Nothing datable in the whole of it: no instant is open, because no
        // arrival can sort in front of a record that folds to nothing.
        return grown.len();
    };
    grown
        .iter()
        .rposition(|(event, _)| event.as_ref().is_some_and(|event| event.ts != last))
        .map_or(0, |before| before + 1)
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
            writeback_item_budget: 0,
            dag_sets: Vec::new(),
            node_sets: Vec::new(),
            adoptions: 0,
            filters: crate::filter::Filters::default(),
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
        let bytes = digest_of_prefix(&paths.journal(), parsed.coverage.bytes)
            .expect("the journal holds the bytes the marker claims");
        let sealed = parsed.coverage.sealed_with(bytes, &parsed.state);
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
    fn a_coverage_the_journal_does_not_corroborate_folds_the_whole_store() {
        let (root, paths, whole) = a_run_with_a_checkpoint("uncorroborated");
        // A marker claiming to account for records the store in front of it does not
        // sort after: the settlement appended since is stamped now, and this says
        // everything covered was written a century later. **Sealed as a writer would
        // have sealed it**, so what refuses it is the ordering condition rather than
        // the seal — which is the condition this journey is about.
        let mut ahead = document(&paths);
        ahead["coverage"]["at"] = json!({"ts": "2199-01-01T00:00:00.000Z", "stream": "zzzz"});
        seal_and_write(&paths, &mut ahead);
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

    /// A store whose **first** records the merge order rearranges accounts for
    /// nothing rather than for something it cannot place.
    #[test]
    fn a_marker_is_never_extended_over_a_record_a_reordering_would_move() {
        let root = scratch("never-extended");
        let paths = a_run(&root, "r-unplaceable");
        relayed(&paths, "graph-b", 0, "2099-01-01T00:00:03.000Z", "build");
        relayed(&paths, "graph-a", 0, "2099-01-01T00:00:01.000Z", "build");
        let state = folded_as(&fold_and_checkpoint(&paths));
        let covered = super::readable(&paths).map(|stored| stored.coverage);
        assert!(
            covered.is_none_or(|covered| covered.records == 0),
            "a record the merge order moves was accounted for"
        );
        assert_eq!(state, without_a_checkpoint(&paths));
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

    /// **A covered record rewritten in place at identical length takes a full
    /// fold.**
    ///
    /// The one modification no count can see: the byte count, the record count and
    /// both ordering maxima are exactly what they were, so the digest is the only
    /// thing that refuses it — and it has to, or a reader serves a state folded
    /// from bytes the file no longer holds. The checkpoint carries an account of
    /// its covered records the journal does not support, so a document that *was*
    /// accepted would be visible rather than indistinguishable.
    #[test]
    fn a_covered_record_changed_at_identical_length_folds_the_whole_store() {
        let root = scratch("prefix-changed");
        let paths = a_recorded_run(&root, "r-changed");
        settle(&paths, "build", "done");
        let _ = fold_and_checkpoint(&paths);

        let marker = super::readable(&paths)
            .expect("the checkpoint that read wrote")
            .coverage;
        // Sealed as a writer would have sealed it, so what refuses this document is
        // the prefix moving underneath it rather than the plant itself.
        let mut planted = document(&paths);
        planted["state"]["outcomes"]["build"] = json!("carried-from-the-checkpoint");
        seal_and_write(&paths, &mut planted);
        settle(&paths, "ship", "done");

        // One covered line replaced by as many bytes as it held.
        let store = paths.journal();
        let held = std::fs::read(&store).expect("the run's journal");
        let mut lines: Vec<Vec<u8>> = held
            .split_inclusive(|byte| *byte == b'\n')
            .map(<[u8]>::to_vec)
            .collect();
        let changed = lines[0].len() - 1;
        lines[0] = b"#".repeat(changed).into_iter().chain(*b"\n").collect();
        let mangled = lines.concat();
        assert_eq!(
            mangled.len(),
            held.len(),
            "this journey did not hold the store's length"
        );
        std::fs::write(&store, &mangled).expect("the journal is rewritten");
        let after = super::readable(&paths)
            .expect("the checkpoint is still there")
            .coverage;
        // Everything the marker *counts* is where it was — which is the whole point:
        // no count can see this rewrite, so only the seal can. The seal itself moved
        // when the plant above was sealed, and is not what is being held still here.
        assert_eq!(
            (after.bytes, after.records, &after.at, &after.streams),
            (marker.bytes, marker.records, &marker.at, &marker.streams),
            "the marker moved, so this journey is not about a store that did not"
        );

        // The control is a reader with no checkpoint at all over **this** store,
        // taken after the rewrite rather than before it: what the criterion asks is
        // that the two agree about the journal as it now stands.
        let whole = without_a_checkpoint(&paths);
        let read = folded_as(&fold_and_checkpoint(&paths));
        assert_eq!(
            read.get("outcomes")
                .and_then(|outcomes| outcomes.get("build")),
            None,
            "a checkpoint over a rewritten prefix was folded from anyway: {read}"
        );
        assert_eq!(
            read, whole,
            "the state after a rewritten prefix is not the state the whole store folds to"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// The checked-in shape of a document at [`CHECKPOINT_SCHEMA_VERSION`].
    ///
    /// Read rather than restated, on [`crate::summary`]'s terms: this is the wire a
    /// later build of this crate parses, and the only thing that stops a marker
    /// field being renamed, an absence becoming a zero, the digest turning back
    /// into a number, or the version moving without anyone deciding to move it.
    const GOLDEN: &str = include_str!("../tests/golden/checkpoint-v2.json");

    /// The digest of the journal bytes the golden's marker covers.
    const GOLDEN_BYTES_DIGESTED: u128 = 0x1234_5678_9abc_def0_1234_5678_9abc_def0;

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
            recorded: BTreeMap::from([(
                "build".to_string(),
                crate::projection::Recorded::At(crate::graph::NodeStatus::Done),
            )]),
            last_write_at: Some(1_786_000_000_000),
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
        coverage.digest = coverage.sealed_with(GOLDEN_BYTES_DIGESTED, &state);
        Checkpoint {
            schema_version: CHECKPOINT_SCHEMA_VERSION,
            run_id: "golden".into(),
            coverage,
            state,
        }
    }

    /// The document the build **before** the park-ending fold wrote, kept exactly
    /// as that build wrote it.
    ///
    /// What proves the fold change is a version and not a quiet re-reading: this
    /// is a real schema 1 document, byte-for-byte the shape this build writes, and
    /// the reader has to refuse it rather than resume from a fold it would not
    /// have computed.
    const GOLDEN_V1: &str = include_str!("../tests/golden/checkpoint-v1.json");

    #[test]
    fn the_document_the_build_before_this_one_wrote_is_refused_rather_than_read() {
        let refused = serde_json::from_str::<Checkpoint>(GOLDEN_V1).expect_err("it is refused");
        assert!(
            refused.to_string().contains("schema_version 1"),
            "the refusal does not name the version it met: {refused}"
        );
    }

    #[test]
    fn a_schema_2_document_is_the_shape_the_golden_pins() {
        let rendered = serde_json::to_string_pretty(&a_checkpoint()).expect("it serialises");
        assert_eq!(
            rendered.trim(),
            GOLDEN.trim(),
            "the checkpoint document changed shape. If that was deliberate, bump \
             CHECKPOINT_SCHEMA_VERSION and update tests/golden/checkpoint-v2.json together"
        );
    }

    #[test]
    fn a_schema_2_document_round_trips_and_a_version_this_build_does_not_read_is_refused() {
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
            read.coverage
                .sealed_with(GOLDEN_BYTES_DIGESTED, &read.state),
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
        let written = crate::edits::Park::of(crate::channel::Author::Planner, Some("waiting"));
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
            crate::channel::Author::Planner,
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
