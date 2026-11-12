//! A selective read of a run's journal: the records a first screen needs out of a
//! whole journal, each with the place a complete read gives it.
//!
//! Published through [`crate::journal`]. Every line is merged, so a kept record's
//! place is the complete read's wherever each line nothing keeps is one the complete
//! read holds as a record, and only what is kept becomes a record.
//!
//! **What the skim rests on.** Every writer of a journal serializes an
//! [`Envelope`] through its one `Serialize`, whose fields are, in this order: `v`,
//! `ts`, `stream`, `seq`, `source`, `kind`, `phase` where there is one, `labels`,
//! `payload`, `artifacts`. A line in exactly that form has its merge fields read off
//! its opening; anything else about a line, a field in another order or a string
//! carrying an escape, sends it to the complete reader's own parse rather than past it.
//!
//! **What it answers as the complete reader does.** A line in the form is read for its
//! merge fields, its node and whether its `artifacts` are empty, and nothing between its
//! opening and its end is parsed unless the selection keeps it: on a long run nearly every
//! byte is a record nothing keeps. Every kept record is then parsed by the complete
//! reader's own parse, and where one in the form turns out to be no record the whole read
//! is the complete reader's, line by line, so what is returned for a kept record is what a
//! complete read holds for it. What is not checked is the middle of a line nothing keeps:
//! one whose opening and end are the writers' and whose middle a complete reader refuses
//! is merged as the record its opening names. A line out of the form is parsed rather than
//! skimmed, and answers what the complete reader answers: a line holding a fragment and
//! then a whole record is that record, and a blank or unreadable line is no record. A
//! final line no newline ended is no record, whatever it holds and whichever form it is in.

use std::borrow::Cow;
use std::collections::HashMap;
use std::path::Path;

use serde::de::value::{Error as ValueError, StrDeserializer};
use serde::Deserialize;

use crate::event::{Envelope, Phase, Source};

/// The kinds a first screen does not parse unless its node is asked for: by far the
/// most numerous records of a long run, and nothing a first answer draws.
const SKIPPED_KINDS: [&str; 2] = ["member-heartbeat", "turn-activity"];

/// Fewer items than this are worked through on one thread.
const WORKED_ALONE: usize = 512;

/// The most threads one read works across: past this a host running other work beside
/// it gains little, and a first screen should not take a busy host's every core.
const MOST_WORKERS: usize = 8;

/// The threads a read of many items works across: the host's parallelism, up to
/// [`MOST_WORKERS`], and the four spike-first-screen measured at where it cannot say.
fn workers() -> usize {
    std::thread::available_parallelism().map_or(4, |cores| cores.get().min(MOST_WORKERS))
}

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

/// A `ts` in the one form the writers stamp, `YYYY-MM-DDTHH:MM:SS.mmmZ`, as the number
/// its digits spell: two of that form compare as their text does, so a journal whose
/// every `ts` is in it is merged on numbers. `None` for any other text.
fn instant_of(ts: &str) -> Option<u64> {
    const FORM: &[u8; 24] = b"0000-00-00T00:00:00.000Z";
    let ts = ts.as_bytes();
    if ts.len() != FORM.len() {
        return None;
    }
    let mut instant = 0_u64;
    for (byte, form) in ts.iter().zip(FORM) {
        if *form == b'0' {
            if !byte.is_ascii_digit() {
                return None;
            }
            instant = instant * 10 + u64::from(byte - b'0');
        } else if byte != form {
            return None;
        }
    }
    Some(instant)
}

/// One line of a journal, as far as a selection needs it.
#[derive(Debug)]
pub(crate) struct Skim<'a> {
    pub(crate) line: &'a [u8],
    ts: Cow<'a, str>,
    pub(crate) stream: Cow<'a, str>,
    pub(crate) seq: u64,
    pub(crate) kind: Cow<'a, str>,
    node: Option<Cow<'a, str>>,
    pub(crate) evidence: bool,
    /// Its `ts` as [`instant_of`] reads it.
    instant: Option<u64>,
    /// The record, where reading this line already parsed it.
    pub(crate) parsed: Option<Box<Envelope>>,
}

