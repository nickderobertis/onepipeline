//! The run journal: the merged three-stream event store, and what writes it.
//!
//! One `events.jsonl` per run holds every envelope the run produced — this
//! crate's own, plus the `oneagentgraph` and `onevcs` envelopes it relays — so
//! there is one ordered record a view, a replay, and a round transition all read
//! the same way.
//!
//! The journal is append-only and the engine verbs are its only writer. Reading
//! is unlocked and takes no lock a writer needs, which is what lets every view
//! run beside a live round.

use std::path::Path;

use serde_json::{json, Map, Value};

use crate::error::Result;
use crate::event::{Envelope, EventKind, Labels, Source, ENVELOPE_VERSION};
use crate::ledger::{self, RunPaths};
use crate::sys;

/// The run was launched.
pub const RUN_STARTED: &str = "run-started";
/// A round began executing.
pub const ROUND_STARTED: &str = "round-started";
/// A round stopped executing, with the state it settled in.
pub const ROUND_FINISHED: &str = "round-finished";
/// A node's dispatch was started.
pub const NODE_DISPATCHED: &str = "node-dispatched";
/// A node reached a terminal status for this round.
pub const NODE_SETTLED: &str = "node-settled";
/// A live edit was accepted and applied to the desired graph.
pub const EDIT_COMMITTED: &str = "edit-committed";
/// A live edit was refused, with the reason its submitter was told.
pub const EDIT_REJECTED: &str = "edit-rejected";
/// A surface was *sent*. Delivery is a separate fact.
pub const PLANNER_SURFACE_QUEUED: &str = "planner-surface-queued";
/// A surface was *consumed* by the planner. This is what resets the pacemaker.
pub const PLANNER_SURFACED: &str = "planner-surfaced";
/// The planner answered a consumed surface.
pub const PLANNER_REPLIED: &str = "planner-replied";
/// A human action was attested.
pub const HUMAN_ATTESTED: &str = "human-attested";
/// A fresh driver was attached to an intact ledger.
pub const DRIVER_ADOPTED: &str = "driver-adopted";
/// The run was ended by `stop`.
pub const RUN_STOPPED: &str = "run-stopped";
/// An in-flight dispatch recorded nothing past the stall threshold.
pub const QUIET_WORKER: &str = "quiet-worker";
/// A round exceeded its budget and cooperatively cancelled its workers.
pub const ROUND_BUDGET_EXCEEDED: &str = "round-budget-exceeded";
/// A request a round boundary depends on was retried.
pub const BOUNDARY_RETRIED: &str = "boundary-retried";
/// A cross-DAG upstream advanced after its consumer recorded it.
pub const UPSTREAM_MODIFIED: &str = "upstream-modified";
/// The planner requested completion, independently of graph mutation.
pub const COMPLETION_REQUESTED: &str = "completion-requested";

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
    pub fn emit(&mut self, kind: &str, labels: Labels, payload: Map<String, Value>) -> Result<()> {
        let envelope = Envelope {
            v: ENVELOPE_VERSION,
            ts: sys::now_rfc3339(),
            stream: self.stream.clone(),
            seq: self.next_seq,
            source: Source::Pipeline,
            kind: EventKind(kind.to_string()),
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
    pub fn relay(&mut self, envelope: &Envelope) -> Result<()> {
        self.append(envelope)
    }

    fn append(&self, envelope: &Envelope) -> Result<()> {
        let line = serde_json::to_string(envelope)
            .map_err(|e| crate::error::Error::Invalid(format!("event: {e}")))?;
        ledger::append_line(&self.paths.journal(), &line)
    }
}

/// Labels naming a run, and optionally a round and a node within it.
pub fn labels(run: &str, round: Option<u64>, node: Option<&str>) -> Labels {
    Labels {
        run_id: Some(run.to_string()),
        round,
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
/// round it is observing.
pub fn read(path: &Path) -> Vec<Envelope> {
    ledger::read_lines(path)
        .iter()
        .filter_map(|line| serde_json::from_str::<Envelope>(line).ok())
        .collect()
}

/// Whether the journal holds a line this build could not read.
///
/// Strict replay needs to know: an unreadable line might have been an
/// authoritative graph mutation, so a transition that folds one reports rather
/// than guesses.
pub fn has_unreadable_lines(path: &Path) -> bool {
    ledger::read_lines(path)
        .iter()
        .any(|line| serde_json::from_str::<Envelope>(line).is_err())
}

/// Merge order across streams: `(ts, stream, seq)`.
///
/// There are no cross-stream ordering promises beyond the timestamps, so the
/// stream id and sequence break a tie deterministically rather than
/// meaningfully.
pub fn merge_order(events: &mut [Envelope]) {
    events.sort_by(|a, b| {
        a.ts.cmp(&b.ts)
            .then_with(|| a.stream.cmp(&b.stream))
            .then_with(|| a.seq.cmp(&b.seq))
    });
}

/// The payload of a `node-settled` event, as the projection folds it.
pub fn settled_payload(status: &str, outcome: Option<&str>, detail: Option<&str>) -> Map<String, Value> {
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
    use super::*;
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
            .emit(RUN_STARTED, labels("demo", None, None), payload(&[]))
            .expect("appended");
        journal
            .emit(ROUND_STARTED, labels("demo", Some(1), None), payload(&[]))
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
            .emit(RUN_STARTED, labels("demo", None, None), payload(&[]))
            .expect("appended");
        ledger::append_line(&paths.journal(), r#"{"v":99,"from":"the future"}"#)
            .expect("appended");

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
            labels: labels("demo", Some(1), Some("build")),
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

    #[test]
    fn the_merge_orders_by_timestamp_then_stream_then_sequence() {
        let event = |ts: &str, stream: &str, seq: u64| Envelope {
            v: ENVELOPE_VERSION,
            ts: ts.into(),
            stream: stream.into(),
            seq,
            source: Source::Pipeline,
            kind: EventKind("k".into()),
            labels: Labels::default(),
            payload: Map::new(),
            artifacts: Vec::new(),
        };
        let mut events = vec![
            event("2026-08-08T00:00:01.000Z", "b", 0),
            event("2026-08-08T00:00:00.000Z", "b", 1),
            event("2026-08-08T00:00:00.000Z", "b", 0),
            event("2026-08-08T00:00:00.000Z", "a", 9),
        ];
        merge_order(&mut events);
        let seen: Vec<(String, u64)> = events
            .iter()
            .map(|e| (e.stream.clone(), e.seq))
            .collect();
        assert_eq!(
            seen,
            vec![
                ("a".to_string(), 9),
                ("b".to_string(), 0),
                ("b".to_string(), 1),
                ("b".to_string(), 0),
            ]
        );
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
