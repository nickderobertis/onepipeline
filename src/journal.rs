//! The run journal: the merged three-stream event store, and what writes it.
//!
//! One `events.jsonl` per run holds every envelope the run produced — this
//! crate's own, plus the `oneagentgraph` and `onevcs` envelopes it relays — so
//! there is one ordered record a view and a replay both read the same way.
//!
//! The journal is append-only and the engine loop is its only writer of graph
//! state. Reading is unlocked and takes no lock a writer needs, which is what
//! lets every view run beside a live run.

use std::collections::{BTreeMap, VecDeque};
use std::path::Path;

use serde_json::{json, Map, Value};

use crate::error::Result;
use crate::event::{Envelope, Labels, Source, ENVELOPE_VERSION};
use crate::ledger::{self, RunPaths};
use crate::sys;

pub use crate::event::PipelineKind;

/// The append-only writer for one run.
///
/// It holds the next sequence number for this process's stream, which is taken
/// above every line already claiming one — including a line written by a schema
/// this build cannot read, because a record's readability and its claim on a
/// sequence number are different questions.
#[derive(Debug)]
pub struct Journal {
    paths: RunPaths,
    stream: String,
    next_seq: u64,
}

impl Journal {
    /// Open the run's journal for appending.
    pub fn open(paths: &RunPaths) -> Self {
        let stream = format!("{}-{}", sys::hostname(), sys::pid());
        let next_seq = ledger::read_lines(&paths.journal())
            .iter()
            .filter_map(|line| serde_json::from_str::<Value>(line).ok())
            .filter(|value| value.get("stream").and_then(Value::as_str) == Some(stream.as_str()))
            .filter_map(|value| value.get("seq").and_then(Value::as_u64))
            .max()
            .map_or(0, |max| max + 1);
        Self {
            paths: paths.clone(),
            stream,
            next_seq,
        }
    }

    /// Append one of this crate's own events.
    pub fn emit(
        &mut self,
        kind: PipelineKind,
        labels: Labels,
        payload: Map<String, Value>,
    ) -> Result<()> {
        let envelope = Envelope {
            v: ENVELOPE_VERSION,
            ts: sys::now_rfc3339(),
            stream: self.stream.clone(),
            seq: self.next_seq,
            source: Source::Pipeline,
            kind: kind.into(),
            labels,
            payload,
            artifacts: Vec::new(),
        };
        self.next_seq += 1;
        self.append(&envelope)
    }

    /// Append an envelope a sibling library produced, as it produced it.
    ///
    /// A relayed envelope keeps its own `stream`, `seq`, and `source`: the merge
    /// is an interleaving of three streams, not a rewriting of two of them, and
    /// per-stream `seq` gaps are how a consumer detects loss.
    ///
    /// This is also **ingest**: the envelope is arriving from a process this
    /// crate started, which is the one moment a path it names carries the
    /// producer's authority rather than the journal's. So the evidence a
    /// settlement points at is copied into the run's own storage here, and every
    /// reader afterwards opens that copy instead of following the line. See
    /// [`crate::report::retain`].
    pub fn relay(&mut self, envelope: &Envelope) -> Result<()> {
        crate::report::retain(&self.paths, envelope);
        self.append(envelope)
    }

    fn append(&self, envelope: &Envelope) -> Result<()> {
        let line = serde_json::to_string(envelope)
            .map_err(|e| crate::error::Error::Invalid(format!("event: {e}")))?;
        ledger::append_line(&self.paths.journal(), &line)
    }
}

/// Labels naming a run, and optionally a node within it.
///
/// No round: execution is continuous, so the reserved `round` key is never
/// stamped — see [`Labels::round`](crate::event::Labels::round).
pub fn labels(run: &str, node: Option<&str>) -> Labels {
    Labels {
        run_id: Some(run.to_string()),
        node: node.map(str::to_string),
        ..Labels::default()
    }
}

/// A payload built from key/value pairs, in the order they are given.
pub fn payload(fields: &[(&str, Value)]) -> Map<String, Value> {
    let mut map = Map::new();
    for (key, value) in fields {
        map.insert((*key).to_string(), value.clone());
    }
    map
}

