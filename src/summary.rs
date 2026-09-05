//! The per-run **summary document**: what a listing reads instead of a journal.
//!
//! One `summary.json` beside each run's `plan.json` and `result.json`, holding
//! the row a listing renders and the launch record's own account of the run. It
//! exists because the only constructor for a run — [`views::RunView::open`] —
//! folds the run's *entire* merged event store into memory, and a listing builds
//! one per run root: so asking a host what is running read every byte every run
//! had ever recorded, and asking it for one row cost more than asking it for
//! fifty.
//!
//! # Who writes it, and why that one
//!
//! **The journal writer**, folding each appended record into the summary as it
//! appends it — [`Maintainer`], held by [`journal::Journal`]. Two properties
//! come from that and from nothing else: the document is current for a run that
//! is *still recording*, which is the run a listing most needs to be right
//! about; and it costs O(1) per record rather than a pass over the store.
//!
//! # What a reader does when it is not there
//!
//! Folds, once, and caches what it folded. A run recorded by a build that
//! predates this document lists exactly as it does today and only more slowly —
//! which is what makes this landing non-breaking — and a run whose summary is
//! **stale** against the journal's own length or modification time is refolded
//! rather than served. Neither answer differs from the other: the same
//! derivation runs over both paths, so the row a listing serves and the row a
//! full fold produces are one row.
//!
//! # What is deliberately not in it
//!
//! **Liveness.** How a run is being driven is read from the host at the moment
//! of the question — a stored answer is stale the instant it is written, and a
//! stored `ACTIVE` is exactly the reading that sends nobody to a run whose
//! driver died. What this document carries is what
//! [`views::liveness`](crate::views::liveness) takes as *input*: the launch
//! record's pid, host, and start token, and the run's last recorded write. The
//! answer stays computed.
//!
//! [`views::RunView::open`]: crate::views::RunView::open

// llmlint: ignore-file[invalid_states_unrepresentable] every identifier and timestamp on
// `RunSummary` is a `String` for the reason `src/ledger.rs`'s own file-level suppression
// states, and this document is that file's records read back: a run id, a project id, a
// launching session and an instant are *serialized* fields a consumer parses and an older
// build wrote, so every reader has to accept what is there rather than what this build
// would mint. `docs/contract.md` names no `RunId` and no timestamp type, so a newtype here
// would be a public vocabulary the contract did not ask for — and the contract that does
// exist is enforced where it can be: `schema_version` is refused by the deserializer, and
// the one value that could be a nonsense pid is `NonZeroU32` rather than a checked `u32`.
use std::collections::BTreeMap;
use std::num::NonZeroU32;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::event::Envelope;
use crate::graph;
use crate::journal;
use crate::ledger::{self, LaunchRecord, RunPaths, Skipped};
use crate::projection::{self, RunState};
use crate::telemetry::{self, RunTelemetry};

/// The schema version of the summary document.
///
/// The whole compatibility statement: a reader that met a document it does not
/// understand and served it anyway would report a run's state out of fields that
/// mean something else. A version this build does not write is **refused**, and
/// a refused document is not an error — it is a run that folds, which is the
/// answer every run had before this document existed.
///
/// The version is why this document is read **closed** —
/// `deny_unknown_fields` — while the launch record beside it is read
/// permissively. That record has no version and no fallback: refusing it takes
/// the whole run away from every view, which is the incident the ledger's own
/// module documentation records. This one has both, so a key this build does not
/// know costs a fold and nothing else, and the alternative — reading a document
/// half of whose meaning is a build's this one is not — is exactly what the
/// version exists to refuse.
pub const SUMMARY_SCHEMA_VERSION: u32 = 1;

/// Read the version, refusing a document this build cannot honestly read.
fn this_version<'de, D: serde::Deserializer<'de>>(reader: D) -> Result<u32, D::Error> {
    let found = u32::deserialize(reader)?;
    if found != SUMMARY_SCHEMA_VERSION {
        return Err(serde::de::Error::custom(format!(
            "summary schema_version {found}, and this build reads {SUMMARY_SCHEMA_VERSION}"
        )));
    }
    Ok(found)
}