/// The bytes from the start of `line` up to the closing quote of a string with no
/// escape in it, and what follows that quote.
pub(crate) fn bytes_of(line: &[u8]) -> Option<(&[u8], &[u8])> {
    let end = memchr::memchr2(b'"', b'\\', line)?;
    if line[end] != b'"' {
        return None;
    }
    Some((&line[..end], &line[end + 1..]))
}

/// [`bytes_of`], where what is wanted is the text.
pub(crate) fn string_of(line: &[u8]) -> Option<(&str, &[u8])> {
    let (text, rest) = bytes_of(line)?;
    Some((std::str::from_utf8(text).ok()?, rest))
}

/// The decimal number at the start of `line`, as an envelope's serializer writes
/// one, and what follows it.
pub(crate) fn number_of(line: &[u8]) -> Option<(u64, &[u8])> {
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
    let mut seen: Vec<&[u8]> = Vec::new();
    if let Some(after) = rest.strip_prefix(b"}") {
        return Some((None, after));
    }
    loop {
        // Only `node`'s value is text anything reads, so no other is decoded.
        let (key, after) = bytes_of(rest.strip_prefix(b"\"")?)?;
        if seen.contains(&key) {
            return None;
        }
        seen.push(key);
        let after = after.strip_prefix(b":")?;
        rest = if let Some(value) = after.strip_prefix(b"\"") {
            if key == b"round" {
                return None;
            }
            if key == b"node" {
                let (value, after) = string_of(value)?;
                node = Some(value);
                after
            } else {
                bytes_of(value)?.1
            }
        } else if key == b"round" {
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

/// The fields a merge needs off the opening of a line in the writers' form, with nothing
/// past `labels` looked at. What a reader asks whose answer is the source alone.
pub(crate) struct Opening<'a> {
    pub(crate) v: u32,
    pub(crate) ts: &'a str,
    pub(crate) stream: &'a str,
    pub(crate) seq: u64,
    pub(crate) source: Source,
    pub(crate) kind: &'a str,
    pub(crate) phase: Option<Phase>,
    node: Option<&'a str>,
    /// The line from just inside its `labels` object's opening brace.
    pub(crate) labels: &'a [u8],
}

pub(crate) fn opening_of(line: &[u8]) -> Option<Opening<'_>> {
    let (v, rest) = number_of(line.strip_prefix(b"{\"v\":")?)?;
    let v = u32::try_from(v).ok()?;
    let (ts, rest) = string_of(rest.strip_prefix(b",\"ts\":\"")?)?;
    let (stream, rest) = string_of(rest.strip_prefix(b",\"stream\":\"")?)?;
    let (seq, rest) = number_of(rest.strip_prefix(b",\"seq\":")?)?;
    let (source, rest) = string_of(rest.strip_prefix(b",\"source\":\"")?)?;
    let source = Source::deserialize(StrDeserializer::<ValueError>::new(source)).ok()?;
    let (kind, mut rest) = string_of(rest.strip_prefix(b",\"kind\":\"")?)?;
    let mut phase = None;
    if let Some(stamped) = rest.strip_prefix(b",\"phase\":\"") {
        let (stamped, after) = string_of(stamped)?;
        phase = Some(Phase::deserialize(StrDeserializer::<ValueError>::new(stamped)).ok()?);
        rest = after;
    }
    let labels = rest.strip_prefix(b",\"labels\":{")?;
    let (node, rest) = labels_of(labels)?;
    rest.strip_prefix(b",\"payload\":{")?;
    Some(Opening {
        v,
        ts,
        stream,
        seq,
        source,
        kind,
        phase,
        node,
        labels,
    })
}

/// Where a line's top-level `artifacts` array opens: the last time this appears in it,
/// since that array is an envelope's last field and an artifact holds no array.
const ARTIFACTS: &[u8] = b",\"artifacts\":[";

/// A line in the one form the writers leave, its merge fields read off its opening and
/// whether it carries evidence off its end, with nothing between parsed; `None` for any
/// other line, which then goes to [`parsed_line`].
///
/// A line holding a second envelope's opening past its start is what a fragment glued to
/// a later append looks like, which the complete reader answers as the later record, so
/// it is never skimmed. Outside a string an opening cannot appear inside a payload's text,
/// whose quotes are escaped; one a payload holds as structure only costs that line a parse.
// llmlint: ignore-block[boundary_inputs_validated] the planner's ruling for this read: a line the
// selection discards is read for its merge fields, node and artifact emptiness and never
// parsed, because validating each one cost a 102 MB journal's first screen 731 ms at the
// median. No skimmed field is trusted for a record `read_selected` returns: every kept line is
// the complete reader's own parse. The heads read (`src/journal_heads.rs`) does take fields
// from a line it never parses whole, for the five kinds `HEADED_KINDS` names, and no others:
// `v`, `ts`, `stream`, `seq`, `source`, `kind`, `phase` and `labels` off this opening, and
// `evidence` off the artifacts' end, because parsing those kinds' payloads is most of what a
// large run's first view waited on. A line outside the writers' form, or with an escape in
// any of those fields, is parsed whole instead. `tests/run_reads.rs`'s
// `every_head_and_every_fetched_record_is_the_complete_reads_and_the_state_is_opens` holds
// every such head equal to the complete read's record for every line the writers produce. A
// line in the form whose middle a complete reader refuses is still headed by its opening, the
// one departure the planner ruled for that cost, which
// `a_headed_line_malformed_in_its_middle_is_headed_by_its_opening_and_never_fetched` holds.
pub(crate) fn skimmed_line(line: &[u8]) -> Option<Skim<'_>> {
    skimmed_opening(line).map(|(skim, _)| skim)
}

