//! The two general reads of a run's journal: every record's head, which carries no
//! payload, and then whole records by the handles those heads hold.
//!
//! Published through [`crate::journal`], and the heads read through
//! [`RunView::heads`](crate::views::RunView::heads), whose rustdoc states which
//! records' payloads its fold parses. The heads read rests on the same skim and merge
//! as [`read_selected`](crate::journal::read_selected), over every line: a line in
//! the writers' form is headed off its opening and its end, and any other line is
//! parsed whole and headed from the record, so a head is the complete read's record in
//! every field it carries. A line in the form whose middle a complete reader refuses
//! is headed by its opening, which is the selective read's ruling for a line it
//! discards: its middle is never parsed unless it is fetched, and [`fetch`] then
//! answers [`RecordMoved`](crate::Error::RecordMoved) for it rather than a record.

use std::borrow::Cow;
use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{mpsc, Arc, Mutex};

use serde_json::{Map, Value};

use crate::checkpoint::Coverage;
use crate::event::{Envelope, EventKind, Labels, Phase, Source};
use crate::journal_selected::{
    covered, finished_lines, in_order, line_runs, merged_in, number_of, on_threads, opening_of,
    parsed_line, skimmed_opening, string_of, whole_file, whole_record, workers, Numbered, Skim,
};

/// Where one record's line sits in a run's journal, and which record it is: enough for
/// [`fetch`] to read that record again without reading the journal through. Opaque;
/// valid for the journal the heads read read, for as long as that journal is only
/// appended to.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct RecordHandle {
    /// The byte its line starts at.
    offset: u64,
    /// Its line's length, short of the newline that ends it.
    len: u64,
    /// The record's stream, shared by every handle of that stream.
    stream: Arc<str>,
    /// The record's `seq`.
    seq: u64,
}

/// One record of a run's journal without its payload.
#[derive(Debug, Clone, PartialEq)]
pub struct RecordHead {
    /// The record's `ts`.
    pub ts: String,
    /// The record's `stream`.
    pub stream: String,
    /// The record's `seq`.
    pub seq: u64,
    /// The record's `source`.
    pub source: Source,
    /// The record's `kind`.
    pub kind: EventKind,
    /// The record's `phase`, where it was stamped at one.
    pub phase: Option<Phase>,
    /// The record's `labels`.
    pub labels: Labels,
    /// Whether the record's `artifacts` are not empty.
    pub evidence: bool,
    /// Its place in the merged store; `None` for a repeated `(stream, seq)`.
    pub place: Option<usize>,
    /// What [`fetch`] reads the whole record by.
    pub handle: RecordHandle,
}

/// The kinds whose payload the heads read's fold never parses: by far the most numerous
/// records of a long run, and none of them decides a node's status, reason, awaits or
/// result, the run's summary or its tallies. Their records are folded by their heads.
/// [`RunView::heads`](crate::views::RunView::heads) states them to its readers.
pub(crate) const HEADED_KINDS: [&str; 5] = [
    "member-heartbeat",
    "turn-activity",
    "turn-message",
    "turn-started",
    "turn-completed",
];

/// What a heads read hands the view it folded for: every record's head, and what the
/// records cover.
#[derive(Debug, Default)]
pub(crate) struct Heads {
    /// Every record's head, in merged order.
    pub(crate) heads: Vec<RecordHead>,
    /// What the records cover, as absorbing each of them in turn would leave it.
    pub(crate) coverage: Coverage,
    /// The merged store's length.
    pub(crate) store_len: usize,
    /// The highest `seq` in the whole journal.
    pub(crate) last_seq: u64,
}

/// What a record's head carries that its folded record does not.
#[derive(Debug)]
struct Mark {
    /// Whether its line's `artifacts` are not empty.
    evidence: bool,
    /// Its place in the merged store.
    place: Option<usize>,
    /// Where its line sits.
    handle: RecordHandle,
    /// Its record's `ts` as [`millis_of`](crate::projection::millis_of) reads it, read by
    /// the worker that built the record rather than by the fold, which cannot be shared.
    at: Option<u64>,
}

/// A record's head, made of its folded record's own fields rather than a copy of them.
fn head_of(record: Envelope, mark: Mark) -> RecordHead {
    RecordHead {
        ts: record.ts,
        stream: record.stream,
        seq: record.seq,
        source: record.source,
        kind: record.kind,
        phase: record.dimensions.phase,
        labels: record.labels,
        evidence: mark.evidence,
        place: mark.place,
        handle: mark.handle,
    }
}

