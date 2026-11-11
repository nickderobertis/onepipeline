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

use std::path::Path;
use std::sync::Arc;

use serde_json::{Map, Value};

use crate::event::{Envelope, EventKind, Labels, Phase, Source};
use crate::journal_selected::{
    finished_lines, in_order, merged, number_of, opening_of, parsed_line, skimmed_line, string_of,
    whole_file, whole_record, Skim,
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

/// What a heads read gives the view it folds.
#[derive(Debug, Default)]
pub(crate) struct Heads {
    /// Every record's head, in merged order.
    pub(crate) heads: Vec<RecordHead>,
    /// Every record, in merged order, as the fold reads it: whole, or, for one of
    /// [`HEADED_KINDS`] in the writers' form, its head with nothing past it.
    pub(crate) folded: Vec<Envelope>,
    /// The merged store's length.
    pub(crate) store_len: usize,
    /// The highest `seq` in the whole journal.
    pub(crate) last_seq: u64,
}

/// A labels object in the form [`opening_of`](crate::journal_selected::opening_of)
/// skims, read whole: string values, a number for `round`, and keys with no escape.
/// `None` for any other, which the record's whole parse then reads.
fn labels_of(mut rest: &[u8]) -> Option<Labels> {
    let mut labels = Labels::default();
    let mut extra = Map::new();
    if rest.starts_with(b"}") {
        return Some(labels);
    }
    loop {
        let (key, after) = string_of(rest.strip_prefix(b"\"")?)?;
        let after = after.strip_prefix(b":")?;
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
        rest = if key == "round" {
            let (round, after) = number_of(after)?;
            labels.round = Some(round);
            after
        } else {
            let (value, after) = string_of(after.strip_prefix(b"\"")?)?;
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
            after
        };
        if rest.starts_with(b"}") {
            labels.extra = extra;
            return Some(labels);
        }
        rest = rest.strip_prefix(b",")?;
    }
}

/// A head and what the fold reads of one line, `None` where the line's record is one
/// only its whole parse can say and that parse refuses it.
fn head_of(
    skim: &Skim<'_>,
    place: Option<usize>,
    handle: RecordHandle,
) -> Option<(RecordHead, Envelope)> {
    let whole = |record: Envelope| {
        let head = RecordHead {
            ts: record.ts.clone(),
            stream: record.stream.clone(),
            seq: record.seq,
            source: record.source,
            kind: record.kind.clone(),
            phase: record.dimensions.phase,
            labels: record.labels.clone(),
            evidence: !record.artifacts.is_empty(),
            place,
            handle: handle.clone(),
        };
        (head, record)
    };
    if let Some(record) = &skim.parsed {
        return Some(whole((**record).clone()));
    }
    let headed =
        opening_of(skim.line).and_then(|opening| Some((labels_of(opening.labels)?, opening)));
    let Some((labels, opening)) = headed else {
        return whole_record(skim.line).map(whole);
    };
    if !HEADED_KINDS.contains(&opening.kind) {
        return whole_record(skim.line).map(whole);
    }
    let head = RecordHead {
        ts: opening.ts.to_owned(),
        stream: opening.stream.to_owned(),
        seq: opening.seq,
        source: opening.source,
        kind: EventKind(opening.kind.to_owned()),
        phase: opening.phase,
        labels,
        evidence: skim.evidence,
        place,
        handle,
    };
    let folded = Envelope {
        v: opening.v,
        ts: head.ts.clone(),
        stream: head.stream.clone(),
        seq: head.seq,
        source: head.source,
        kind: head.kind.clone(),
        dimensions: head.phase.into(),
        labels: head.labels.clone(),
        payload: Map::new(),
        artifacts: Vec::new(),
    };
    Some((head, folded))
}

/// The heads of `bytes`' lines, read by `skim`; `None` where a line whose record
/// the fold needs whole turned out to be no record.
fn heads_of(bytes: &[u8], skim: for<'a> fn(&'a [u8]) -> Option<Skim<'a>>) -> Option<Heads> {
    let finished: Vec<&[u8]> = finished_lines(bytes).collect();
    let lines: Vec<Skim<'_>> = in_order(&finished, |line| skim(line))
        .into_iter()
        .flatten()
        .collect();
    let (queues, order) = merged(&lines);
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
    let placed: Vec<(usize, Option<usize>)> = order
        .iter()
        .map(|index| {
            last_seq = last_seq.max(lines[*index].seq);
            let place = (!repeated[*index]).then(|| {
                store_len += 1;
                store_len - 1
            });
            (*index, place)
        })
        .collect();
    let start = bytes.as_ptr() as usize;
    let read = in_order(&placed, |(index, place)| {
        let skim = &lines[*index];
        let handle = RecordHandle {
            offset: (skim.line.as_ptr() as usize - start) as u64,
            len: skim.line.len() as u64,
            stream: Arc::clone(
                stream_of[*index]
                    .as_ref()
                    .expect("every line is in a queue"),
            ),
            seq: skim.seq,
        };
        head_of(skim, *place, handle)
    });
    let mut heads = Vec::with_capacity(read.len());
    let mut folded = Vec::with_capacity(read.len());
    for line in read {
        let (head, record) = line?;
        heads.push(head);
        folded.push(record);
    }
    Some(Heads {
        heads,
        folded,
        store_len,
        last_seq,
    })
}

/// Every record's head, and what the fold reads of each.
///
/// A journal that cannot be read reads as one holding nothing, which is the rule the
/// complete read follows. Where a line in the writers' form whose record the fold reads
/// whole is no record, the whole read is the complete reader's, line by line, as the
/// selective read's is.
pub(crate) fn read_heads(journal: &Path) -> Heads {
    let Some(bytes) = whole_file(journal) else {
        return Heads::default();
    };
    crate::loopstats::journal_read(journal, 0, bytes.len() as u64);
    heads_of(&bytes, |line| {
        skimmed_line(line).or_else(|| parsed_line(line))
    })
    .or_else(|| heads_of(&bytes, parsed_line))
    .unwrap_or_default()
}

/// `buffer` filled from `file` at `offset`.
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
        let answers: [(&str, &dyn Fn(&crate::views::RunHeads) -> Vec<RecordHandle>); 3] =
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
}