/// [`skimmed_line`], with the opening it read the line's skim off.
pub(crate) fn skimmed_opening(line: &[u8]) -> Option<(Skim<'_>, Opening<'_>)> {
    let opening = opening_of(line)?;
    let body = line.strip_suffix(b"]}")?;
    if memchr::memmem::find(&line[1..], b"{\"v\":").is_some() {
        return None;
    }
    let artifacts = memchr::memmem::rfind(body, ARTIFACTS)?;
    let skim = Skim {
        line,
        ts: Cow::Borrowed(opening.ts),
        stream: Cow::Borrowed(opening.stream),
        seq: opening.seq,
        kind: Cow::Borrowed(opening.kind),
        node: opening.node.map(Cow::Borrowed),
        evidence: artifacts + ARTIFACTS.len() < body.len(),
        instant: instant_of(opening.ts),
        parsed: None,
    };
    Some((skim, opening))
} // llmlint: ignore-end[boundary_inputs_validated]

/// A finished line parsed whole as the complete reader parses it: a whole record, or the
/// whole record after a fragment, or nothing.
///
/// The one place this read, the heads read and [`fetch`](crate::journal::fetch) hand a
/// line to the JSON parser whole, so a test counting what they parse counts it here.
pub(crate) fn whole_record(line: &[u8]) -> Option<Envelope> {
    let parsed = std::str::from_utf8(line)
        .ok()
        .and_then(|text| serde_json::from_str::<Envelope>(text).ok());
    let event = parsed.or_else(|| {
        let text = String::from_utf8_lossy(line);
        crate::journal::glued_tail(text.trim_end_matches('\r')).map(|(_, event)| event)
    })?;
    #[cfg(test)]
    whole_parses::parsed(&event, line);
    Some(event)
}

/// What the tests of these reads count: every record [`whole_record`] parsed, on
/// whichever of a read's threads it parsed it.
#[cfg(test)]
pub(crate) mod whole_parses {
    use std::cell::RefCell;
    use std::sync::{Arc, Mutex};

    use crate::event::Envelope;

    /// One record parsed whole.
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub(crate) struct Parsed {
        pub(crate) stream: String,
        pub(crate) seq: u64,
        pub(crate) kind: String,
        /// The line's bytes, all of which went to the parser.
        pub(crate) line_bytes: usize,
        /// Its payload's, as the writers serialize it.
        pub(crate) payload_bytes: usize,
    }

    type Sink = Arc<Mutex<Vec<Parsed>>>;

    thread_local! {
        static SINK: RefCell<Option<Sink>> = const { RefCell::new(None) };
    }

    /// Count from here on, on this thread and the workers its reads start.
    pub(crate) fn start() {
        SINK.with_borrow_mut(|sink| *sink = Some(Sink::default()));
    }