/// The labels object at the start of `rest`, from just inside its opening brace, walked
/// as [`labels_of`] reads it: each key and its value, a string or a number for `round`,
/// handed to `visit`. `None` where the object is not in that form, or `visit` refuses.
fn walked_labels<'a>(
    mut rest: &'a [u8],
    mut visit: impl FnMut(&'a str, Result<u64, &'a str>) -> Option<()>,
) -> Option<()> {
    if rest.starts_with(b"}") {
        return Some(());
    }
    loop {
        let (key, after) = string_of(rest.strip_prefix(b"\"")?)?;
        let after = after.strip_prefix(b":")?;
        rest = if key == "round" {
            let (round, after) = number_of(after)?;
            visit(key, Ok(round))?;
            after
        } else {
            let (value, after) = string_of(after.strip_prefix(b"\"")?)?;
            visit(key, Err(value))?;
            after
        };
        if rest.starts_with(b"}") {
            return Some(());
        }
        rest = rest.strip_prefix(b",")?;
    }
}

/// A labels object in the form [`opening_of`](crate::journal_selected::opening_of)
/// skims, read whole: string values, a number for `round`, and keys with no escape.
/// `None` for any other, which the record's whole parse then reads.
fn labels_of(rest: &[u8]) -> Option<Labels> {
    let mut labels = Labels::default();
    let mut extra = Map::new();
    walked_labels(rest, |key, value| {
        let seen = match key {
            "round" => labels.round.is_some(),
            "run_id" => labels.run_id.is_some(),
            "node" => labels.node.is_some(),
            "step" => labels.step.is_some(),
            "member" => labels.member.is_some(),
            "persona" => labels.persona.is_some(),
            other => extra.contains_key(other),
        };
        if seen {
            return None;
        }
        match value {
            Ok(round) => labels.round = Some(round),
            Err(value) => {
                let value = value.to_owned();
                match key {
                    "run_id" => labels.run_id = Some(value),
                    "node" => labels.node = Some(value),
                    "step" => labels.step = Some(value),
                    "member" => labels.member = Some(value),
                    "persona" => labels.persona = Some(value),
                    other => {
                        extra.insert(other.to_owned(), Value::String(value));
                    }
                }
            }
        }
        Some(())
    })?;
    labels.extra = extra;
    Some(labels)
}

/// Whether [`labels_of`] reads the labels object at the start of `rest`, answered without
/// building them, for a line whose skim has already refused a repeated key.
///
/// The skim's own walk of the object reads its keys and every value but `node` as bytes,
/// so what this adds is that each is text, the one thing [`labels_of`] then asks of it.
fn labels_read(rest: &[u8]) -> bool {
    walked_labels(rest, |_, _| Some(())).is_some()
}

/// One line as the heads read takes it, before the merge: the skim the merge reads, which
/// holds no record where the line is one of [`HEADED_KINDS`] in the writers' form whose
/// labels [`labels_of`] reads, and is headed off its opening. Every other line is the
/// complete reader's own parse, whose record the skim holds, so a line that parse refuses
/// is dropped here, as the complete read drops it, and is never merged.
// llmlint: ignore-block[boundary_inputs_validated] the planner's ruling for this read: a
// line of one of `HEADED_KINDS` in the writers' form is headed off its opening, whose
// every field `opening_of` and `labels_of` read at `Envelope`'s own types (a `Source` and
// a `Phase` through their deserializers, `seq` and `round` as numbers, every string
// unescaped or else the line is parsed whole), and its middle is never validated, because
// knowing the complete reader refuses it means parsing the payload this read exists not to
// parse. Such a line is headed where the complete read holds no record, which
// `RunView::heads`'s rustdoc and `docs/contract.md` state; `fetch` answers `RecordMoved`
// for it, never a record, and `tests/run_reads.rs`'s
// `a_headed_line_malformed_in_its_middle_is_headed_by_its_opening_and_never_fetched` holds
// that. Every line the writers produce is headed equal to the complete read's record, which
// `every_head_and_every_fetched_record_is_the_complete_reads_and_the_state_is_opens` holds.
// Every other line, a malformed one of a kind the fold parses included, is the complete
// reader's own parse, and dropped where that parse refuses it.
fn taken(line: &[u8]) -> Option<Skim<'_>> {
    if let Some((skim, opening)) = skimmed_opening(line) {
        if HEADED_KINDS.contains(&opening.kind) && labels_read(opening.labels) {
            return Some(skim);
        }
    }
    parsed_line(line)
}