/// Every envelope in a run's journal, in the order it was appended.
///
/// A line this build cannot parse is skipped rather than ending the read: a
/// reader skips records from a version it does not know rather than failing the
/// run it is observing. A line holding a **fragment and then a whole record** —
/// the shape a writer that died mid-record used to leave behind — hands back the
/// whole one: it is a record the store really holds, and losing it was how the
/// event reporting that writer's death disappeared.
pub fn read(path: &Path) -> Vec<Envelope> {
    ledger::read_records(path)
        .iter()
        .filter_map(|record| match reading(record) {
            Reading::Whole(envelope) => Some(envelope),
            Reading::Glued { envelope, .. } => Some(envelope),
            Reading::Blank | Reading::Truncated | Reading::Unparseable => None,
        })
        .collect()
}

/// Whether the journal holds a line this build could not read.
///
/// Strict replay needs to know: an unreadable line might have been an
/// authoritative graph mutation, so a reader that folds one reports rather than
/// guesses. Every class of loss counts, a fragment as much as a record from a
/// schema this build does not know — what strict replay is about is that
/// *something* the graph may have turned on is not there.
pub fn has_unreadable_lines(path: &Path) -> bool {
    ledger::read_records(path).iter().any(|record| {
        matches!(
            reading(record),
            Reading::Glued { .. } | Reading::Truncated | Reading::Unparseable
        )
    })
}

/// What one line of the journal turned out to be.
enum Reading {
    /// A whole record this build reads.
    Whole(Envelope),
    /// Nothing at all: a blank line, which every reader here has always skipped.
    Blank,
    /// A fragment, and then a whole record appended after it by another process
    /// — one line holding two, glued where the first writer stopped.
    Glued {
        /// How many bytes of the line are the fragment.
        lost: u64,
        /// The whole record after it.
        envelope: Envelope,
    },
    /// A record whose writer did not finish it.
    Truncated,
    /// A whole line this build cannot read: a record from a schema it does not
    /// know, or something that is not a record at all.
    Unparseable,
}

/// Which of the five a line is.
///
/// The distinction a reader could not previously draw. An unterminated final
/// line is a fragment whatever its parse says — the writer had not finished it —
/// and among the terminated ones `serde_json`'s own `is_eof` separates a record
/// that stops early from one that is whole and unreadable.
// llmlint: ignore-block[boundary_inputs_validated] the store is this crate's own record and not external input, and `docs/contract.md` is explicit about how it is read: a relayed envelope's kind is a wire string this library never rejects, and a record from a version this build does not know is *skipped and reported* rather than refused. `deny_unknown_fields` here would turn a newer build's record — the case this reader exists to name — into a parse failure indistinguishable from a torn one, and refusing an unknown `v` would do the same.
fn reading(record: &ledger::Record) -> Reading {
    if record.text.trim().is_empty() {
        return Reading::Blank;
    }
    // The terminator first, and before the parse, because a record is finished
    // when its newline lands and not before: an append writes the record and its
    // terminator in one call, so a line that parses whole and ends without one
    // is a write that stopped in the middle — and the next append discards it as
    // exactly that. A reader that counted it as a record would hand back a
    // record the store is about to say it lost.
    if !record.terminated {
        return Reading::Truncated;
    }
    match serde_json::from_str::<Envelope>(&record.text) {
        Ok(envelope) => Reading::Whole(envelope),
        Err(e) => match glued_tail(&record.text) {
            Some((lost, envelope)) => Reading::Glued { lost, envelope },
            None if e.is_eof() => Reading::Truncated,
            None => Reading::Unparseable,
        },
    }
}

/// The whole record hiding after a fragment on one line, and how much of the
/// line was the fragment.
///
/// Only ever asked of a line that did not parse, and only at the positions a
/// record of this store can start at. Every envelope written here is serialized
/// from the same struct, whose first field is `v`, so the needle is the whole of
/// that opening — searching every `{` instead would try a parse per brace, and a
/// payload has a brace per field.
fn glued_tail(text: &str) -> Option<(u64, Envelope)> {
    text.match_indices("{\"v\":")
        .skip_while(|(at, _)| *at == 0)
        .find_map(|(at, _)| {
            serde_json::from_str::<Envelope>(&text[at..])
                .ok()
                .map(|envelope| (at as u64, envelope))
        })
}
// llmlint: ignore-end[boundary_inputs_validated]