/// One run, as a listing reads it: a bounded read that does not grow with the
/// run's journal.
///
/// Everything here is a **record of what the store said**, never a reading of
/// the host: see the module's note on liveness. Every field the launch record
/// contributes carries that record's own absence policy — a value the record
/// does not state is absent here rather than invented, because a listing that
/// fabricated a launch instant, a host, or a pid would be the more expensive
/// mistake by far.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunSummary {
    /// The document's own version, so a reader can refuse one it does not
    /// understand. See [`SUMMARY_SCHEMA_VERSION`].
    #[serde(deserialize_with = "this_version")]
    pub schema_version: u32,
    /// The run.
    pub run_id: String,
    /// The run's last recorded write, in milliseconds since the epoch.
    ///
    /// **The ordering key.** A listing orders by it, and it is stored rather
    /// than derived for exactly that reason: an order taken from the journal
    /// would drag the whole fold back in for every row on the list. Absent for a
    /// run whose store carries no record this build can date, which is not the
    /// same fact as a run last written at the epoch.
    pub last_write_at: Option<u64>,
    /// The wire string of the last record in the run's merged store, or absent
    /// for a run that has recorded none.
    ///
    /// A wire string rather than a kind of this crate's own: the merged store
    /// interleaves three producers and two of the three spell their kinds in
    /// their own vocabulary.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_event_kind: Option<String>,
    /// How many records the run's merged store holds.
    pub event_count: u64,
    /// Each recorded status word to the number of nodes carrying it, in the
    /// run's own words.
    ///
    /// Derived by the **same precedence [`views`](crate::views) uses** — the
    /// projection's own `statuses`, which recomputes the derived gates against
    /// the graph as it stands — so a row and the graph it opens cannot describe
    /// different graphs. A word no node carries is absent rather than present
    /// and zero.
    pub node_counts: BTreeMap<String, u64>,
    /// Whether a stop has been recorded at all, however it went.
    pub stop_recorded: bool,
    /// Whether every node of the graph reached a state the loop is finished
    /// with, so no further pass is coming.
    ///
    /// Every node settled, over the same statuses
    /// [`node_counts`](Self::node_counts) is counted from, and `false` for a run
    /// whose graph has no nodes at all: a run that has recorded no plan has not
    /// converged, it has not started. With [`stop_recorded`](Self::stop_recorded)
    /// this is what a settled-run filter and a timing-quality reading are decided
    /// from — telemetry over a run still moving is a partial measurement.
    pub graph_complete: bool,
    /// How many decision points are reported as holding dependents back and not
    /// yet reported as released.
    pub decisions_pending: u64,
    /// How many surfaces the run has sent.
    pub surfaces_queued: u64,
    /// How many a planner has consumed.
    pub surfaces_read: u64,
    /// Whether a ready human action is outstanding: one nobody has attested.
    ///
    /// The **graph's** half of a decision point. The other half is a blocking
    /// surface, which lives in the channel rather than the store and is not a
    /// fact this document records — see
    /// [`views::decision_outstanding`](crate::views::decision_outstanding),
    /// which is the whole question.
    pub awaiting_human_action: bool,
    /// The qualified onetaskgraph project id the run was launched with. Empty on
    /// a record written before the store was where a plan came from.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub project: String,
    /// The launcher, as the launch record states it.
    pub launcher: String,
    /// The launching session, or **empty** for an unattributed launch — which is
    /// what a record carrying no session, and one carrying a blank one, both
    /// mean, and what a view labels `[unknown]`.
    pub session: String,
    /// When the run was launched, when the record says.
    ///
    /// Absent, and never an instant standing in for one: a launch instant nobody
    /// recorded is a different fact from one recorded at the epoch, and only the
    /// second is a measurement.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at: Option<String>,
    /// The driver process, when the record names one a reader may act on.
    ///
    /// Absent for the `0` a record naming none defaults to. A pid is one third
    /// of a claim — which process, on which host, and the
    /// [`started`](Self::started) stamp saying it is still that process — and a
    /// pid nobody wrote has no stamp beside it by construction, so nothing that
    /// acts on a pid may act on this one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pid: Option<NonZeroU32>,
    /// The host that pid is meaningful on, when the record names one.
    ///
    /// Absent rather than claimed: a pid means nothing across machines, so a
    /// host the record does not name resolves toward *not this one*.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
    /// The driver's own process start token, when the record carries one.
    ///
    /// The proof that the pid beside it is still the process it was written for.
    /// Absent on a record that predates the stamp — and an absent stamp never
    /// matches, so a reader of this row acts on the pid above exactly as far as
    /// this field lets it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started: Option<String>,
    /// The run's aggregate wall clock and usage.
    ///
    /// The **whole of [`RunTelemetry`](crate::views::RunTelemetry)**, referenced
    /// rather than restated: its fields are declared once, on the type this
    /// crate already aggregates, so there are not two accounts of one run's
    /// clock to drift apart. It is here so that listing a host's runs no longer
    /// costs a process per row to get it.
    pub timing: RunTelemetry,
    /// The journal's length in bytes when this document was written.
    ///
    /// Half of the stamp a stale summary is detected by. The journal is
    /// append-only, so a length that has moved is a record this document does
    /// not know about.
    pub journal_len: u64,
    /// The journal's modification time when this document was written, in
    /// milliseconds since the epoch.
    ///
    /// The other half, for the change a length cannot see: a store rewritten to
    /// the same size — healed of a torn tail, or edited by hand — is a store
    /// this document no longer describes.
    pub journal_mtime_ms: u64,
}

/// The journal's length and modification time, as it stands.
///
/// A journal that is not there stamps as `(0, 0)`: a run with no store yet is a
/// real state — a directory and a launch record, written before the first record
/// — and it is one a summary describes exactly as well as any other.
fn journal_stamp(paths: &RunPaths) -> (u64, u64) {
    let Ok(about) = std::fs::metadata(paths.journal()) else {
        return (0, 0);
    };
    let modified = about
        .modified()
        .ok()
        .and_then(|at| at.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |since| {
            u64::try_from(since.as_millis()).unwrap_or(u64::MAX)
        });
    (about.len(), modified)
}

/// What a summary stamps: the bytes of the journal it **accounted for**, and
/// when the file was last written.
///
/// The length is the writer's own count rather than whatever the file holds at
/// the moment of the stat, and the difference is the whole safety of the stamp.
/// A run's journal has several appenders — the launcher relaying its driver's
/// stream, and the engine loop's own writer — so one landing between our append
/// and our stat would have us declare a document fresh for a record it does not
/// carry, and a reader would serve a row one record behind with nothing saying
/// so. Counting what was folded can only ever fall *short* of the file, and a
/// stamp that falls short reads as stale, which costs a fold and never an
/// answer.
type Stamp = (u64, u64);

impl RunSummary {
    /// One run's summary: the stored document where it is current, and a fold
    /// where it is not.
    ///
    /// The **only** public way to this document, so no caller decides for itself
    /// whether a stored one may be served. A run with no summary, or one whose
    /// summary is stale against the journal's recorded length or modification
    /// time, is folded exactly as [`views::RunView::open`](crate::views::RunView::open)
    /// folds it and the fold is cached — so the next reader of that run pays a
    /// bounded read, and a run recorded by an older build answers identically
    /// and only more slowly.
    ///
    /// The cache is written best-effort and its failure is never reported: a
    /// read-only runs root, or a directory this reader may not write, costs the
    /// next reader a fold and costs this one nothing. A cache written beside a
    /// live driver may be overtaken by that driver's own next append, which is
    /// the same staleness this document is read through in the first place.
    pub fn of(paths: &RunPaths) -> crate::Result<Self> {
        if !paths.exists() {
            return Err(crate::Error::NoSuchRun {
                run: paths.run.clone(),
                root: paths.dir.parent().unwrap_or(Path::new(".")).to_path_buf(),
            });
        }
        let stamp = journal_stamp(paths);
        if let Some(stored) = ledger::read_json_opt::<Self>(&paths.summary()) {
            if (stored.journal_len, stored.journal_mtime_ms) == stamp && stored.run_id == paths.run
            {
                return Ok(stored);
            }
        }
        // Stamped with what the store held **before** the fold, for the reason
        // [`Stamp`] states: a record appended while this read was folding is one
        // this row may not claim to carry.
        let folded = Self::folded(paths, stamp)?;
        let _ = ledger::write_json(&paths.summary(), &folded);
        Ok(folded)
    }

    /// The same summary, always by folding the whole store.
    ///
    /// What the fallback runs, and what the writer's own account is held equal
    /// to: one derivation, so the two accounts of a run cannot drift apart.
    fn folded(paths: &RunPaths, stamp: Stamp) -> crate::Result<Self> {
        let view = crate::views::RunView::open(paths)?;
        Ok(Self::derive(
            &paths.run,
            &view.launch,
            &view.state,
            view.events.len() as u64,
            view.events.last().map(|event| event.kind.0.clone()),
            &telemetry::of_run(paths, &view.events),
            stamp,
        ))
    }