/// What the fold reads of a line [`taken`] heads off its opening: its head, with nothing
/// past it. The opening is read again here, on the worker building the record, rather
/// than held from the skim for every line until then.
fn headed(line: &[u8]) -> Envelope {
    let opening = opening_of(line).expect("a headed line's opening was read in its skim");
    Envelope {
        v: opening.v,
        ts: opening.ts.to_owned(),
        stream: opening.stream.to_owned(),
        seq: opening.seq,
        source: opening.source,
        kind: EventKind(opening.kind.to_owned()),
        dimensions: opening.phase.into(),
        labels: labels_of(opening.labels).expect("a headed line's labels were read in its skim"),
        payload: Map::new(),
        artifacts: Vec::new(),
    }
}
// llmlint: ignore-end[boundary_inputs_validated]

/// One line in merged order, on its way to being a folded record and a head's mark.
struct Placed {
    /// Its index among the read's lines.
    index: usize,
    /// Its place in the merged store.
    place: Option<usize>,
    /// Its record, where its line was parsed whole.
    parsed: Option<Box<Envelope>>,
}

/// How many records one step of the pipeline below builds, folds and heads at a time:
/// enough that a step is worth handing between threads, few enough that the fold starts
/// on the first records while the rest are still being built.
const PIPELINED: usize = 2048;

/// Every record's head, with each record handed to `fold` in merged order before its
/// head is made.
///
/// A journal that cannot be read reads as one holding nothing, which is the rule the
/// complete read follows. Each line is read once, by [`taken`], and the journal is never
/// read through a second time. Each record's strings are built once, and `fold` reads
/// them before they are moved into its head.
pub(crate) fn read_heads(journal: &Path, mut fold: impl FnMut(&Envelope, Option<u64>)) -> Heads {
    let Some(bytes) = whole_file(journal) else {
        return Heads::default();
    };
    crate::loopstats::journal_read(journal, 0, bytes.len() as u64);
    // Each worker finds its own run's lines, and numbers their streams.
    let runs = on_threads(line_runs(&bytes), |run| {
        let mut numbers: HashMap<Cow<'_, str>, usize> = HashMap::new();
        let mut skims = Vec::new();
        let mut of = Vec::new();
        for skim in finished_lines(run).filter_map(taken) {
            let number = match numbers.get(skim.stream.as_ref()) {
                Some(number) => *number,
                None => {
                    let number = numbers.len();
                    numbers.insert(skim.stream.clone(), number);
                    number
                }
            };
            of.push(number);
            skims.push(skim);
        }
        let mut names = vec![Cow::Borrowed(""); numbers.len()];
        for (name, number) in numbers {
            names[number] = name;
        }
        (skims, of, names)
    });
    let len = runs.iter().map(|(skims, _, _)| skims.len()).sum();
    let mut lines = Vec::with_capacity(len);
    let mut run_numbers = Vec::with_capacity(runs.len());
    let mut run_names = Vec::with_capacity(runs.len());
    for (skims, of, names) in runs {
        lines.extend(skims);
        run_numbers.push(of);
        run_names.push(names);
    }
    // One number for each stream across the runs, each name hashed once a run.
    let mut numbers: HashMap<&str, usize> = HashMap::new();
    let mut names: Vec<&str> = Vec::new();
    let mut of = Vec::with_capacity(len);
    for (run_of, run_names) in run_numbers.iter().zip(&run_names) {
        let renumbered: Vec<usize> = run_names
            .iter()
            .map(|name| {
                *numbers.entry(name.as_ref()).or_insert_with(|| {
                    names.push(name.as_ref());
                    names.len() - 1
                })
            })
            .collect();
        of.extend(run_of.iter().map(|number| renumbered[*number]));
    }
    drop(run_numbers);
    let (queues, order) = merged_in(&lines, Some(Numbered { of, names }));
    let coverage = covered(&lines, &queues);
    // Each line's stream, shared by every handle of it, and whether it repeats the
    // `(stream, seq)` before it in its stream.
    let mut stream_of: Vec<Option<Arc<str>>> = vec![None; lines.len()];
    let mut repeated = vec![false; lines.len()];
    for queue in &queues {
        let Some(first) = queue.first() else {
            continue;
        };
        let stream: Arc<str> = Arc::from(lines[*first].stream.as_ref());
        for index in queue {
            stream_of[*index] = Some(Arc::clone(&stream));
        }
        for pair in queue.windows(2) {
            repeated[pair[1]] = lines[pair[0]].seq == lines[pair[1]].seq;
        }
    }
    drop(queues);
    let mut store_len = 0;
    let mut last_seq = 0;
    let mut placed: Vec<Vec<Placed>> = Vec::new();
    for run in order.chunks(PIPELINED) {
        let mut this = Vec::with_capacity(run.len());
        for index in run {
            last_seq = last_seq.max(lines[*index].seq);
            let place = (!repeated[*index]).then(|| {
                store_len += 1;
                store_len - 1
            });
            this.push(Placed {
                index: *index,
                place,
                // Moved out of its skim rather than copied: the fold reads this record.
                parsed: lines[*index].parsed.take(),
            });
        }
        placed.push(this);
    }
    let skims = &lines;
    let start = bytes.as_ptr() as usize;
    let built = |placed: Placed| {
        let skim = &skims[placed.index];
        let handle = RecordHandle {
            offset: (skim.line.as_ptr() as usize - start) as u64,
            len: skim.line.len() as u64,
            stream: Arc::clone(
                stream_of[placed.index]
                    .as_ref()
                    .expect("every line is in a queue"),
            ),
            seq: skim.seq,
        };
        let record = match placed.parsed {
            Some(record) => *record,
            None => headed(skim.line),
        };
        let mark = Mark {
            evidence: skim.evidence,
            place: placed.place,
            handle,
            at: crate::projection::millis_of(&record.ts),
        };
        (record, mark)
    };
    let heads = pipelined(placed, order.len(), built, &mut fold);
    drop(lines);
    // Nothing reads the journal's bytes once its heads are made, and handing a buffer the
    // journal's size back to the host is a wait of its own, so it is let go of on a thread
    // that outlives this read rather than before the read answers. Where the host refuses
    // that thread, they are let go of here, as they were.
    let spent = (bytes, stream_of, order, repeated);
    let _ = std::thread::Builder::new().spawn(move || drop(spent));
    Heads {
        heads,
        coverage,
        store_len,
        last_seq,
    }
}