/// One record a run's store does not hold whole, and where it is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Loss {
    /// The 1-based line it is on.
    pub line: usize,
    /// Where in the file that line begins.
    pub offset: u64,
    /// How many bytes of it are not a record this build can read.
    pub bytes: u64,
}

/// What a read of one run's journal found that is not a whole record.
///
/// The three are deliberately distinct, because they call for different things:
/// a **truncated** record is a writer that died mid-line and is a loss this run
/// really suffered; an **unparseable** one is very often a record from a build
/// newer than this one, and nothing is missing but this reader's ability to read
/// it; a **healed** one is a fragment an append has already cut away, which no
/// reader will ever see in the file again and which is the whole reason it is
/// recorded beside it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Integrity {
    /// Records whose writer did not finish them.
    pub truncated: Vec<Loss>,
    /// Whole lines this build cannot read.
    pub unparseable: Vec<Loss>,
    /// Fragments an append healed out of the file, as the ledger recorded them.
    pub healed: Vec<ledger::TornTail>,
}

impl Integrity {
    /// Whether the store holds every record it was ever written, whole.
    pub fn is_whole(&self) -> bool {
        self.truncated.is_empty() && self.unparseable.is_empty() && self.healed.is_empty()
    }

    /// How a view says what the store lost, or nothing when it lost nothing.
    ///
    /// Counted **and** placed. A count alone tells a reader something is wrong
    /// and not where to look, and the journal is what every other view is folded
    /// from — a loss inside it is what makes the rest of them unprovable.
    pub fn phrase(&self) -> String {
        if self.is_whole() {
            return String::new();
        }
        let mut said: Vec<String> = Vec::new();
        // Both spellings, rather than an `s` on the end of one: the plural of
        // "line this build cannot read" is not that word with an `s` after it.
        for (losses, one, many) in [
            (&self.truncated, "truncated record", "truncated records"),
            (
                &self.unparseable,
                "line this build cannot read",
                "lines this build cannot read",
            ),
        ] {
            if losses.is_empty() {
                continue;
            }
            let placed: Vec<String> = losses
                .iter()
                .map(|loss| {
                    format!(
                        "line {} at byte {} ({} bytes)",
                        loss.line, loss.offset, loss.bytes
                    )
                })
                .collect();
            said.push(format!(
                "{} {}: {}",
                losses.len(),
                if losses.len() == 1 { one } else { many },
                placed.join(", ")
            ));
        }
        if !self.healed.is_empty() {
            let placed: Vec<String> = self
                .healed
                .iter()
                .map(|torn| format!("byte {} ({} bytes)", torn.offset, torn.bytes))
                .collect();
            said.push(format!(
                "{} fragment{} discarded at append: {}",
                self.healed.len(),
                if self.healed.len() == 1 { "" } else { "s" },
                placed.join(", ")
            ));
        }
        said.join("; ")
    }
}

/// What one run's journal holds that is not a whole record.
///
/// Read from the file rather than from the folded events, because what this is
/// about is exactly the records the fold never saw.
pub fn integrity(path: &Path) -> Integrity {
    let mut integrity = Integrity {
        healed: ledger::torn_tails(path),
        ..Integrity::default()
    };
    for record in ledger::read_records(path) {
        let bytes = record.bytes;
        match reading(&record) {
            Reading::Whole(_) | Reading::Blank => {}
            Reading::Glued { lost, .. } => integrity.truncated.push(Loss {
                line: record.line,
                offset: record.offset,
                bytes: lost,
            }),
            Reading::Truncated => integrity.truncated.push(Loss {
                line: record.line,
                offset: record.offset,
                bytes,
            }),
            Reading::Unparseable => integrity.unparseable.push(Loss {
                line: record.line,
                offset: record.offset,
                bytes,
            }),
        }
    }
    integrity
}

/// Merge order: each stream in its own `seq`, interleaved between streams by
/// `ts`.
///
/// `seq` is the producer's own statement of the order it wrote things in, and
/// the only ordering promise an envelope carries. `ts` is a wall clock — not
/// this process's, and not monotonic, since a host clock can be stepped under a
/// running producer — so ordering a stream by it swaps that stream's own records
/// against the only party that knew. Between streams nothing is promised beyond
/// the timestamps, so a `ts` tie there is broken by stream id: deterministic
/// rather than meaningful.
pub fn merge_order(events: &mut [Envelope]) {
    let mut merged: Vec<Envelope> = merged_order(events)
        .into_iter()
        .map(|index| events[index].clone())
        .collect();
    events.swap_with_slice(&mut merged);
}