    /// Compose the document out of a run's launch record and its folded state.
    ///
    /// The one place either producer builds a row, which is what makes the two
    /// the same row.
    fn derive(
        run: &str,
        launch: &LaunchRecord,
        state: &RunState,
        event_count: u64,
        last_event_kind: Option<String>,
        timing: &RunTelemetry,
        (journal_len, journal_mtime_ms): Stamp,
    ) -> Self {
        let statuses = state.statuses();
        let mut node_counts: BTreeMap<String, u64> = BTreeMap::new();
        for status in statuses.values() {
            *node_counts.entry(status.as_str().to_string()).or_insert(0) += 1;
        }
        Self {
            schema_version: SUMMARY_SCHEMA_VERSION,
            run_id: run.to_string(),
            last_write_at: state.last_write_at,
            last_event_kind,
            event_count,
            node_counts,
            stop_recorded: state.stop_recorded(),
            graph_complete: !statuses.is_empty() && graph::is_terminal(&statuses),
            decisions_pending: state.decisions_pending.len() as u64,
            surfaces_queued: state.surfaces_queued,
            surfaces_read: state.surfaces_read,
            awaiting_human_action: state.awaiting_human_action(),
            project: launch.project.clone(),
            launcher: launch.launcher.clone(),
            session: launch.session.clone(),
            started_at: launch.launched_at().map(str::to_string),
            pid: launch.driver_pid(),
            host: launch.recorded_host().map(str::to_string),
            started: (!launch.started.is_empty()).then(|| launch.started.clone()),
            timing: timing.clone(),
            journal_len,
            journal_mtime_ms,
        }
    }
}

/// What a bounded listing over a whole runs root read, and what it refused.
///
/// The second half is why this type has one: a listing that reported only what
/// it could read would reintroduce, at the cheap surface, the silent omission
/// [`Survey`](crate::views::Survey) exists to remove — a host with thirty run
/// roots on it rendering as nothing at all. A refused root is a fact about the
/// root, and it arrives on the same terms
/// [`Survey::skipped`](crate::views::Survey::skipped) already states.
#[derive(Debug, Clone, PartialEq)]
pub struct Listing {
    /// The runs root this listing read. Named on the output, because it is the
    /// scope of every claim made from it.
    pub root: PathBuf,
    /// The runs that read, **most recently written first** — the order
    /// [`RunSummary::last_write_at`] is stored to make answerable without a
    /// fold. A run whose store carries nothing datable sorts last, then by id,
    /// so the order is total and stable.
    pub summaries: Vec<RunSummary>,
    /// The run roots that did not, each with the reason it was refused.
    pub skipped: Vec<Skipped>,
}

impl Listing {
    /// Read every run under a root, keeping what could not be read.
    ///
    /// The bounded counterpart of [`Survey::of`](crate::views::Survey::of), and
    /// the same account of a refusal: a root the ledger refused and a run this
    /// build could neither read nor fold are the same fact to a reader — one
    /// directory that claimed to be a run and is not being reported as one — so
    /// they arrive on one list.
    pub fn of(root: &Path) -> Self {
        let index = ledger::all_runs(root);
        let mut summaries = Vec::new();
        let mut skipped = index.skipped;
        for paths in index.runs {
            match RunSummary::of(&paths) {
                Ok(summary) => summaries.push(summary),
                // The refusal as the folding reader already words it. Nothing is
                // added to it here — a second wording of one refusal is a second
                // thing to keep true.
                Err(error) => skipped.push(Skipped {
                    path: paths.dir,
                    reason: error.to_string(),
                }),
            }
        }
        summaries.sort_by(|a, b| {
            b.last_write_at
                .cmp(&a.last_write_at)
                .then_with(|| a.run_id.cmp(&b.run_id))
        });
        skipped.sort_by(|a, b| a.path.cmp(&b.path));
        Self {
            root: root.to_path_buf(),
            summaries,
            skipped,
        }
    }
}

/// A run's store folded: everything the summary is derived from except the
/// launch record.
///
/// One value rather than four fields side by side, because it is carried
/// forward as a unit — [`Maintainer`] holds the fold of everything settled and
/// re-folds the records still arriving at one instant onto a copy of it, and a
/// copy that took three of the four would be a state describing a store nobody
/// recorded.
#[derive(Debug, Default, Clone)]
struct Folded {
    state: RunState,
    aggregate: telemetry::Aggregate,
    events: u64,
    /// The kind of the record the **merge order** ends with, which for a store
    /// folded in that order is whatever was taken last.
    last_event_kind: Option<String>,
}

impl Folded {
    /// The state of an empty store, as [`projection::fold`] starts from.
    fn new() -> Self {
        Self {
            state: RunState {
                strict: true,
                ..RunState::default()
            },
            ..Self::default()
        }
    }

    /// Take one record that belongs at the end of the merge order.
    fn take(&mut self, paths: &RunPaths, event: &Envelope) {
        projection::fold_one(&mut self.state, event);
        self.aggregate.fold(paths, event);
        self.events += 1;
        self.last_event_kind = Some(event.kind.0.clone());
    }
}

/// One run's summary, kept current a record at a time by the process appending
/// to its journal.
///
/// Held by [`journal::Journal`], which is the only writer of a run's merged
/// store, so the document is written by whatever wrote the record it describes
/// and is current for a live run rather than as of some later pass.
///
/// # Why an appended record can be folded at all
///
/// Because the derivations are over the store in **merge order**, and while
/// every record arrives at or past the last instant already recorded, that order
/// is: records by timestamp, ties between streams broken by stream id, each
/// stream's own `seq` preserved. So this holds the fold of everything stamped
/// *before* the newest instant, and the handful of records stamped **at** it —
/// two producers relay their own timestamps into this store and a millisecond
/// holds several records, so a record arriving beside one already folded has to
/// be able to sort in front of it. Serving the summary re-folds that handful
/// onto a copy, which is bounded by one instant's records rather than by the
/// run's length.
///
/// Two arrivals do not belong anywhere this can place them, and the answer to
/// both is to read the store again. One is a record stamped *behind* the newest
/// instant **any** stream has reached — a producer's clock is not this one's, and
/// the merge holds a stream stamped ahead of the others back until they drain, so
/// the order can end behind an instant the store already carries. The other is a
/// record whose
/// stream has already had a **higher** `seq` folded and settled: a stream is
/// merged in its own `seq` order whatever its stamps say, so a producer that
/// publishes `seq` 10 stamped before its `seq` 5 puts the later record first in
/// the merge and there is no instant this can hold it at. Both are real — the
/// second is what an `oneharness-session` record does, published out of band and
/// stamped when its session opened — and both are rare, which is what makes
/// reading again affordable.
#[derive(Debug)]
pub(crate) struct Maintainer {
    paths: RunPaths,
    /// The fold of every record stamped **before** [`open_ts`](Self::open_ts).
    settled: Folded,
    /// The records stamped **at** it, unfolded, so one arriving beside them can
    /// still sort in front of them.
    open: Vec<Envelope>,
    /// The instant the **merge order ends at**. Empty for a store with nothing in
    /// it, which every stamp is past.
    open_ts: String,
    /// The newest instant **any** record in this store carries, which is not
    /// always the one above.
    ///
    /// The merge is a k-way one — each stream in its own `seq`, streams
    /// interleaved by `ts` — so a stream whose head is stamped ahead of every
    /// other stream's remaining records is held back until those are drained, and
    /// the order then ends on a record stamped *behind* one already placed. A
    /// producer whose clock runs ahead of this one's is ordinary between hosts, so
    /// this is not a rare shape. An arrival behind this instant has records to
    /// sort in front of wherever the order happens to end, and is not one this
    /// state can place.
    newest_ts: String,
    /// How many bytes of the journal this state has accounted for. See
    /// [`Stamp`].
    accounted: u64,
    /// The highest `seq` each stream has **settled**, which is what an arriving
    /// record of that stream has to be past: the merge orders a stream by its own
    /// `seq` and by nothing else, so a lower one arriving now belongs in front of
    /// a record this state has already frozen.
    settled_seq: BTreeMap<String, u64>,
}