/// One built run of records, and their marks.
type Built = (Vec<Envelope>, Vec<Mark>);

/// `runs` built by `build` on the read's workers, handed to `fold` in their order, and
/// then moved into heads on a thread of its own: the fold is the one step that cannot be
/// shared, so the others are done around it, a run at a time, rather than before and
/// after it.
fn pipelined(
    runs: Vec<Vec<Placed>>,
    len: usize,
    build: impl Fn(Placed) -> (Envelope, Mark) + Sync,
    fold: &mut impl FnMut(&Envelope, Option<u64>),
) -> Vec<RecordHead> {
    let made = |runs: &mut dyn Iterator<Item = Built>| {
        let mut heads = Vec::with_capacity(len);
        for (records, marks) in runs {
            heads.extend(
                records
                    .into_iter()
                    .zip(marks)
                    .map(|(record, mark)| head_of(record, mark)),
            );
        }
        heads
    };
    if runs.len() < 2 {
        let mut folded = runs.into_iter().map(|run| {
            let (records, marks): Built = run.into_iter().map(&build).unzip();
            for (record, mark) in records.iter().zip(&marks) {
                fold(record, mark.at);
            }
            (records, marks)
        });
        return made(&mut folded);
    }
    let count = runs.len();
    let pending: Vec<Mutex<Option<Vec<Placed>>>> =
        runs.into_iter().map(|run| Mutex::new(Some(run))).collect();
    let next = AtomicUsize::new(0);
    std::thread::scope(|scope| {
        let (built_tx, built_rx) = mpsc::channel::<(usize, Built)>();
        for _ in 0..workers().min(count) {
            let built_tx = built_tx.clone();
            let (pending, next, build) = (&pending, &next, &build);
            scope.spawn(move || {
                // Taken in their order, so the fold, which needs them in it, waits least.
                loop {
                    let index = next.fetch_add(1, Ordering::Relaxed);
                    let Some(slot) = pending.get(index) else {
                        return;
                    };
                    let run = slot
                        .lock()
                        .expect("a run's slot is never poisoned")
                        .take()
                        .expect("each run is built once");
                    let built: Built = run.into_iter().map(build).unzip();
                    if built_tx.send((index, built)).is_err() {
                        return;
                    }
                }
            });
        }
        drop(built_tx);
        let (folded_tx, folded_rx) = mpsc::channel::<Built>();
        let heads = scope.spawn(move || made(&mut folded_rx.into_iter()));
        let mut waiting: Vec<Option<Built>> = (0..count).map(|_| None).collect();
        let mut due = 0;
        for (index, built) in built_rx {
            waiting[index] = Some(built);
            while let Some(built) = waiting.get_mut(due).and_then(Option::take) {
                for (record, mark) in built.0.iter().zip(&built.1) {
                    fold(record, mark.at);
                }
                if folded_tx.send(built).is_err() {
                    break;
                }
                due += 1;
            }
        }
        drop(folded_tx);
        heads.join().expect("making heads does not panic")
    })
}