    /// What was counted since [`start`], which stops counting.
    pub(crate) fn finish() -> Vec<Parsed> {
        let sink = SINK
            .with_borrow_mut(Option::take)
            .expect("counting was started");
        let parsed = sink.lock().expect("the count").clone();
        parsed
    }

    /// This thread's count, for a worker it starts to join.
    pub(super) fn current() -> Option<Sink> {
        SINK.with_borrow(Clone::clone)
    }

    /// Count on this worker into `sink`.
    pub(super) fn join(sink: Option<Sink>) {
        SINK.with_borrow_mut(|own| *own = sink);
    }

    pub(super) fn parsed(event: &Envelope, line: &[u8]) {
        SINK.with_borrow(|sink| {
            if let Some(sink) = sink {
                sink.lock().expect("the count").push(Parsed {
                    stream: event.stream.clone(),
                    seq: event.seq,
                    kind: event.kind.0.clone(),
                    line_bytes: line.len(),
                    payload_bytes: serde_json::to_string(&event.payload)
                        .expect("a payload serializes")
                        .len(),
                });
            }
        });
    }
}

/// A finished line read as the complete reader reads it: a whole record, or the
/// whole record after a fragment, or nothing.
pub(crate) fn parsed_line(line: &[u8]) -> Option<Skim<'_>> {
    let event = whole_record(line)?;
    Some(Skim {
        line,
        ts: Cow::Owned(event.ts.clone()),
        stream: Cow::Owned(event.stream.clone()),
        seq: event.seq,
        kind: Cow::Owned(event.kind.0.clone()),
        node: event.labels.node.clone().map(Cow::Owned),
        evidence: !event.artifacts.is_empty(),
        instant: instant_of(&event.ts),
        parsed: Some(Box::new(event)),
    })
}

/// Every finished line of `bytes`, up to where a final unfinished one begins.
pub(crate) fn finished_lines(bytes: &[u8]) -> impl Iterator<Item = &[u8]> {
    let mut start = 0;
    memchr::memchr_iter(b'\n', bytes).map(move |end| {
        let line = &bytes[start..end];
        start = end + 1;
        line
    })
}

/// `work` over each of `items`, in their order, across threads where there are many.
pub(crate) fn in_order<T: Sync, R: Send>(items: &[T], work: impl Fn(&T) -> R + Sync) -> Vec<R> {
    if items.len() < WORKED_ALONE {
        return items.iter().map(work).collect();
    }
    let work = &work;
    #[cfg(test)]
    let counted = whole_parses::current();
    std::thread::scope(|scope| {
        let workers: Vec<_> = items
            .chunks(items.len().div_ceil(workers()))
            .map(|items| {
                #[cfg(test)]
                let counted = counted.clone();
                scope.spawn(move || {
                    #[cfg(test)]
                    whole_parses::join(counted);
                    items.iter().map(work).collect::<Vec<_>>()
                })
            })
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
        None => whole_record(skim.line),
    })
}

