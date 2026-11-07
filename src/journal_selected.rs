//! A selective read of a run's journal: the records a first screen needs out of a
//! whole journal, each with the place a complete read gives it.
//!
//! Published through [`crate::journal`]. Every line is merged, so a kept record's
//! place is the complete read's, and only what is kept becomes a record.
//!
//! **What the skim rests on.** Every writer of a journal serializes an
//! [`Envelope`] through its one `Serialize`, whose fields are, in this order: `v`,
//! `ts`, `stream`, `seq`, `source`, `kind`, `phase` where there is one, `labels`,
//! `payload`, `artifacts`. A line in exactly that form has its merge fields read off
//! its opening; anything else about a line, a field in another order or a string
//! carrying an escape, sends it to the complete reader's own parse rather than past it.
//!
//! **What it answers as the complete reader does, on every line.** A line holds a place
//! only if the complete reader would hand back a record for it, so each line in the form
//! is also validated to the end, by the same parser and depth and against the envelope's
//! own field types, without building its payload: [`Checked`] is why a line nothing keeps
//! costs its bytes and no tree. A line that fails goes to the complete reader's parse,
//! which answers what it answers: a line holding a fragment and then a whole record is
//! that record, and a blank or unreadable line is no record. A final line no newline
//! ended is no record, whatever it holds and whichever form it is in.

use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::path::Path;

use serde::de::value::{Error as ValueError, StrDeserializer};
use serde::de::{IgnoredAny, MapAccess, SeqAccess, Visitor};
use serde::{Deserialize, Deserializer};

use crate::event::{ArtifactRef, Envelope, Phase, Source};

/// The kinds a first screen does not parse unless its node is asked for: by far the
/// most numerous records of a long run, and nothing a first answer draws.
const SKIPPED_KINDS: [&str; 2] = ["member-heartbeat", "turn-activity"];

/// Fewer items than this are worked through on one thread.
const WORKED_ALONE: usize = 512;

/// What spike-first-screen measured the read at, on a host running other work beside it.
const WORKERS: usize = 4;

/// What a [`read_selected`] keeps of a journal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Selection<'a> {
    /// What a run's first screen is drawn from: every record but `member-heartbeat`
    /// and `turn-activity`, plus each stream's first and last record, the record of
    /// a node before each of that node's records carrying evidence, the journal's
    /// first and last, and, when `node` is named, all of that node's records.
    ///
    /// A record carries evidence when its `artifacts` are not empty, and the record
    /// before it is the one before it among its node's records in the merged store.
    /// This is the selection [`crate::views::RunView::over_selected`] folds.
    FirstScreen {
        /// The node whose records are all kept, whatever their kind.
        node: Option<&'a str>,
    },
    /// One owner's records, plus the journal's first and last: `Some(node)` that
    /// node's, and `None` those with no node.
    Share(Option<&'a str>),
}

/// The records a [`read_selected`] kept, and where they stand in the whole journal.
///
/// "The merged store" below is what a complete read holds: every record of the
/// journal in [`merge_order`](crate::journal::merge_order), less any repeating a
/// `(stream, seq)` already in it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SelectedRecords {
    /// The records kept, in merged order. A repeat of a `(stream, seq)` is kept
    /// where the selection names it, because a complete fold meets it too.
    pub events: Vec<Envelope>,
    /// Each kept record's place in the merged store, parallel to
    /// [`events`](Self::events): `None` for a repeated `(stream, seq)`, which holds
    /// no place there.
    pub places: Vec<Option<usize>>,
    /// That store's length.
    pub store_len: usize,
    /// The highest `seq` in the whole journal.
    pub last_seq: u64,
    /// Per node, each kept record's place among that node's records in the merged
    /// store, in merged order. A repeated `(stream, seq)` has none.
    pub shares: HashMap<String, Vec<usize>>,
}

/// One line of a journal, as far as a selection needs it.
#[derive(Debug)]
struct Skim<'a> {
    line: &'a [u8],
    ts: Cow<'a, str>,
    stream: Cow<'a, str>,
    seq: u64,
    kind: Cow<'a, str>,
    node: Option<Cow<'a, str>>,
    evidence: bool,
    /// The record, where reading this line already parsed it.
    parsed: Option<Box<Envelope>>,
}