impl Maintainer {
    /// Build the state of a run's store as it stands, by reading it once.
    ///
    /// The one unbounded read in this file, and it happens where an unbounded
    /// read already did: a journal writer opens by reading the store to find the
    /// sequence number it may claim.
    pub(crate) fn of(paths: &RunPaths) -> Self {
        // Bracketed, so what is accounted for is what was read: the two agree
        // unless another appender landed across the read, and where they do not
        // the shorter is taken, which reads as stale rather than as an answer.
        let (before, _) = journal_stamp(paths);
        let mut events = journal::read(&paths.journal());
        let (after, _) = journal_stamp(paths);
        journal::merge_order(&mut events);

        // The last instant's records are held open rather than folded, because
        // the next record to arrive may be stamped at that same instant and
        // belong in front of one of them.
        //
        // The **trailing run** of them, counted from the end, and not every
        // record that happens to carry that stamp: the merge orders by each
        // stream's own `seq` first, so a store some producer stamped out of
        // order can carry that instant earlier as well — and taking those with
        // it would re-sort records the merge had already placed, which is a
        // different store from the one on disk.
        let open_ts = events
            .last()
            .map(|event| event.ts.clone())
            .unwrap_or_default();
        let opened = events
            .iter()
            .rposition(|event| event.ts != open_ts)
            .map_or(0, |before| before + 1);
        let mut settled = Folded::new();
        let mut settled_seq = BTreeMap::new();
        for event in &events[..opened] {
            settled.take(paths, event);
            seq_reached(&mut settled_seq, event);
        }
        Self {
            paths: paths.clone(),
            settled,
            open: events[opened..].to_vec(),
            newest_ts: events
                .iter()
                .map(|event| &event.ts)
                .max()
                .cloned()
                .unwrap_or_default(),
            open_ts,
            accounted: before.min(after),
            settled_seq,
        }
    }

    /// The whole store folded: everything settled, plus the newest instant's
    /// records in the order the merge puts them.
    ///
    /// Within one instant the merge orders by stream and then by each stream's
    /// own `seq`, which is what this sorts by — the same order
    /// [`journal::merge_order`] would put them in, decided in one place so the
    /// two cannot disagree.
    fn current(&self) -> Folded {
        let mut folded = self.settled.clone();
        let mut open: Vec<&Envelope> = self.open.iter().collect();
        open.sort_by(|a, b| a.stream.cmp(&b.stream).then(a.seq.cmp(&b.seq)));
        for event in open {
            folded.take(&self.paths, event);
        }
        folded
    }

    /// Take one appended record, and write the run's summary.
    ///
    /// Called after the record has reached the file, so a read of the store —
    /// the answer to a record this state cannot place — reads a store that
    /// already holds it.
    ///
    /// `healed` is what the same append cut off the file *before* writing that
    /// record — a fragment a dead writer left, which
    /// [`ledger::append_line_healed`] reports. It is subtracted first, because
    /// this state's count is a byte offset into the store and a heal moves every
    /// boundary past it: counting on regardless would leave the offset inside a
    /// record, and the tail read from there drops the record it starts in and
    /// then stamps the document **fresh** — a run recorded as never stopped, on
    /// a store whose last record says it was. Where the arithmetic does not come
    /// out exactly — a fragment this state never counted, or an appender that
    /// landed across the heal — the store is read again rather than folded from
    /// an offset nothing can place. That is a whole read on the rarest path
    /// there is, and the alternative is a wrong row served as a current one.
    pub(crate) fn appended(&mut self, event: &Envelope, healed: u64, bytes: u64) {
        let len = journal_stamp(&self.paths).0;
        self.accounted = self.accounted.saturating_sub(healed);
        if len == self.accounted + bytes {
            // Ours alone: the file grew by exactly this record, so what is folded
            // here and what the file holds are the same store.
            self.fold(event, bytes);
        } else if healed == 0 && len > self.accounted {
            // Somebody else appended beside us, which is the ordinary shape of a
            // run being driven: the relay thread writes the observer's envelopes
            // while the engine thread writes the graph's. What the store grew by
            // is read **from where this state left off** rather than from the
            // beginning — reading it whole per append would make recording a run
            // quadratic in its own length, which is the cost this document exists
            // to remove, reintroduced at the writer.
            self.catch_up();
        } else {
            // The file is not the file this state was holding: it is shorter than
            // what was accounted for — replaced, or healed of a fragment nobody
            // here counted — or a heal has moved the boundaries a tail read would
            // start from. Nothing here can be placed against it.
            *self = Self::of(&self.paths);
        }
        self.write();
    }

    /// Fold one record, or read the store again where it does not belong at or
    /// past the newest instant — and say which it did.
    ///
    /// A read has taken the whole store, so a caller walking a tail of it has
    /// nothing left to fold and must stop rather than fold the same records
    /// twice.
    fn fold(&mut self, event: &Envelope, bytes: u64) -> Rebuilt {
        let settled_past_it = self
            .settled_seq
            .get(&event.stream)
            .is_some_and(|reached| event.seq <= *reached);
        // A record stamped later than the instant still open closes that instant,
        // so one whose own stream is *already* further on inside it cannot be
        // placed either: the merge would put this record in front of one about to
        // be frozen behind it.
        let closing_over_it = event.ts > self.open_ts
            && self
                .open
                .iter()
                .any(|held| held.stream == event.stream && held.seq > event.seq);
        if settled_past_it || closing_over_it || event.ts < self.newest_ts {
            *self = Self::of(&self.paths);
            return Rebuilt::Yes;
        }
        if event.ts > self.open_ts {
            self.settled = self.current();
            for settled in &self.open {
                seq_reached(&mut self.settled_seq, settled);
            }
            self.open_ts = event.ts.clone();
            self.open.clear();
        }
        self.open.push(event.clone());
        if event.ts > self.newest_ts {
            self.newest_ts = event.ts.clone();
        }
        self.accounted += bytes;
        Rebuilt::No
    }

