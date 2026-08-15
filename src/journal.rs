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
/// run it is observing.
pub fn read(path: &Path) -> Vec<Envelope> {
    ledger::read_lines(path)
        .iter()
        .filter_map(|line| serde_json::from_str::<Envelope>(line).ok())
        .collect()
}

/// Whether the journal holds a line this build could not read.
///
/// Strict replay needs to know: an unreadable line might have been an
/// authoritative graph mutation, so a reader that folds one reports rather than
/// guesses.
pub fn has_unreadable_lines(path: &Path) -> bool {
    ledger::read_lines(path)
        .iter()
        .any(|line| serde_json::from_str::<Envelope>(line).is_err())
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
            .expect("the stream this round chose has a head");
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
            .emit(
                PipelineKind::RunStarted,
                labels("demo", None),
                payload(&[]),
            )
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
            .emit(
                PipelineKind::RunStarted,
                labels("demo", None),
                payload(&[]),
            )
            .expect("appended");
        ledger::append_line(&paths.journal(), r#"{"v":99,"from":"the future"}"#).expect("appended");

        assert_eq!(read(&paths.journal()).len(), 1, "the future line was read");
        assert!(has_unreadable_lines(&paths.journal()));
        fs::remove_dir_all(&root).ok();
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