/// Bytes `from` the start of `line` up to the closing quote of a string with no
/// escape in it, and what follows that quote.
fn string_of(line: &[u8]) -> Option<(&str, &[u8])> {
    let end = memchr::memchr2(b'"', b'\\', line)?;
    if line[end] != b'"' {
        return None;
    }
    Some((std::str::from_utf8(&line[..end]).ok()?, &line[end + 1..]))
}

/// The decimal number at the start of `line`, as an envelope's serializer writes
/// one, and what follows it.
fn number_of(line: &[u8]) -> Option<(u64, &[u8])> {
    let digits = line.iter().position(|byte| !byte.is_ascii_digit())?;
    if digits == 0 || (digits > 1 && line[0] == b'0') {
        return None;
    }
    let number = std::str::from_utf8(&line[..digits]).ok()?.parse().ok()?;
    Some((number, &line[digits..]))
}

/// The `node` label off a labels object in the form: its values are strings, or a
/// number for `round`, and anything else is read by the parse. Also what follows
/// the object.
fn labels_of(mut rest: &[u8]) -> Option<(Option<&str>, &[u8])> {
    let mut node = None;
    let mut seen: Vec<&str> = Vec::new();
    if let Some(after) = rest.strip_prefix(b"}") {
        return Some((None, after));
    }
    loop {
        let (key, after) = string_of(rest.strip_prefix(b"\"")?)?;
        if seen.contains(&key) {
            return None;
        }
        seen.push(key);
        let after = after.strip_prefix(b":")?;
        rest = if let Some(value) = after.strip_prefix(b"\"") {
            if key == "round" {
                return None;
            }
            let (value, after) = string_of(value)?;
            if key == "node" {
                node = Some(value);
            }
            after
        } else if key == "round" {
            number_of(after)?.1
        } else {
            return None;
        };
        if let Some(after) = rest.strip_prefix(b"}") {
            return Some((node, after));
        }
        rest = rest.strip_prefix(b",")?;
    }
}

/// A JSON value read by the parser a record is read by, to the end and at the same
/// depth, with nothing kept of it: every string decoded and every number read, so what
/// the complete reader refuses in a payload this refuses, and a tree it would build
/// here is not built.
struct Checked;

impl<'de> Deserialize<'de> for Checked {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_any(CheckedVisitor)
    }
}

struct CheckedVisitor;

impl<'de> Visitor<'de> for CheckedVisitor {
    type Value = Checked;

    fn expecting(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        out.write_str("any JSON value")
    }
    fn visit_bool<E>(self, _: bool) -> Result<Checked, E> {
        Ok(Checked)
    }
    fn visit_i64<E>(self, _: i64) -> Result<Checked, E> {
        Ok(Checked)
    }
    fn visit_u64<E>(self, _: u64) -> Result<Checked, E> {
        Ok(Checked)
    }
    fn visit_f64<E>(self, _: f64) -> Result<Checked, E> {
        Ok(Checked)
    }
    fn visit_str<E>(self, _: &str) -> Result<Checked, E> {
        Ok(Checked)
    }
    fn visit_unit<E>(self) -> Result<Checked, E> {
        Ok(Checked)
    }
    fn visit_seq<A: SeqAccess<'de>>(self, mut items: A) -> Result<Checked, A::Error> {
        while items.next_element::<Checked>()?.is_some() {}
        Ok(Checked)
    }
    fn visit_map<A: MapAccess<'de>>(self, mut fields: A) -> Result<Checked, A::Error> {
        while fields.next_entry::<Checked, Checked>()?.is_some() {}
        Ok(Checked)
    }
}

/// A whole line in the writers' form, as far as the envelope's own types decide it: a
/// field it does not name, a repeated one or a value of another type is refused here and
/// answered by the complete reader's parse. The labels are [`labels_of`]'s to have read.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(
    dead_code,
    reason = "each field is read for the refusal its type makes, and only `artifacts` is used"
)]
struct WholeLine {
    v: u32,
    ts: IgnoredAny,
    stream: IgnoredAny,
    seq: u64,
    source: IgnoredAny,
    kind: IgnoredAny,
    phase: Option<IgnoredAny>,
    labels: Checked,
    payload: Checked,
    artifacts: Vec<ArtifactRef>,
}