    /// Fold everything the store has grown by since this state last accounted
    /// for it.
    ///
    /// Bounded by what arrived rather than by what the run has ever recorded: the
    /// tail is read from the byte this state stopped at. A record in it stamped
    /// behind the newest instant takes the whole store again, which is the same
    /// answer [`fold`](Self::fold) gives for one appended here — and one this
    /// build cannot read still advances the count, because it is still a line the
    /// file holds.
    fn catch_up(&mut self) {
        for (record, bytes) in journal::read_after(&self.paths.journal(), self.accounted) {
            match record {
                // A read answered the whole tail; there is nothing left of it
                // this state has not already taken.
                Some(event) if self.fold(&event, bytes) == Rebuilt::Yes => return,
                Some(_) => {}
                None => self.accounted += bytes,
            }
        }
    }

    /// Write what the run stands at now.
    ///
    /// Best effort, and its failure is never reported: a summary that could not
    /// be written costs the next reader a fold, and a journal append that
    /// refused because the document beside it could not be written would be this
    /// cache taking a run's own record down with it.
    fn write(&mut self) {
        let mut folded = self.current();
        // Cross-DAG edges are resolved the way a view resolves them, so a row
        // and the graph it opens cannot describe different graphs. A graph
        // naming no other run pays a walk of its own nodes and nothing else; one
        // that does names pays that edge's read, which is what any reader of it
        // already pays.
        folded.state.cross_dag = crate::crossdag::resolve_quietly(
            &self
                .paths
                .dir
                .parent()
                .map_or_else(ledger::runs_root, Path::to_path_buf),
            &folded.state.graph,
        );
        let summary = RunSummary::derive(
            &self.paths.run,
            &self.launch(),
            &folded.state,
            folded.events,
            folded.last_event_kind.clone(),
            &folded.aggregate.finish(&self.paths.run, &folded.state),
            (self.accounted, journal_stamp(&self.paths).1),
        );
        let _ = ledger::write_json(&self.paths.summary(), &summary);
    }

    /// The run's launch record, as the row's attribution needs it.
    ///
    /// Read from the file on each write rather than held: the record is rewritten
    /// under a live run — an adoption claims the run for a fresh driver, and the
    /// observer's graph run is recorded after the launch that made it — and a
    /// copy taken once would go on naming the driver that died. A record this
    /// build cannot read leaves the row with the launch's defaults, which say
    /// exactly what they say everywhere else: the record does not say.
    fn launch(&self) -> LaunchRecord {
        ledger::read_json_opt::<LaunchRecord>(&self.paths.launch()).unwrap_or(LaunchRecord {
            run_id: self.paths.run.clone(),
            project: String::new(),
            dir: PathBuf::new(),
            graph: String::new(),
            graph_run: String::new(),
            observer_runs: Vec::new(),
            observer_ending: String::new(),
            node_graph: String::new(),
            pr_author_graph: String::new(),
            node_validator: String::new(),
            envelope_reviewer: String::new(),
            launcher: crate::sys::UNKNOWN_LAUNCHER.to_string(),
            session: String::new(),
            pid: 0,
            host: String::new(),
            started: String::new(),
            started_at: String::new(),
            heartbeat_interval: 0,
            dag_sets: Vec::new(),
            node_sets: Vec::new(),
            adoptions: 0,
            filters: crate::filter::Filters::default(),
        })
    }
}

/// Record how far a stream has been folded, keeping the highest `seq` seen.
///
/// The highest rather than the last, because a producer may publish its own
/// records out of `seq` order and the question this answers is what the merge
/// has already placed.
fn seq_reached(reached: &mut BTreeMap<String, u64>, event: &Envelope) {
    let held = reached.entry(event.stream.clone()).or_insert(event.seq);
    *held = (*held).max(event.seq);
}

/// Whether folding a record took the whole store again — which is what a caller
/// walking a tail has to stop on, or it folds the same records twice.
#[derive(Debug, PartialEq, Eq)]
enum Rebuilt {
    No,
    Yes,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::{EventKind, Labels, Source, ENVELOPE_VERSION};
    use crate::journal::{Journal, PipelineKind};
    use crate::plan::{Node, Plan, PLAN_SCHEMA_VERSION};
    use crate::sys;
    use serde_json::json;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("onepipeline-summary-{name}-{}", sys::pid()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("a scratch root");
        dir
    }