/// Where each record belongs in the merged order, as indices into `events`.
///
/// A k-way merge rather than one sort: the order [`merge_order`] states is not a
/// total order over the fields — `seq` within a stream, `ts` between them — and
/// `sort_by` given an inconsistent comparator produces an order nobody
/// specified.
fn merged_order(events: &[Envelope]) -> Vec<usize> {
    let mut streams: BTreeMap<&str, VecDeque<usize>> = BTreeMap::new();
    for (index, event) in events.iter().enumerate() {
        streams
            .entry(event.stream.as_str())
            .or_default()
            .push_back(index);
    }
    for queue in streams.values_mut() {
        let mut ordered: Vec<usize> = queue.iter().copied().collect();
        // Stable, so two records of one stream claiming one `seq` — which only
        // a producer in error emits — keep the order they were appended in
        // rather than being reordered by a tie-break that means nothing.
        ordered.sort_by_key(|index| events[*index].seq);
        *queue = ordered.into();
    }

    let mut order = Vec::with_capacity(events.len());
    // Each pass takes the earliest record still at the head of any stream. A
    // stream's head is its next `seq`, so its own order is never in question;
    // what this decides is only which stream goes next.
    while let Some(stream) = streams
        .iter()
        .filter_map(|(stream, queued)| queued.front().map(|index| (&events[*index].ts, *stream)))
        .min()
        .map(|(_, stream)| stream)
    {
        let index = streams
            .get_mut(stream)
            .and_then(VecDeque::pop_front)
            .expect("the stream this pass chose has a head");
        order.push(index);
    }
    order
}

/// The `run-stopped` payload field naming what its teardown established.
///
/// Named once because `stop` writes it and the projection reads it, and a run
/// whose two sides disagree about this field reports work as ended that is still
/// running.
pub const STOP_TEARDOWN: &str = "teardown";

/// What a `run-stopped` record says its teardown established about the run's
/// processes.
///
/// A closed set on the wire as well as in the code: an unknown value is not a
/// fourth meaning to guess at, and [`StopTeardown::of`] reads one as the
/// conservative answer rather than the convenient one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum StopTeardown {
    /// The run's process tree was listed in full and every process in it was
    /// signalled.
    Signalled,
    /// This host gave no listing the tree could be read from, so nothing was
    /// signalled and the run was left as it was.
    NotAttempted,
    /// The tree was listed and part of it was signalled; at least one process in
    /// it could not be, and is still running.
    PartlySignalled,
    /// The run's driver is on another host, so this one attempted nothing and
    /// has nothing to say about its processes.
    Elsewhere,
}

impl StopTeardown {
    /// What a `run-stopped` payload says, read defensively.
    ///
    /// A record with no such field is one written before it existed, and those
    /// stops signalled a tree they had read: [`Signalled`](Self::Signalled). A
    /// record that *has* the field and says something this build does not know
    /// is a newer writer describing an outcome this one cannot interpret, and
    /// the safe reading of an uninterpretable teardown is the most cautious one
    /// this build has — never that the run's workers were reached.
    pub fn of(payload: &Map<String, Value>) -> Self {
        match payload.get(STOP_TEARDOWN) {
            None => Self::Signalled,
            Some(value) => serde_json::from_value(value.clone()).unwrap_or(Self::NotAttempted),
        }
    }
}

/// The `node-settled` payload field saying whether the node's change reached
/// its base branch.
///
/// Named once because `engine::settle` writes it and `projection` reads it, and
/// a run whose two sides disagree about this field is a run that reports work as
/// landed on the strength of nothing. Absent on a settlement that published no
/// change of its own — see [`crate::graph::Landing`].
pub const SETTLED_LANDING: &str = "landing";

/// The payload of a `node-settled` event, as the projection folds it.
pub fn settled_payload(
    status: &str,
    outcome: Option<&str>,
    detail: Option<&str>,
) -> Map<String, Value> {
    let mut fields = vec![("status", json!(status))];
    if let Some(outcome) = outcome {
        fields.push(("outcome", json!(outcome)));
    }
    if let Some(detail) = detail {
        fields.push(("detail", json!(detail)));
    }
    payload(&fields)
}