/// The fields a merge needs off the opening of a line in the writers' form, with nothing
/// past `labels` looked at. What a reader asks whose answer is the source alone.
struct Opening<'a> {
    ts: &'a str,
    stream: &'a str,
    seq: u64,
    source: Source,
    kind: &'a str,
    node: Option<&'a str>,
}

fn opening_of(line: &[u8]) -> Option<Opening<'_>> {
    let (_, rest) = number_of(line.strip_prefix(b"{\"v\":")?)?;
    let (ts, rest) = string_of(rest.strip_prefix(b",\"ts\":\"")?)?;
    let (stream, rest) = string_of(rest.strip_prefix(b",\"stream\":\"")?)?;
    let (seq, rest) = number_of(rest.strip_prefix(b",\"seq\":")?)?;
    let (source, rest) = string_of(rest.strip_prefix(b",\"source\":\"")?)?;
    let source = Source::deserialize(StrDeserializer::<ValueError>::new(source)).ok()?;
    let (kind, mut rest) = string_of(rest.strip_prefix(b",\"kind\":\"")?)?;
    if let Some(phase) = rest.strip_prefix(b",\"phase\":\"") {
        let (phase, after) = string_of(phase)?;
        Phase::deserialize(StrDeserializer::<ValueError>::new(phase)).ok()?;
        rest = after;
    }
    let (node, rest) = labels_of(rest.strip_prefix(b",\"labels\":{")?)?;
    rest.strip_prefix(b",\"payload\":{")?;
    Some(Opening {
        ts,
        stream,
        seq,
        source,
        kind,
        node,
    })
}

/// A line in the one form the writers leave that the complete reader would read as a
/// record, or `None` for any other, which then goes to [`parsed_line`].
fn skimmed_line(line: &[u8]) -> Option<Skim<'_>> {
    let opening = opening_of(line)?;
    if !line.ends_with(b"]}") {
        return None;
    }
    let whole: WholeLine = serde_json::from_str(std::str::from_utf8(line).ok()?).ok()?;
    Some(Skim {
        line,
        ts: Cow::Borrowed(opening.ts),
        stream: Cow::Borrowed(opening.stream),
        seq: opening.seq,
        kind: Cow::Borrowed(opening.kind),
        node: opening.node.map(Cow::Borrowed),
        evidence: !whole.artifacts.is_empty(),
        parsed: None,
    })
}

/// A finished line read as the complete reader reads it: a whole record, or the
/// whole record after a fragment, or nothing.
fn parsed_line(line: &[u8]) -> Option<Skim<'_>> {
    let parsed = std::str::from_utf8(line)
        .ok()
        .and_then(|text| serde_json::from_str::<Envelope>(text).ok());
    let event = parsed.or_else(|| {
        let text = String::from_utf8_lossy(line);
        crate::journal::glued_tail(text.trim_end_matches('\r')).map(|(_, event)| event)
    })?;
    Some(Skim {
        line,
        ts: Cow::Owned(event.ts.clone()),
        stream: Cow::Owned(event.stream.clone()),
        seq: event.seq,
        kind: Cow::Owned(event.kind.0.clone()),
        node: event.labels.node.clone().map(Cow::Owned),
        evidence: !event.artifacts.is_empty(),
        parsed: Some(Box::new(event)),
    })
}

/// Every finished line of `bytes`, up to where a final unfinished one begins.
fn finished_lines(bytes: &[u8]) -> impl Iterator<Item = &[u8]> {
    let mut start = 0;
    memchr::memchr_iter(b'\n', bytes).map(move |end| {
        let line = &bytes[start..end];
        start = end + 1;
        line
    })
}