/// A positioned read, so the fetch's workers share one open file without a cursor
/// between them; a file too short for `buffer` answers `UnexpectedEof`.
fn read_at(file: &std::fs::File, buffer: &mut [u8], offset: u64) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        std::os::unix::fs::FileExt::read_exact_at(file, buffer, offset)
    }
    #[cfg(windows)]
    {
        let mut filled = 0;
        while filled < buffer.len() {
            let read = std::os::windows::fs::FileExt::seek_read(
                file,
                &mut buffer[filled..],
                offset + filled as u64,
            )?;
            if read == 0 {
                return Err(std::io::ErrorKind::UnexpectedEof.into());
            }
            filled += read;
        }
        Ok(())
    }
    #[cfg(not(any(unix, windows)))]
    {
        use std::io::{Read, Seek, SeekFrom};
        let mut file = file;
        file.seek(SeekFrom::Start(offset))?;
        file.read_exact(buffer)
    }
}

/// The whole records `handles` name, in the order named.
///
/// Each is the record the complete read holds at that handle's place, payload and
/// artifacts included, parsed from its line alone.
///
/// # Errors
///
/// [`RecordMoved`](crate::Error::RecordMoved) where the line at a handle is not a
/// record of that handle's `(stream, seq)`, as in a journal rewritten rather than
/// appended to since the heads read, or one shorter than that line's end: a fetch
/// never answers a record of another `(stream, seq)`. [`Ledger`](crate::Error::Ledger)
/// where the journal cannot be opened or read.
pub fn fetch(journal: &Path, handles: &[RecordHandle]) -> crate::Result<Vec<Envelope>> {
    if handles.is_empty() {
        return Ok(Vec::new());
    }
    let ledger = |source| crate::Error::Ledger {
        path: journal.to_path_buf(),
        source,
    };
    let file = std::fs::File::open(journal).map_err(ledger)?;
    let moved = |handle: &RecordHandle| crate::Error::RecordMoved {
        path: journal.to_path_buf(),
        offset: handle.offset,
        stream: handle.stream.to_string(),
        seq: handle.seq,
    };
    let fetched = in_order(handles, |handle| {
        let len = usize::try_from(handle.len).map_err(|_| moved(handle))?;
        let mut line = vec![0_u8; len];
        match read_at(&file, &mut line, handle.offset) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::UnexpectedEof => {
                return Err(moved(handle));
            }
            Err(error) => return Err(ledger(error)),
        }
        match whole_record(&line) {
            Some(record) if record.seq == handle.seq && *record.stream == *handle.stream => {
                Ok(record)
            }
            _ => Err(moved(handle)),
        }
    });
    // One report for the fetch, on the thread that asked, where its tally is read.
    crate::loopstats::journal_read(
        journal,
        handles
            .iter()
            .map(|handle| handle.offset)
            .min()
            .unwrap_or(0),
        handles.iter().map(|handle| handle.len).sum(),
    );
    fetched.into_iter().collect()
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::path::PathBuf;

    use serde_json::json;

    use super::*;
    use crate::journal::{self, Journal, PipelineKind};
    use crate::journal_selected::whole_parses;
    use crate::ledger::RunPaths;
    use crate::views::RunView;

    /// Every kind the journal holds beside the run's own: the five the fold reads by
    /// their heads, and three it parses.
    const KINDS: [&str; 8] = [
        "member-heartbeat",
        "turn-activity",
        "turn-message",
        "turn-started",
        "turn-completed",
        "change-check",
        "oneharness-session",
        "landing-started",
    ];

    /// The kinds whose payloads the heads read must never parse unless they are fetched,
    /// named here rather than read off the engine, so the test holds the engine to them.
    const NEVER_PARSED: [&str; 5] = [
        "member-heartbeat",
        "turn-activity",
        "turn-message",
        "turn-started",
        "turn-completed",
    ];

    type Key = (String, u64);

    /// A run whose journal its writers left, every kind in it, and the records of lines
    /// written out of the writers' form: in another field order, or with an escaped
    /// string in their opening.
    struct Written {
        root: PathBuf,
        paths: RunPaths,
        out_of_form: BTreeSet<Key>,
    }

    impl Drop for Written {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    fn written(name: &str) -> Written {
        written_with(name, &[])
    }

    /// [`written`], with `spliced` written into the middle of its journal as they are.
    fn written_with(name: &str, spliced: &[String]) -> Written {
        let root = std::env::var_os("ONEPIPELINE_NODE_SCRATCH_DIR")
            .map_or_else(std::env::temp_dir, PathBuf::from)
            .join(format!("heads-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let paths = RunPaths::under(&root, "demo");
        paths.create().expect("a run directory");
        let launch = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/recorded/run-root/onemessagebus-repair-2/launch.json");
        std::fs::copy(launch, paths.launch()).expect("a launch record");
        let plan = json!({
            "schema_version": 3, "name": "demo", "concurrency": 4,
            "tasks": [{"id": "build", "persona": "engineer", "task": "## What\ndo it"}],
        });
        let mut writer = Journal::open(&paths);
        writer
            .emit(
                PipelineKind::RunStarted,
                journal::labels("demo", None),
                journal::payload(&[("plan", plan)]),
            )
            .expect("started");
        writer
            .emit(
                PipelineKind::NodeDispatched,
                journal::labels("demo", Some("build")),
                journal::payload(&[]),
            )
            .expect("dispatched");
        let mut out_of_form = BTreeSet::new();
        let mut appended = Vec::new();
        for seq in 0..1_200_u64 {
            let kind = KINDS[(seq % KINDS.len() as u64) as usize];
            let source = if kind == "landing-started" {
                "vcs"
            } else {
                "agentgraph"
            };
            let mut record = json!({
                "v": 1,
                "ts": format!("2026-11-06T10:{:02}:{:02}.{:03}Z", seq / 60_000, (seq / 1000) % 60, seq % 1000),
                "stream": format!("graph-{}", seq % 7),
                "seq": seq,
                "source": source,
                "kind": kind,
                "labels": {"run_id": "demo", "node": "build", "member": "worker"},
                "payload": {"name": "Bash", "detail": format!("step {seq}"), "text": "a \"quoted\" {\"v\":1} text"},
            });
            if seq % 13 == 0 {
                record["artifacts"] =
                    json!([{"id": format!("log-{seq}"), "kind": "log", "bytes": 3}]);
            }
            let key = (format!("graph-{}", seq % 7), seq);
            match seq % 50 {
                // An escaped string in its opening, through the writer itself.
                7 => {
                    record["labels"]["step"] = json!("a \"quoted\" step");
                    out_of_form.insert(key);
                }
                // The same record, its fields in another order than any writer leaves.
                19 => {
                    let line = serde_json::to_string(
                        &serde_json::from_value::<Envelope>(record).expect("a record"),
                    )
                    .expect("a line");
                    let fields: serde_json::Map<String, Value> =
                        serde_json::from_str(&line).expect("an object");
                    let reversed: Vec<String> = fields
                        .iter()
                        .rev()
                        .map(|(key, value)| format!("{}:{value}", json!(key)))
                        .collect();
                    appended.push(format!("{{{}}}", reversed.join(",")));
                    out_of_form.insert(key);
                    continue;
                }
                _ => {}
            }
            writer
                .relay(&serde_json::from_value(record).expect("a record"))
                .expect("relayed");
        }
        drop(writer);
        let mut text = std::fs::read_to_string(paths.journal()).expect("the journal");
        if !spliced.is_empty() {
            let lines: Vec<&str> = text.lines().collect();
            let (before, after) = lines.split_at(lines.len() / 2);
            text = before
                .iter()
                .copied()
                .chain(spliced.iter().map(String::as_str))
                .chain(after.iter().copied())
                .map(|line| format!("{line}\n"))
                .collect();
        }
        for line in appended {
            text.push_str(&line);
            text.push('\n');
        }
        std::fs::write(paths.journal(), text).expect("the journal");
        Written {
            root,
            paths,
            out_of_form,
        }
    }

    fn keys(parsed: &[whole_parses::Parsed]) -> Vec<Key> {
        let mut keys: Vec<Key> = parsed
            .iter()
            .map(|parsed| (parsed.stream.clone(), parsed.seq))
            .collect();
        keys.sort();
        keys
    }

    /// Each record the heads read and then a fetch parse whole is fetched, of a kind the
    /// fold parses, or a line written out of the writers' form; and every one fetched is.
    #[test]
    fn only_what_is_fetched_folded_whole_or_out_of_form_is_ever_parsed_whole() {
        let run = written("parses");
        let complete = RunView::open(&run.paths).expect("opens").events;
        assert!(complete.len() > 1_200, "the fixture holds every record");
        // What the heads read must parse, and nothing beside it.
        let mut folded_whole: Vec<Key> = complete
            .iter()
            .filter(|record| {
                !NEVER_PARSED.contains(&record.kind.0.as_str())
                    || run
                        .out_of_form
                        .contains(&(record.stream.clone(), record.seq))
            })
            .map(|record| (record.stream.clone(), record.seq))
            .collect();
        folded_whole.sort();
        assert!(
            complete
                .iter()
                .any(|record| NEVER_PARSED.contains(&record.kind.0.as_str())
                    && run
                        .out_of_form
                        .contains(&(record.stream.clone(), record.seq))),
            "the fixture holds headed kinds out of form"
        );
        for kind in KINDS {
            assert!(
                complete.iter().any(|record| record.kind.0 == kind),
                "{kind}"
            );
        }

        let all = |heads: &crate::views::RunHeads| -> Vec<RecordHandle> {
            heads.heads.iter().map(|head| head.handle.clone()).collect()
        };
        let chosen = |heads: &crate::views::RunHeads| -> Vec<RecordHandle> {
            // A few of each kind, the five headed kinds among them, in another order.
            heads
                .heads
                .iter()
                .rev()
                .filter(|head| head.seq % 97 == 3)
                .map(|head| head.handle.clone())
                .collect()
        };
        let none = |_: &crate::views::RunHeads| -> Vec<RecordHandle> { Vec::new() };
        type Fetched<'a> = &'a dyn Fn(&crate::views::RunHeads) -> Vec<RecordHandle>;
        let answers: [(&str, Fetched<'_>); 3] =
            [("none", &none), ("a few", &chosen), ("every", &all)];
        for (what, fetched) in answers {
            whole_parses::start();
            let heads = RunView::heads(&run.paths).expect("the heads read");
            let read = whole_parses::finish();
            // As a set: where the journal's last line is out of the form, the view's
            // reading of where its refresh resumes parses that line a second time.
            assert_eq!(
                keys(&read).into_iter().collect::<BTreeSet<Key>>(),
                folded_whole.iter().cloned().collect::<BTreeSet<Key>>(),
                "{what}: the heads read's parses"
            );
            let handles = fetched(&heads);
            whole_parses::start();
            let records = fetch(&run.paths.journal(), &handles).expect("fetched");
            let fetching = whole_parses::finish();
            let mut named: Vec<Key> = handles
                .iter()
                .map(|handle| (handle.stream.to_string(), handle.seq))
                .collect();
            named.sort();
            assert_eq!(
                keys(&fetching),
                named,
                "{what}: a fetch parses what it names"
            );
            assert_eq!(records.len(), handles.len());
            if what == "a few" {
                assert!(
                    records
                        .iter()
                        .filter(|record| NEVER_PARSED.contains(&record.kind.0.as_str()))
                        .count()
                        > 5,
                    "the few fetched hold headed kinds"
                );
            }
            // No headed record in the form that was not fetched was parsed whole.
            let fetched_keys: BTreeSet<Key> = named.iter().cloned().collect();
            for parsed in read.iter().chain(&fetching) {
                let key = (parsed.stream.clone(), parsed.seq);
                assert!(
                    fetched_keys.contains(&key)
                        || !NEVER_PARSED.contains(&parsed.kind.as_str())
                        || run.out_of_form.contains(&key),
                    "{what}: {key:?}, a {}, was parsed whole",
                    parsed.kind
                );
            }
        }
    }

    /// A line of a kind the fold parses whole, in the writers' form but malformed in its
    /// middle, among records of the five headed kinds: it is dropped as the complete read
    /// drops it, every head keeps the complete read's place, and no unfetched headed record
    /// is parsed whole, since the heads read never reads the journal through again.
    #[test]
    fn a_malformed_folded_line_is_dropped_without_parsing_a_headed_record_whole() {
        let record: Envelope = serde_json::from_value(json!({
            "v": 1, "ts": "2026-11-06T10:00:00.600Z", "stream": "graph-checks", "seq": 0,
            "source": "agentgraph", "kind": "change-check",
            "labels": {"run_id": "demo", "node": "build"},
            "payload": {"text": "intact"},
        }))
        .expect("a record");
        let malformed = serde_json::to_string(&record)
            .expect("a line")
            .replace("\"intact\"", "intact");
        assert!(
            crate::journal_selected::skimmed_line(malformed.as_bytes()).is_some(),
            "the malformed line is in the writers' form"
        );
        let run = written_with("malformed", std::slice::from_ref(&malformed));
        let complete = RunView::open(&run.paths).expect("opens");
        assert!(
            !complete
                .events
                .iter()
                .any(|record| record.stream == "graph-checks"),
            "the complete read drops the malformed line"
        );
        let mut folded_whole: BTreeSet<Key> = complete
            .events
            .iter()
            .filter(|record| {
                !NEVER_PARSED.contains(&record.kind.0.as_str())
                    || run
                        .out_of_form
                        .contains(&(record.stream.clone(), record.seq))
            })
            .map(|record| (record.stream.clone(), record.seq))
            .collect();

        whole_parses::start();
        let read = RunView::heads(&run.paths).expect("the heads read");
        let parsed = whole_parses::finish();
        let parsed_keys: BTreeSet<Key> = keys(&parsed).into_iter().collect();
        for parsed in &parsed {
            let key = (parsed.stream.clone(), parsed.seq);
            assert!(
                !NEVER_PARSED.contains(&parsed.kind.as_str()) || run.out_of_form.contains(&key),
                "{key:?}, a {}, was parsed whole and nobody fetched it",
                parsed.kind
            );
        }
        assert_eq!(parsed_keys, std::mem::take(&mut folded_whole));

        // Every head is the complete read's record at its place.
        assert_eq!(read.heads.len(), complete.events.len());
        let mut seen = BTreeSet::new();
        let mut place = 0;
        for (head, record) in read.heads.iter().zip(&complete.events) {
            let expected = seen.insert((record.stream.clone(), record.seq)).then(|| {
                place += 1;
                place - 1
            });
            assert_eq!(
                (
                    &head.ts,
                    &head.stream,
                    head.seq,
                    head.source,
                    &head.kind,
                    head.phase,
                    &head.labels,
                    head.evidence,
                    head.place,
                ),
                (
                    &record.ts,
                    &record.stream,
                    record.seq,
                    record.source,
                    &record.kind,
                    record.dimensions.phase,
                    &record.labels,
                    !record.artifacts.is_empty(),
                    expected,
                ),
            );
        }
        assert_eq!(read.store_len, place);
        assert_eq!(
            read.last_seq,
            complete
                .events
                .iter()
                .map(|record| record.seq)
                .max()
                .unwrap_or(0)
        );
        // And the state it folded is `open`'s.
        assert_eq!(read.view.state.statuses(), complete.state.statuses());
        assert_eq!(read.view.summary(), complete.summary());
        let reading =
            |view: &RunView| serde_json::to_value(crate::views::reading(view)).expect("a reading");
        assert_eq!(reading(&read.view), reading(&complete));
        let tally = |view: &RunView| {
            view.state.activity["build"]
                .progress
                .map(|progress| progress.events())
        };
        assert_eq!(tally(&read.view), tally(&complete));
    }
}