/// Each stream's lines in its own order, and every line's index in merged order: what
/// [`merge_order`](crate::journal::merge_order) answers for the same records.
///
/// Where every line's `ts` is in the writers' form the heads are compared as numbers,
/// the `ts` as [`instant_of`] reads it and the stream as its rank among the journal's
/// streams, which orders them as their text does; otherwise as the text itself.
pub(crate) fn merged(lines: &[Skim<'_>]) -> (Vec<Vec<usize>>, Vec<usize>) {
    let Some(instants) = lines
        .iter()
        .map(|line| line.instant)
        .collect::<Option<Vec<u64>>>()
    else {
        let key = |index: usize| {
            let line = &lines[index];
            (line.ts.as_ref(), line.stream.as_ref(), line.seq)
        };
        let queues = crate::journal::stream_queues(lines.len(), key);
        let order = crate::journal::merge_queues(queues.clone(), key);
        return (queues.into_iter().map(|(_, queue)| queue).collect(), order);
    };
    let mut streams: HashMap<&str, usize> = HashMap::new();
    let mut queues: Vec<(&str, Vec<usize>)> = Vec::new();
    for (index, line) in lines.iter().enumerate() {
        let at = *streams.entry(line.stream.as_ref()).or_insert_with(|| {
            queues.push((line.stream.as_ref(), Vec::new()));
            queues.len() - 1
        });
        queues[at].1.push(index);
    }
    // By stream, so a queue's position is its stream's rank.
    queues.sort_unstable_by(|left, right| left.0.cmp(right.0));
    let mut queues: Vec<Vec<usize>> = queues.into_iter().map(|(_, queue)| queue).collect();
    for queue in &mut queues {
        // Stable, for the reason `stream_queues` gives.
        queue.sort_by_key(|index| lines[*index].seq);
    }
    let mut heads: std::collections::BinaryHeap<std::cmp::Reverse<(u64, usize)>> = queues
        .iter()
        .enumerate()
        .filter_map(|(rank, queue)| Some(std::cmp::Reverse((instants[*queue.first()?], rank))))
        .collect();
    let mut cursors = vec![0_usize; queues.len()];
    let mut order = Vec::with_capacity(lines.len());
    while let Some(std::cmp::Reverse((_, rank))) = heads.pop() {
        let queue = &queues[rank];
        order.push(queue[cursors[rank]]);
        cursors[rank] += 1;
        if let Some(next) = queue.get(cursors[rank]) {
            heads.push(std::cmp::Reverse((instants[*next], rank)));
        }
    }
    (queues, order)
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
    let (queues, order) = merged(&lines);
    let mut stream_end = vec![false; lines.len()];
    let mut repeated = vec![false; lines.len()];
    for queue in &queues {
        if let (Some(first), Some(last)) = (queue.first(), queue.last()) {
            stream_end[*first] = true;
            stream_end[*last] = true;
        }
        for pair in queue.windows(2) {
            repeated[pair[1]] = lines[pair[0]].seq == lines[pair[1]].seq;
        }
    }
    drop(queues);

    // Each line's node as a number, so nothing below hashes a name per line.
    let mut node_ids: HashMap<&str, usize> = HashMap::new();
    let mut names: Vec<&str> = Vec::new();
    let node_of: Vec<Option<usize>> = lines
        .iter()
        .map(|line| {
            let node = line.node.as_deref()?;
            Some(*node_ids.entry(node).or_insert_with(|| {
                names.push(node);
                names.len() - 1
            }))
        })
        .collect();
    let asked = match selection {
        Selection::FirstScreen { node } | Selection::Share(node) => {
            node.map(|node| node_ids.get(node).copied())
        }
    };

    let mut before_evidence = vec![false; lines.len()];
    if matches!(selection, Selection::FirstScreen { .. }) {
        let mut previous: Vec<Option<usize>> = vec![None; names.len()];
        for index in &order {
            if let (Some(node), false) = (node_of[*index], repeated[*index]) {
                if lines[*index].evidence {
                    if let Some(before) = previous[node] {
                        before_evidence[before] = true;
                    }
                }
                previous[node] = Some(*index);
            }
        }
    }

    let chosen = |index: usize, at: usize| {
        let line = &lines[index];
        if at == 0 || at + 1 == order.len() {
            return true;
        }
        match (selection, asked) {
            (Selection::FirstScreen { .. }, asked) => {
                !SKIPPED_KINDS.contains(&line.kind.as_ref())
                    || stream_end[index]
                    || before_evidence[index]
                    || asked.is_some_and(|asked| asked.is_some() && asked == node_of[index])
            }
            (Selection::Share(_), None) => node_of[index].is_none(),
            (Selection::Share(_), Some(asked)) => asked.is_some() && asked == node_of[index],
        }
    };

    let mut kept: Vec<(&Skim<'_>, Option<usize>)> = Vec::new();
    let mut places = Vec::new();
    let mut share_places = Vec::new();
    let mut store_len = 0;
    let mut last_seq = 0;
    let mut share_lengths = vec![0_usize; names.len()];
    for (at, index) in order.iter().enumerate() {
        let line = &lines[*index];
        last_seq = last_seq.max(line.seq);
        let place = (!repeated[*index]).then(|| {
            store_len += 1;
            store_len - 1
        });
        let share_place = node_of[*index].zip(place).map(|(node, _)| {
            share_lengths[node] += 1;
            share_lengths[node] - 1
        });
        if chosen(*index, at) {
            kept.push((line, node_of[*index]));
            places.push(place);
            share_places.push(share_place);
        }
    }

    let mut share_lists: Vec<Vec<usize>> = vec![Vec::new(); names.len()];
    let mut events = Vec::with_capacity(kept.len());
    let lines_kept: Vec<&Skim<'_>> = kept.iter().map(|(line, _)| *line).collect();
    for (record, ((_, node), share_place)) in parsed_records(&lines_kept)
        .into_iter()
        .zip(kept.iter().zip(share_places))
    {
        events.push(record?);
        if let (Some(node), Some(place)) = (node, share_place) {
            share_lists[*node].push(place);
        }
    }
    let shares: HashMap<String, Vec<usize>> = names
        .iter()
        .zip(share_lists)
        .filter(|(_, places)| !places.is_empty())
        .map(|(name, places)| ((*name).to_owned(), places))
        .collect();
    Some(SelectedRecords {
        events,
        places,
        store_len,
        last_seq,
        shares,
    })
}

/// Fewer bytes than this are read on one thread. Only the Unix read splits a file
/// across threads, so elsewhere there is nothing for it to bound.
#[cfg(unix)]
const READ_ALONE: usize = 8 << 20;

/// The bytes of `path` up to its length when opened: on a long journal read in parts
/// across threads, since one thread's copy out of the page cache is most of what a
/// 100 MB first screen used to wait on. A file that came up shorter than its length
/// was read again whole, which is what a reader of an append-only file never meets.
pub(crate) fn whole_file(path: &Path) -> Option<Vec<u8>> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::FileExt;
        let file = std::fs::File::open(path).ok()?;
        let len = usize::try_from(file.metadata().ok()?.len()).ok()?;
        if len >= READ_ALONE {
            let mut bytes = vec![0_u8; len];
            let part = len.div_ceil(workers());
            let read = std::thread::scope(|scope| {
                let parts: Vec<_> = bytes
                    .chunks_mut(part)
                    .enumerate()
                    .map(|(at, chunk)| {
                        let file = &file;
                        scope.spawn(move || file.read_exact_at(chunk, (at * part) as u64))
                    })
                    .collect();
                parts
                    .into_iter()
                    .all(|part| part.join().is_ok_and(|read| read.is_ok()))
            });
            if read {
                return Some(bytes);
            }
        }
    }
    std::fs::read(path).ok()
}