/// `work` over each of `items`, in their order, across threads where there are many.
fn in_order<T: Sync, R: Send>(items: &[T], work: impl Fn(&T) -> R + Sync) -> Vec<R> {
    if items.len() < WORKED_ALONE {
        return items.iter().map(work).collect();
    }
    let work = &work;
    std::thread::scope(|scope| {
        let workers: Vec<_> = items
            .chunks(items.len().div_ceil(WORKERS))
            .map(|items| scope.spawn(move || items.iter().map(work).collect::<Vec<_>>()))
            .collect();
        workers
            .into_iter()
            .flat_map(|worker| worker.join().expect("reading a line does not panic"))
            .collect()
    })
}

/// `lines` parsed in their order, `None` for one that does not parse.
fn parsed_records(lines: &[&Skim<'_>]) -> Vec<Option<Envelope>> {
    in_order(lines, |skim| match &skim.parsed {
        Some(event) => Some((**event).clone()),
        None => serde_json::from_slice::<Envelope>(skim.line).ok(),
    })
}

/// A selection over the lines of a journal, or `None` where a kept line in the
/// skimmed form turned out to be no record.
fn select(
    bytes: &[u8],
    selection: &Selection<'_>,
    skim: for<'a> fn(&'a [u8]) -> Option<Skim<'a>>,
) -> Option<SelectedRecords> {
    let finished: Vec<&[u8]> = finished_lines(bytes).collect();
    let lines: Vec<Skim<'_>> = in_order(&finished, |line| skim(line))
        .into_iter()
        .flatten()
        .collect();
    let key = |index: usize| {
        let line = &lines[index];
        (line.ts.as_ref(), line.stream.as_ref(), line.seq)
    };
    let queues = crate::journal::stream_queues(lines.len(), key);
    let mut stream_end = vec![false; lines.len()];
    let mut repeated = vec![false; lines.len()];
    for (_, queue) in &queues {
        if let (Some(first), Some(last)) = (queue.first(), queue.last()) {
            stream_end[*first] = true;
            stream_end[*last] = true;
        }
        for pair in queue.windows(2) {
            repeated[pair[1]] = lines[pair[0]].seq == lines[pair[1]].seq;
        }
    }
    let order = crate::journal::merge_queues(queues, key);

    let mut before_evidence: HashSet<usize> = HashSet::new();
    if matches!(selection, Selection::FirstScreen { .. }) {
        let mut previous: HashMap<&str, usize> = HashMap::new();
        for index in &order {
            let line = &lines[*index];
            if let (Some(node), false) = (line.node.as_deref(), repeated[*index]) {
                if line.evidence {
                    before_evidence.extend(previous.get(node).copied());
                }
                previous.insert(node, *index);
            }
        }
    }

    let chosen = |index: usize, at: usize| {
        let line = &lines[index];
        if at == 0 || at + 1 == order.len() {
            return true;
        }
        match selection {
            Selection::FirstScreen { node } => {
                !SKIPPED_KINDS.contains(&line.kind.as_ref())
                    || stream_end[index]
                    || before_evidence.contains(&index)
                    || node.is_some_and(|node| line.node.as_deref() == Some(node))
            }
            Selection::Share(owner) => line.node.as_deref() == *owner,
        }
    };

    let mut kept: Vec<&Skim<'_>> = Vec::new();
    let mut places = Vec::new();
    let mut share_places = Vec::new();
    let mut store_len = 0;
    let mut last_seq = 0;
    let mut share_lengths: HashMap<&str, usize> = HashMap::new();
    for (at, index) in order.iter().enumerate() {
        let line = &lines[*index];
        last_seq = last_seq.max(line.seq);
        let place = (!repeated[*index]).then(|| {
            store_len += 1;
            store_len - 1
        });
        let share_place = line.node.as_deref().zip(place).map(|(node, _)| {
            let length = share_lengths.entry(node).or_insert(0);
            *length += 1;
            *length - 1
        });
        if chosen(*index, at) {
            kept.push(line);
            places.push(place);
            share_places.push(share_place);
        }
    }

    let mut shares: HashMap<String, Vec<usize>> = HashMap::new();
    let mut events = Vec::with_capacity(kept.len());
    for (record, (line, share_place)) in parsed_records(&kept)
        .into_iter()
        .zip(kept.iter().zip(share_places))
    {
        events.push(record?);
        if let (Some(node), Some(place)) = (line.node.as_deref(), share_place) {
            shares.entry(node.to_owned()).or_default().push(place);
        }
    }
    Some(SelectedRecords {
        events,
        places,
        store_len,
        last_seq,
        shares,
    })
}