#[cfg(test)]
mod tests {

    /// What a `run-stopped` record this build cannot interpret is taken to mean.
    #[test]
    fn an_uninterpretable_teardown_is_never_read_as_a_clean_stop() {
        assert_eq!(
            StopTeardown::of(&payload(&[])),
            StopTeardown::Signalled,
            "a record written before the field existed should read as the stop it was"
        );
        for (name, said) in [
            ("a kind this build has never seen", json!("swept")),
            ("a value of the wrong shape", json!(true)),
            ("nothing at all", json!(null)),
        ] {
            assert_eq!(
                StopTeardown::of(&payload(&[(STOP_TEARDOWN, said)])),
                StopTeardown::NotAttempted,
                "{name} was read as an outcome this build understands"
            );
        }
        assert_eq!(
            StopTeardown::of(&payload(&[(STOP_TEARDOWN, json!("elsewhere"))])),
            StopTeardown::Elsewhere
        );
    }

    use super::*;
    use crate::event::EventKind;
    use std::fs;
    use std::path::PathBuf;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("onepipeline-journal-{name}-{}", sys::pid()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("a scratch root");
        dir
    }

    #[test]
    fn a_reopened_journal_takes_its_next_sequence_above_what_it_wrote() {
        let root = scratch("seq");
        let paths = RunPaths::under(&root, "demo");
        paths.create().expect("the run directory");

        let mut journal = Journal::open(&paths);
        journal
            .emit(PipelineKind::RunStarted, labels("demo", None), payload(&[]))
            .expect("appended");
        journal
            .emit(
                PipelineKind::NodeReady,
                labels("demo", Some("build")),
                payload(&[]),
            )
            .expect("appended");

        let reopened = Journal::open(&paths);
        assert_eq!(reopened.next_seq, 2, "a reopened journal replayed a seq");

        let events = read(&paths.journal());
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].seq, 0);
        assert_eq!(events[1].seq, 1);
        assert!(events.iter().all(|e| e.source == Source::Pipeline));
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_line_this_build_cannot_read_is_skipped_but_still_reported() {
        let root = scratch("unreadable");
        let paths = RunPaths::under(&root, "demo");
        paths.create().expect("the run directory");
        let mut journal = Journal::open(&paths);
        journal
            .emit(PipelineKind::RunStarted, labels("demo", None), payload(&[]))
            .expect("appended");
        ledger::append_line(&paths.journal(), r#"{"v":99,"from":"the future"}"#).expect("appended");

        assert_eq!(read(&paths.journal()).len(), 1, "the future line was read");
        assert!(has_unreadable_lines(&paths.journal()));
        fs::remove_dir_all(&root).ok();
    }

    /// A truncated record, an unreadable line, and a line holding both a
    /// fragment and a whole record are three different findings.
    ///
    /// The distinction nothing here could draw. `read` filtered all three away
    /// and the only hook was a bool — so a run that lost the record reporting
    /// its own driver's death rendered as a run that had simply been quiet.
    #[test]
    fn a_reader_tells_the_three_ways_a_line_is_not_a_record_apart() {
        let root = scratch("integrity");
        let paths = RunPaths::under(&root, "demo");
        paths.create().expect("the run directory");
        let mut journal = Journal::open(&paths);
        journal
            .emit(PipelineKind::RunStarted, labels("demo", None), payload(&[]))
            .expect("appended");
        journal
            .emit(
                PipelineKind::NodeReady,
                labels("demo", Some("build")),
                payload(&[]),
            )
            .expect("appended");

        // The store as a build carrying no healing left it: the second record
        // is a fragment with a whole record glued onto it, which is what a
        // second process appending after a dying writer produced.
        let text = ledger::read_lines(&paths.journal());
        let fragment = r#"{"v":1,"ts":"2026-08-16T00:00:00.000Z","strea"#;
        std::fs::write(
            paths.journal(),
            format!(
                "{}\n{fragment}{}\n{{\"v\":99,\"from\":\"a newer build\"}}\n{}",
                text[0], text[1], r#"{"v":1,"ts":"2026-08-16T"#
            ),
        )
        .expect("the store is written");

        let integrity = integrity(&paths.journal());
        assert!(!integrity.is_whole());
        assert_eq!(
            integrity.truncated.len(),
            2,
            "a fragment was not counted as one: {integrity:?}"
        );
        // The glued line: the loss is the fragment in front of the record, not
        // the whole line, and it is placed where the line begins.
        assert_eq!(integrity.truncated[0].line, 2);
        assert_eq!(integrity.truncated[0].offset, text[0].len() as u64 + 1);
        assert_eq!(integrity.truncated[0].bytes, fragment.len() as u64);
        // The unterminated last line: a record whose writer did not finish it.
        assert_eq!(integrity.truncated[1].line, 4);
        assert_eq!(integrity.unparseable.len(), 1, "{integrity:?}");
        assert_eq!(integrity.unparseable[0].line, 3);

        // The record the tear used to destroy is handed back whole, because it
        // is whole: only what is in front of it is not.
        let events = read(&paths.journal());
        assert_eq!(events.len(), 2, "the glued record was thrown away");
        assert_eq!(events[1].kind.0, "node-ready");
        assert_eq!(events[1].labels.node.as_deref(), Some("build"));

        // Strict replay still reports every one of them, as it always did.
        assert!(has_unreadable_lines(&paths.journal()));
        std::fs::remove_dir_all(&root).ok();
    }

    /// A store nothing has torn says nothing, and one that has been healed says
    /// what it cost even though a reader can no longer see it.
    #[test]
    fn a_healed_loss_is_reported_although_the_store_no_longer_holds_it() {
        let root = scratch("integrity-healed");
        let paths = RunPaths::under(&root, "demo");
        paths.create().expect("the run directory");
        let mut journal = Journal::open(&paths);
        journal
            .emit(PipelineKind::RunStarted, labels("demo", None), payload(&[]))
            .expect("appended");
        assert!(integrity(&paths.journal()).is_whole());
        assert!(integrity(&paths.journal()).phrase().is_empty());

        let whole = std::fs::read_to_string(paths.journal()).expect("the store reads");
        std::fs::write(paths.journal(), format!("{whole}{{\"half\":")).expect("written");
        journal
            .emit(PipelineKind::NodeReady, labels("demo", None), payload(&[]))
            .expect("appended");

        let integrity = integrity(&paths.journal());
        assert!(
            integrity.truncated.is_empty() && integrity.unparseable.is_empty(),
            "the store still holds the fragment: {integrity:?}"
        );
        assert!(!integrity.is_whole(), "the healed loss went unreported");
        assert_eq!(
            integrity.phrase(),
            format!(
                "1 fragment discarded at append: byte {} (8 bytes)",
                whole.len()
            )
        );
        std::fs::remove_dir_all(&root).ok();
    }

    /// How the count and the position read when there is more than one of each.
    #[test]
    fn the_phrase_counts_and_places_every_loss_it_reports() {
        let losses = Integrity {
            truncated: vec![
                Loss {
                    line: 2,
                    offset: 40,
                    bytes: 12,
                },
                Loss {
                    line: 9,
                    offset: 900,
                    bytes: 3,
                },
            ],
            unparseable: vec![Loss {
                line: 4,
                offset: 120,
                bytes: 30,
            }],
            healed: Vec::new(),
        };
        assert_eq!(
            losses.phrase(),
            "2 truncated records: line 2 at byte 40 (12 bytes), line 9 at byte 900 (3 bytes); \
             1 line this build cannot read: line 4 at byte 120 (30 bytes)"
        );
    }

    #[test]
    fn a_relayed_envelope_keeps_its_own_stream_and_source() {
        let root = scratch("relay");
        let paths = RunPaths::under(&root, "demo");
        paths.create().expect("the run directory");
        let mut journal = Journal::open(&paths);

        let relayed = Envelope {
            v: ENVELOPE_VERSION,
            ts: "2026-08-08T00:00:00.000Z".into(),
            stream: "oneagentgraph-1".into(),
            seq: 7,
            source: Source::Agentgraph,
            kind: EventKind("turn-finished".into()),
            labels: labels("demo", Some("build")),
            payload: payload(&[]),
            artifacts: Vec::new(),
        };
        journal.relay(&relayed).expect("relayed");

        let events = read(&paths.journal());
        assert_eq!(events[0].source, Source::Agentgraph);
        assert_eq!(events[0].stream, "oneagentgraph-1");
        assert_eq!(events[0].seq, 7, "the relay renumbered a sibling's stream");
        fs::remove_dir_all(&root).ok();
    }

    fn event(ts: &str, stream: &str, seq: u64) -> Envelope {
        Envelope {
            v: ENVELOPE_VERSION,
            ts: ts.into(),
            stream: stream.into(),
            seq,
            source: Source::Pipeline,
            kind: EventKind("k".into()),
            labels: Labels::default(),
            payload: Map::new(),
            artifacts: Vec::new(),
        }
    }

    fn merged(mut events: Vec<Envelope>) -> Vec<(String, u64)> {
        merge_order(&mut events);
        events
            .into_iter()
            .map(|event| (event.stream, event.seq))
            .collect()
    }

    #[test]
    fn the_merge_interleaves_streams_by_timestamp() {
        assert_eq!(
            merged(vec![
                event("2026-08-08T00:00:02.000Z", "b", 1),
                event("2026-08-08T00:00:03.000Z", "a", 1),
                event("2026-08-08T00:00:00.000Z", "a", 0),
                event("2026-08-08T00:00:01.000Z", "b", 0),
            ]),
            vec![
                ("a".to_string(), 0),
                ("b".to_string(), 0),
                ("b".to_string(), 1),
                ("a".to_string(), 1),
            ],
            "the streams did not interleave by the clock"
        );
    }

    #[test]
    fn a_timestamp_collision_between_streams_is_broken_by_the_stream_id() {
        // Nothing promises anything about the order of two records from
        // different producers written inside one clock tick, so the tie-break is
        // deterministic rather than meaningful — but it does have to be *some*
        // one order, or two readings of one store disagree.
        let tick = "2026-08-08T00:00:00.000Z";
        assert_eq!(
            merged(vec![
                event(tick, "b", 0),
                event(tick, "a", 0),
                event(tick, "a", 1),
                event(tick, "b", 1),
            ]),
            vec![
                ("a".to_string(), 0),
                ("a".to_string(), 1),
                ("b".to_string(), 0),
                ("b".to_string(), 1),
            ]
        );
    }

    /// A stream's own sequence survives timestamps that do not agree with it.
    ///
    /// `ts` is a wall clock, and a wall clock is neither monotonic nor this
    /// process's: a host clock stepped under a running producer stamps a later
    /// record with an earlier reading. `seq` is that producer saying what order
    /// it wrote things in, and ordering its records by anything else discards
    /// the only ordering fact the envelope actually carries.
    #[test]
    fn a_streams_own_sequence_outranks_its_timestamps() {
        assert_eq!(
            merged(vec![
                event("2026-08-08T00:00:02.000Z", "a", 0),
                // The clock went backwards between these two, and between these
                // two only. Ordered by `ts` the stream reads 1, 2, 0.
                event("2026-08-08T00:00:00.000Z", "a", 1),
                event("2026-08-08T00:00:01.000Z", "a", 2),
            ]),
            vec![
                ("a".to_string(), 0),
                ("a".to_string(), 1),
                ("a".to_string(), 2),
            ],
            "the merge reordered a stream against the sequence its producer stamped"
        );
    }

    #[test]
    fn two_records_of_one_stream_claiming_one_sequence_keep_the_order_they_arrived_in() {
        // A producer in error, so there is nothing to be right about beyond
        // being stable: the store must not shuffle under a second reading.
        let mut events = vec![
            event("2026-08-08T00:00:01.000Z", "a", 0),
            event("2026-08-08T00:00:00.000Z", "a", 0),
        ];
        merge_order(&mut events);
        let timestamps: Vec<String> = events.iter().map(|event| event.ts.clone()).collect();
        assert_eq!(
            timestamps,
            vec![
                "2026-08-08T00:00:01.000Z".to_string(),
                "2026-08-08T00:00:00.000Z".to_string(),
            ]
        );
    }

    #[test]
    fn merging_nothing_is_nothing() {
        assert!(merged(Vec::new()).is_empty());
    }

    #[test]
    fn a_settled_payload_omits_the_fields_it_was_not_given() {
        let bare = settled_payload("done", None, None);
        assert_eq!(bare.len(), 1);
        let full = settled_payload("failed", Some("infrastructure-failure"), Some("OOM"));
        assert_eq!(full["outcome"], json!("infrastructure-failure"));
        assert_eq!(full["detail"], json!("OOM"));
    }
}