    fn plan(nodes: &[&str]) -> Plan {
        Plan {
            schema_version: PLAN_SCHEMA_VERSION,
            goal: Some(crate::plan::Goal {
                text: "list a host without folding it".into(),
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
        let mut record = LaunchRecord {
            run_id: run.to_string(),
            project: "plans:demo".into(),
            dir: PathBuf::from("/tmp/launch"),
            graph: String::new(),
            graph_run: String::new(),
            observer_runs: Vec::new(),
            observer_ending: String::new(),
            node_graph: String::new(),
            pr_author_graph: String::new(),
            node_validator: String::new(),
            envelope_reviewer: String::new(),
            launcher: "e2e".into(),
            session: "a-session".into(),
            pid: 0,
            host: String::new(),
            started: String::new(),
            started_at: sys::now_rfc3339(),
            heartbeat_interval: 1_800,
            dag_sets: Vec::new(),
            node_sets: Vec::new(),
            adoptions: 0,
            filters: crate::filter::Filters::default(),
        };
        record.driven_by_this_process();
        ledger::write_json(&paths.launch(), &record).expect("a launch record");
        paths
    }

    /// One of this crate's own records, as its writer emits it.
    fn emit(journal: &mut Journal, kind: PipelineKind, node: Option<&str>, run: &str) {
        journal
            .emit(
                kind,
                crate::journal::labels(run, node),
                crate::journal::payload(&[("status", json!("done"))]),
            )
            .expect("appended");
    }

    /// A run whose store holds `records` of this crate's own, written through
    /// the real journal writer — which is what keeps the summary current.
    fn recorded(root: &Path, run: &str, records: usize) -> RunPaths {
        let paths = a_run(root, run);
        let mut journal = Journal::open(&paths);
        journal
            .emit(
                PipelineKind::RunStarted,
                crate::journal::labels(run, None),
                crate::journal::payload(&[("plan", json!(plan(&["build", "ship"])))]),
            )
            .expect("appended");
        for nth in 0..records {
            emit(
                &mut journal,
                PipelineKind::NodeReady,
                Some(if nth % 2 == 0 { "build" } else { "ship" }),
                run,
            );
        }
        paths
    }

    /// What one read of a run's summary cost, in bytes off the ledger.
    fn cost_of(paths: &RunPaths) -> (RunSummary, u64) {
        let before = ledger::bytes_read();
        let summary = RunSummary::of(paths).expect("the run reads");
        (summary, ledger::bytes_read() - before)
    }

    /// The whole point of the document: a listing's cost does not grow with the
    /// journals it is listing.
    ///
    /// Two runs three orders of magnitude apart, measured on **bytes read off
    /// the ledger** rather than on a clock — a wall-clock ratio says nothing
    /// about why it was fast, and this says exactly what was opened. The fold
    /// beside it is the control: it reads the whole store, and it is the reading
    /// every listing did before this document existed.
    #[test]
    fn a_summary_read_is_bounded_and_the_fold_it_replaces_is_not() {
        let root = scratch("bounded");
        let small = recorded(&root, "small", 10);
        let large = recorded(&root, "large", 10_000);
        assert!(
            std::fs::metadata(large.journal()).expect("a store").len()
                > 100 * std::fs::metadata(small.journal()).expect("a store").len(),
            "the two stores are not orders of magnitude apart"
        );

        let (small_row, small_cost) = cost_of(&small);
        let (large_row, large_cost) = cost_of(&large);
        assert_eq!(small_row.event_count, 11);
        assert_eq!(large_row.event_count, 10_001);
        // Both read one document each and stat one journal, so the cost is the
        // document's — which is the graph's size and not the store's.
        assert!(
            large_cost < 2 * small_cost,
            "reading the larger run's summary cost {large_cost} bytes against \
             {small_cost} for a store a thousandth the size"
        );

        // And the reading it replaces, over the same two stores: proportional,
        // which is what makes the measurement above mean something.
        let folded = |paths: &RunPaths| {
            std::fs::remove_file(paths.summary()).expect("the document");
            cost_of(paths).1
        };
        let small_fold = folded(&small);
        let large_fold = folded(&large);
        assert!(
            large_fold > 100 * small_fold,
            "the fold this replaces cost {large_fold} against {small_fold}, so the \
             measurement above is not measuring what a listing reads"
        );
        std::fs::remove_dir_all(&root).ok();
    }

    /// A run with no summary answers identically, folding once, and the next
    /// reader of it pays a bounded read.
    #[test]
    fn a_run_with_no_summary_folds_once_and_caches_what_it_folded() {
        let root = scratch("fallback");
        let paths = recorded(&root, "demo", 40);
        let maintained = RunSummary::of(&paths).expect("the run reads");

        std::fs::remove_file(paths.summary()).expect("the document");
        let (folded, fold_cost) = cost_of(&paths);
        assert_eq!(
            folded, maintained,
            "the row a fold produces differs from the row the writer maintained"
        );
        assert!(
            paths.summary().is_file(),
            "the fold was not cached, so every later reader folds again"
        );

        let (again, cached_cost) = cost_of(&paths);
        assert_eq!(again, folded);
        assert!(
            cached_cost < fold_cost,
            "the cached read cost {cached_cost} against a fold's {fold_cost}"
        );
        std::fs::remove_dir_all(&root).ok();
    }

    /// A summary that no longer describes the store beside it is refolded.
    ///
    /// Both halves of the stamp, because they catch different things: a record
    /// appended behind the writer's back moves the journal's **length**, and a
    /// store rewritten to the same size — healed of a torn tail, or edited by
    /// hand — moves only its **modification time**.
    #[test]
    fn a_stale_summary_is_refolded_rather_than_served() {
        let root = scratch("stale");
        let paths = recorded(&root, "demo", 6);
        let served = RunSummary::of(&paths).expect("the run reads");
        assert_eq!(served.event_count, 7);

        // A record appended by something that is not this writer: the length
        // moves, and the document no longer describes the store.
        let mut appended = event(PipelineKind::NodeReady, "demo", "other-stream", 0);
        appended.ts = sys::now_rfc3339();
        ledger::append_line(
            &paths.journal(),
            &serde_json::to_string(&appended).expect("a record"),
        )
        .expect("appended");
        let refolded = RunSummary::of(&paths).expect("the run reads");
        assert_eq!(
            refolded.event_count, 8,
            "a summary that predates a record in the store was served anyway"
        );

        // And the same size, rewritten: only the modification time says so.
        let store = std::fs::read(paths.journal()).expect("the store");
        let len = store.len();
        // Far enough ahead that no filesystem's timestamp granularity hides it.
        std::thread::sleep(std::time::Duration::from_millis(1_100));
        std::fs::write(paths.journal(), &store[..len - 1]).expect("a store rewritten in place");
        std::fs::write(paths.journal(), [&store[..len - 1], b"\n"].concat())
            .expect("a store rewritten to its own length");
        assert_eq!(
            std::fs::metadata(paths.journal()).expect("the store").len() as usize,
            len,
            "the rewrite changed the length, so this is not the case under test"
        );
        let served = RunSummary::of(&paths).expect("the run reads");
        assert_eq!(
            (served.journal_len, served.journal_mtime_ms),
            journal_stamp(&paths),
            "a summary written against a store that has since been rewritten was served"
        );
        std::fs::remove_dir_all(&root).ok();
    }

    /// A run that is **still recording** is listed at bounded cost, because the
    /// document is kept current as the store grows rather than written once.
    ///
    /// Several rounds, and after each one both halves: the served row says what
    /// the run now is, and serving it cost what it cost the round before.
    #[test]
    fn a_run_still_recording_stays_current_and_stays_bounded() {
        let root = scratch("growing");
        let paths = a_run(&root, "demo");
        let mut journal = Journal::open(&paths);
        journal
            .emit(
                PipelineKind::RunStarted,
                crate::journal::labels("demo", None),
                crate::journal::payload(&[("plan", json!(plan(&["build"])))]),
            )
            .expect("appended");

        let mut costs = Vec::new();
        let mut written = 1;
        for round in 1..=4 {
            for _ in 0..(round * 500) {
                emit(&mut journal, PipelineKind::NodeReady, Some("build"), "demo");
                written += 1;
            }
            let (row, cost) = cost_of(&paths);
            assert_eq!(
                row.event_count, written,
                "the served summary is behind the store it describes"
            );
            assert_eq!(
                row.last_event_kind.as_deref(),
                Some(PipelineKind::NodeReady.as_str())
            );
            costs.push(cost);
        }
        let (first, last) = (costs[0], costs[costs.len() - 1]);
        assert!(
            last < 2 * first,
            "serving the summary grew with the journal: {costs:?}"
        );
        std::fs::remove_dir_all(&root).ok();
    }

    /// A record that arrives **behind** the newest instant recorded is not folded
    /// onto the end of it.
    ///
    /// Two producers relay their own timestamps into this store, so a record
    /// landing out of order is a case rather than a hypothetical — and the whole
    /// of the timing account is a walk of the timeline in order. The answer is
    /// to rebuild, and what proves it is the fold beside it: the two rows are
    /// one row.
    #[test]
    fn a_record_stamped_behind_the_newest_instant_is_reread_rather_than_folded_onto_the_end() {
        let root = scratch("out-of-order");
        let paths = recorded(&root, "demo", 4);

        // A sibling's record, stamped before everything already in the store.
        let mut behind = event(PipelineKind::NodeReady, "demo", "a-sibling", 7);
        behind.source = Source::Agentgraph;
        behind.kind = EventKind("turn-completed".into());
        behind.ts = "2020-01-01T00:00:00.000Z".into();
        behind
            .payload
            .insert("usage".into(), json!({"input_tokens": 11}));
        let mut journal = Journal::open(&paths);
        journal.relay(&behind).expect("relayed");

        let served = RunSummary::of(&paths).expect("the run reads");
        std::fs::remove_file(paths.summary()).expect("the document");
        let folded = RunSummary::of(&paths).expect("the run reads");
        assert_eq!(
            served, folded,
            "a record stamped behind the newest instant left the two accounts apart"
        );
        // And it is measured where it belongs: the run's clock now starts at the
        // record that is stamped first.
        assert!(
            folded.timing.wall_ms > 0,
            "a record stamped years before the store left no wall clock"
        );
        std::fs::remove_dir_all(&root).ok();
    }

    /// A producer that publishes its records out of its own `seq` order.
    ///
    /// Not a hypothetical: `oneagentgraph` relays an `oneharness-session` record
    /// out of band, stamped when the session opened and carrying a `seq` far past
    /// the turn records around it — so one stream arrives `2, 3, 10, 4, 5` with
    /// `10` stamped *before* `5`. The merge orders a stream by its own `seq` and
    /// by nothing else, so `10` belongs after `5` however it is stamped, and
    /// there is no instant a state folding forward can hold it at. What the
    /// writer does is read the store again — and what this holds is that it does,
    /// by holding its row equal to the fold's.
    ///
    /// **Both ways the record can be reached**, because they are caught by
    /// different readings and only one of them was: the out-of-order `seq` can
    /// arrive while the instant it belongs behind is still open, and it can
    /// arrive after something else has already closed that instant.
    #[test]
    fn a_producer_publishing_out_of_its_own_seq_order_leaves_one_account_not_two() {
        let root = scratch("out-of-seq");
        // The stream, exactly as it arrives, and then the same stream with a
        // record of *another* producer closing the instant in between.
        let closed_by_another = |also: bool| -> Vec<(&'static str, u64, &'static str, bool)> {
            let mut relayed = vec![
                ("a-sibling", 2, "turn-activity", false),
                ("a-sibling", 3, "turn-message", false),
                // The record out of `seq` order, and one that means something: it
                // opens the judge side's turn, so where it is folded decides
                // whose the run's last stretch of clock is.
                ("a-sibling", 10, "member-started", true),
                ("a-sibling", 4, "turn-completed", false),
            ];
            if also {
                relayed.push(("b-sibling", 0, "turn-activity", false));
            }
            relayed.push(("a-sibling", 5, crate::report::MEMBER_SETTLED, false));
            relayed
        };

        for (run, relayed) in [
            ("still-open", closed_by_another(false)),
            ("already-closed", closed_by_another(true)),
        ] {
            let paths = a_run(&root, run);
            let mut engine = Journal::open(&paths);
            engine
                .emit(
                    PipelineKind::RunStarted,
                    crate::journal::labels(run, None),
                    crate::journal::payload(&[("plan", json!(plan(&["build"])))]),
                )
                .expect("appended");
            emit(
                &mut engine,
                PipelineKind::NodeDispatched,
                Some("build"),
                run,
            );

            let mut relay = Journal::open(&paths);
            let base = sys::now_millis();
            let instant = sys::rfc3339_from_millis(base);
            let later = sys::rfc3339_from_millis(base + 5);
            for (stream, seq, kind, judge) in relayed {
                let mut record = event(PipelineKind::NodeReady, run, stream, seq);
                record.source = Source::Agentgraph;
                record.kind = EventKind(kind.into());
                // The two records past the burst are stamped at the later
                // instant; everything in the burst shares the first one.
                record.ts = if seq == 5 || stream == "b-sibling" {
                    later.clone()
                } else {
                    instant.clone()
                };
                if judge {
                    record.payload.insert("role".into(), json!("judge"));
                }
                relay.relay(&record).expect("relayed");
            }
            // Far enough after that the span the run's clock ends on is a real
            // one: the record out of `seq` order is stamped *before* the one
            // merged in front of it, so where the two orders leave the clock is
            // where they differ — and a span of nought would hide it.
            std::thread::sleep(std::time::Duration::from_millis(30));
            emit(&mut engine, PipelineKind::NodeSettled, Some("build"), run);

            let served = RunSummary::of(&paths).expect("the run reads");
            std::fs::remove_file(paths.summary()).expect("the document");
            assert_eq!(
                served,
                RunSummary::of(&paths).expect("the run folds"),
                "on '{run}' a producer's out-of-order `seq` left the maintained row \
                 and the folded row apart"
            );
        }
        std::fs::remove_dir_all(&root).ok();
    }

    /// Two writers on one run's journal, which is the ordinary shape of a run
    /// being driven: the relay thread appends the observer's envelopes while the
    /// engine thread appends the graph's.
    ///
    /// Both keep the document current, and neither re-reads the store to do it.
    /// Reading the store again per append is the obvious answer to "somebody
    /// else appended", and it makes recording a run quadratic in its own length
    /// — which is the cost this document exists to remove, reintroduced at the
    /// writer.
    #[test]
    fn two_writers_on_one_journal_keep_the_summary_current_without_rereading_the_store() {
        let root = scratch("two-writers");
        let paths = recorded(&root, "demo", 400);
        let store = std::fs::metadata(paths.journal()).expect("a store").len();

        let mut engine = Journal::open(&paths);
        let mut relay = Journal::open(&paths);
        let before = ledger::bytes_read();
        const ROUNDS: usize = 40;
        for nth in 0..ROUNDS {
            emit(&mut engine, PipelineKind::NodeReady, Some("build"), "demo");
            let mut relayed = event(PipelineKind::NodeReady, "demo", "a-sibling", nth as u64);
            relayed.source = Source::Agentgraph;
            relayed.kind = EventKind("turn-completed".into());
            relay.relay(&relayed).expect("relayed");
        }
        let maintaining = ledger::bytes_read() - before;
        // Reading the store again for each of these would cost the store's whole
        // length every time. What keeping the document current may cost is the
        // tails that arrived, plus the launch record each row's attribution is
        // read from — both real reads, and neither proportional to the run's
        // length. The bound rules out the one that is proportional to both.
        let rereading = store * (ROUNDS as u64) * 2;
        assert!(
            maintaining * 4 < rereading,
            "keeping the document current across {} interleaved appends read {maintaining} \
             bytes, against {rereading} for reading a store of {store} again each time: \
             the writer is re-reading what it already holds",
            ROUNDS * 2
        );

        // And what it kept is what the store says: the two accounts of a run
        // written by two processes at once are still one account.
        let served = RunSummary::of(&paths).expect("the run reads");
        assert_eq!(served.event_count as usize, 401 + ROUNDS * 2);
        std::fs::remove_file(paths.summary()).expect("the document");
        assert_eq!(
            served,
            RunSummary::of(&paths).expect("the run folds"),
            "two writers left the maintained row and the folded row apart"
        );
        std::fs::remove_dir_all(&root).ok();
    }

    /// A bounded listing names the roots it could not read, on the same terms
    /// the folding survey does.
    #[test]
    fn a_listing_reports_the_roots_it_could_not_read() {
        let root = scratch("listing");
        recorded(&root, "readable", 3);
        recorded(&root, "also-readable", 3);
        std::fs::create_dir_all(root.join("half-written")).expect("a run root with no launch");

        let listing = Listing::of(&root);
        let named: Vec<&str> = listing
            .summaries
            .iter()
            .map(|row| row.run_id.as_str())
            .collect();
        assert_eq!(named.len(), 2, "{named:?}");
        assert!(named.contains(&"readable"));
        let survey = crate::views::Survey::of(&root);
        assert_eq!(
            listing
                .skipped
                .iter()
                .map(|root| (root.path.clone(), root.reason.clone()))
                .collect::<Vec<_>>(),
            survey
                .skipped
                .iter()
                .map(|root| (root.path.clone(), root.reason.clone()))
                .collect::<Vec<_>>(),
            "the bounded listing and the survey report different refusals"
        );
        std::fs::remove_dir_all(&root).ok();
    }

    /// The checked-in shape of a schema-1 document.
    ///
    /// Read rather than restated: this is the wire a consumer parses, and the
    /// only thing that stops a field being renamed, an absence becoming a zero,
    /// or the version moving without anyone deciding to move it.
    const GOLDEN: &str = include_str!("../tests/golden/run-summary-v1.json");

    /// The document the golden pins, built through the types.
    ///
    /// Every absence policy the launch record establishes is on it at least
    /// once, because the wire is where an absence either survives or becomes a
    /// zero: this run's record named no host, no pid, no stamp, and no launch
    /// instant, and it is the record 141 roots on one host actually hold.
    fn golden() -> RunSummary {
        RunSummary {
            schema_version: SUMMARY_SCHEMA_VERSION,
            run_id: "golden".into(),
            last_write_at: Some(1_786_000_000_000),
            last_event_kind: Some("node-settled".into()),
            event_count: 42,
            node_counts: BTreeMap::from([("done".to_string(), 2), ("failed".to_string(), 1)]),
            stop_recorded: false,
            graph_complete: true,
            decisions_pending: 0,
            surfaces_queued: 2,
            surfaces_read: 1,
            awaiting_human_action: false,
            project: "plans:golden".into(),
            launcher: "claude-code".into(),
            // Unattributed: the launch record named no session, which is the
            // `[unknown]` owner every view has always printed for one.
            session: String::new(),
            started_at: None,
            pid: None,
            host: None,
            started: None,
            timing: serde_json::from_str(include_str!("../tests/golden/telemetry-v2.json"))
                .expect("the telemetry golden reads back into the types"),
            journal_len: 8_192,
            journal_mtime_ms: 1_786_000_000_100,
        }
    }

    #[test]
    fn a_schema_1_document_is_the_shape_the_golden_pins() {
        let rendered = serde_json::to_string_pretty(&golden()).expect("it serialises");
        assert_eq!(
            rendered.trim(),
            GOLDEN.trim(),
            "the summary document changed shape. If that was deliberate, bump \
             SUMMARY_SCHEMA_VERSION and update tests/golden/run-summary-v1.json together"
        );
    }

    #[test]
    fn a_schema_1_document_round_trips_and_a_version_this_build_does_not_read_is_refused() {
        let read: RunSummary =
            serde_json::from_str(GOLDEN).expect("the golden reads back into the types");
        assert_eq!(read, golden());
        // Every absence survives the round trip as an absence, which is the one
        // thing the wire can quietly turn into a measurement.
        assert_eq!(read.started_at, None);
        assert_eq!(read.pid, None);
        assert_eq!(read.host, None);
        assert_eq!(read.started, None);
        assert!(read.session.is_empty());

        // And a document from a schema this build does not read is refused
        // rather than read as one it does — which is a run that folds, not a run
        // that vanishes.
        let mut later: serde_json::Value = serde_json::from_str(GOLDEN).expect("it parses");
        later["schema_version"] = json!(SUMMARY_SCHEMA_VERSION + 1);
        let refused = serde_json::from_value::<RunSummary>(later).expect_err("it is refused");
        assert!(
            refused.to_string().contains("schema_version"),
            "the refusal does not name the version: {refused}"
        );
    }

    /// A run whose summary is a version this build does not read folds, and the
    /// fold replaces the document it could not read.
    #[test]
    fn a_summary_from_a_schema_this_build_does_not_read_folds_rather_than_vanishes() {
        let root = scratch("later-schema");
        let paths = recorded(&root, "demo", 5);
        let mut document: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(paths.summary()).expect("the document"))
                .expect("a summary");
        document["schema_version"] = json!(SUMMARY_SCHEMA_VERSION + 1);
        std::fs::write(paths.summary(), document.to_string()).expect("a later build's summary");

        let served = RunSummary::of(&paths).expect("the run reads");
        assert_eq!(served.schema_version, SUMMARY_SCHEMA_VERSION);
        assert_eq!(served.event_count, 6);
        std::fs::remove_dir_all(&root).ok();
    }

    /// One record of a run's own, built by hand where the writer's own stream
    /// would not do — a relayed record, or one stamped out of order.
    fn event(kind: PipelineKind, run: &str, stream: &str, seq: u64) -> Envelope {
        Envelope {
            v: ENVELOPE_VERSION,
            ts: sys::now_rfc3339(),
            stream: stream.to_string(),
            seq,
            source: Source::Pipeline,
            kind: EventKind(kind.as_str().into()),
            phase: None,
            labels: Labels {
                run_id: Some(run.to_string()),
                node: Some("build".into()),
                ..Labels::default()
            },
            payload: crate::journal::payload(&[("status", json!("done"))]),
            artifacts: Vec::new(),
        }
    }
}