/// Read a whole journal, parsing only what `selection` keeps.
///
/// A journal that cannot be read reads as one holding nothing, which is the rule
/// [`crate::journal::read`] follows. The module note says what the read rests on.
#[must_use]
pub fn read_selected(journal: &Path, selection: &Selection<'_>) -> SelectedRecords {
    let Ok(bytes) = std::fs::read(journal) else {
        return SelectedRecords::default();
    };
    crate::loopstats::journal_read(journal, 0, bytes.len() as u64);
    select(&bytes, selection, |line| {
        skimmed_line(line).or_else(|| parsed_line(line))
    })
    .or_else(|| select(&bytes, selection, parsed_line))
    .unwrap_or_default()
}

/// Every record this crate's own writers put in a journal, in the order it was
/// appended, with no other record parsed: what a reader whose answer no relayed record
/// decides reads, since on a long run those are nearly all of its bytes.
pub(crate) fn pipeline_records(journal: &Path) -> Vec<Envelope> {
    let Ok(bytes) = std::fs::read(journal) else {
        return Vec::new();
    };
    crate::loopstats::journal_read(journal, 0, bytes.len() as u64);
    finished_lines(&bytes)
        .filter(|line| opening_of(line).is_none_or(|opening| opening.source == Source::Pipeline))
        .filter_map(|line| parsed_line(line)?.parsed.map(|event| *event))
        .filter(|event| event.source == Source::Pipeline)
        .collect()
}