/// Read a whole journal, parsing only what `selection` keeps.
///
/// A journal that cannot be read reads as one holding nothing, which is the rule
/// the complete read follows. The module note says what the read rests on.
#[must_use]
pub fn read_selected(journal: &Path, selection: &Selection<'_>) -> SelectedRecords {
    let Some(bytes) = whole_file(journal) else {
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
        .filter(|line| {
            // A relayed fragment glued to a later append opens as the fragment and reads
            // as the record after it, which may be this crate's own.
            opening_of(line).is_none_or(|opening| opening.source == Source::Pipeline)
                || memchr::memmem::find(&line[1..], b"{\"v\":").is_some()
        })
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

    /// A line in any other form is not skimmed: each of these reads through the parse, a
    /// second envelope's opening inside it among them.
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
            whole.replace(r#""payload":{}"#, r#""payload":{"inner":{"v":1}}"#),
            whole.replace(r#","artifacts":[]"#, r#","artifacts":[],"stray":1"#),
            whole.replace(r#"{"v":2"#, r#"{"v":99999999999"#),
            whole[..whole.len() - 4].to_owned(),
            format!(r#"{}{whole}"#, &whole[..whole.len() - 20]),
        ] {
            assert!(skimmed_line(other.as_bytes()).is_none(), "{other}");
        }
        // What the skim does not look at: a middle nothing keeps is never parsed, so one
        // broken there is skimmed by its opening, and found only if it is kept.
        for broken in [
            whole.replace(r#""payload":{}"#, r#""payload":{broken}"#),
            whole.replace(r#""payload":{}"#, r#""payload":{},"stray":1"#),
        ] {
            assert!(skimmed_line(broken.as_bytes()).is_some(), "{broken}");
        }
        let evidence = whole.replace(r#""artifacts":[]"#, r#""artifacts":[{"id":"a"}]"#);
        assert!(skimmed_line(evidence.as_bytes()).is_some_and(|skim| skim.evidence));
        assert!(skimmed_line(whole.as_bytes()).is_some_and(|skim| !skim.evidence));
    }
}