/// How far from its start a journal's first `len` bytes are accounted for by a view
/// holding `reached`, the greatest `seq` it holds per stream: the end of the last
/// finished line whose record that covers.
///
/// What a view over chosen records resumes at. The file is append-only and a writer
/// claims a `seq` above its stream's last, so what was appended after the read
/// that chose them is exactly the tail no held `seq` reaches, and a final unfinished
/// line is no part of it. Read from the end, so a read that lost no race costs one
/// tail of the file.
pub(crate) fn covered_bytes(
    journal: &Path,
    len: u64,
    reached: &std::collections::BTreeMap<String, u64>,
) -> u64 {
    let mut window = 64 * 1024_u64;
    loop {
        let from = len.saturating_sub(window);
        let Some(bytes) = crate::ledger::read_range(journal, from, len - from) else {
            return 0;
        };
        // A line cut by the window's start is read in the next, wider window.
        let whole_from = match (from, memchr::memchr(b'\n', &bytes)) {
            (0, _) => 0,
            (_, Some(newline)) => newline + 1,
            (_, None) => bytes.len(),
        };
        let mut end =
            memchr::memrchr(b'\n', &bytes[whole_from..]).map_or(whole_from, |at| whole_from + at);
        while end > whole_from {
            let start = memchr::memrchr(b'\n', &bytes[whole_from..end])
                .map_or(whole_from, |at| whole_from + at + 1);
            let read = skimmed_line(&bytes[start..end]).or_else(|| parsed_line(&bytes[start..end]));
            if read.is_some_and(|line| {
                reached
                    .get(line.stream.as_ref())
                    .is_some_and(|held| line.seq <= *held)
            }) {
                return from + end as u64 + 1;
            }
            end = start.saturating_sub(1);
            if start == whole_from {
                break;
            }
        }
        if from == 0 {
            return 0;
        }
        window = window.saturating_mul(4);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::journal::{self, Journal, PipelineKind};
    use crate::ledger::RunPaths;
    use serde_json::json;

    /// The form the skim states is the one every writer leaves: this crate's own
    /// `emit` and its relay of each sibling, on a real journal. A line these writers
    /// stop leaving in that form is read rather than skimmed, which this fails on,
    /// since a whole journal of them would be parsed line by line.
    #[test]
    fn every_writer_leaves_the_field_order_the_skim_states() {
        let root = std::env::temp_dir().join(format!(
            "onepipeline-selected-writers-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        let paths = RunPaths::under(&root, "written");
        paths.create().expect("a run directory");
        let mut writer = Journal::open(&paths);
        writer
            .emit(
                PipelineKind::NodeDispatched,
                journal::labels("written", Some("build")),
                journal::payload(&[("note", json!("a \"quoted\" {\"v\":1} note"))]),
            )
            .expect("emitted");
        writer
            .emit(
                PipelineKind::RunStarted,
                journal::labels("written", None),
                journal::payload(&[]),
            )
            .expect("emitted without a node");
        let relayed = [
            json!({"v": 1, "ts": "2026-11-06T10:00:00.000Z", "stream": "graph-1", "seq": 0,
                   "source": "agentgraph", "kind": "turn-activity",
                   "labels": {"run_id": "written", "round": 2, "node": "build",
                              "step": "verify", "member": "worker", "persona": "engineer"},
                   "payload": {"index": 3},
                   "artifacts": [{"id": "gate-log", "kind": "log", "bytes": 12}]}),
            json!({"v": 1, "ts": "2026-11-06T10:00:01.000Z", "stream": "vcs-1", "seq": 0,
                   "source": "vcs", "kind": "landing-started", "phase": "integrate",
                   "labels": {"run_id": "written", "node": "build"}, "payload": {}}),
            json!({"v": 1, "ts": "2026-11-06T10:00:02.000Z", "stream": "vcs-1", "seq": 1,
                   "source": "vcs", "kind": "repository-gate", "labels": {}}),
        ];
        for record in &relayed {
            writer
                .relay(&serde_json::from_value(record.clone()).expect("a record"))
                .expect("relayed");
        }
        let text = std::fs::read_to_string(paths.journal()).expect("the journal");
        assert_eq!(text.lines().count(), 2 + relayed.len());
        for line in text.lines() {
            let skim = skimmed_line(line.as_bytes())
                .unwrap_or_else(|| panic!("no writer leaves this form:\n{line}"));
            let record: Envelope = serde_json::from_str(line).expect("a record");
            assert_eq!(skim.ts, record.ts, "{line}");
            assert_eq!(skim.stream, record.stream, "{line}");
            assert_eq!(skim.seq, record.seq, "{line}");
            assert_eq!(skim.kind, record.kind.0, "{line}");
            assert_eq!(
                skim.node.as_deref(),
                record.labels.node.as_deref(),
                "{line}"
            );
            assert_eq!(skim.evidence, !record.artifacts.is_empty(), "{line}");
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A line in any other form is not skimmed: each of these reads through the parse.
    #[test]
    fn a_line_in_another_form_is_never_skimmed() {
        let whole = concat!(
            r#"{"v":2,"ts":"2026-11-06T10:00:00.000Z","stream":"s","seq":1,"source":"pipeline","#,
            r#""kind":"node-started","labels":{"node":"n"},"payload":{},"artifacts":[]}"#
        );
        assert!(skimmed_line(whole.as_bytes()).is_some());
        for other in [
            whole.replace(r#""stream":"s","seq":1"#, r#""seq":1,"stream":"s""#),
            whole.replace(r#""node":"n""#, r#""node":"n"m""#),
            whole.replace(r#""seq":1"#, r#""seq":01"#),
            whole.replace(r#""seq":1"#, r#""seq":"1""#),
            whole.replace(r#""pipeline""#, r#""elsewhere""#),
            whole.replace(r#""labels":{"node":"n"}"#, r#""labels":{"round":"2"}"#),
            whole.replace(r#","artifacts":[]"#, ""),
            whole.replace(r#""payload":{}"#, r#""payload":{broken}"#),
            whole.replace(r#""payload":{}"#, r#""payload":{},"stray":1"#),
            whole.replace(r#"{"v":2"#, r#"{"v":99999999999"#),
            whole[..whole.len() - 4].to_owned(),
            format!(r#"{}{whole}"#, &whole[..whole.len() - 20]),
        ] {
            assert!(skimmed_line(other.as_bytes()).is_none(), "{other}");
        }
    }
}
